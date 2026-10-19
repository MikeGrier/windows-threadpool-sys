<!-- Split from ../durable-ioring/CONTRACT.md at 4fbcb343: the worked examples at its end. -->
# win-durable-ioring

A ring for Windows file I/O with **durability scheduled into its work**: the Windows driver for
[durable-ioring](../durable-ioring/COMPONENT.md), built on
[windows-ioring-sys](../windows-ioring-sys/README.md)'s `IoRing`. A consumer submits reads and
writes as it would to the ring, tags each write with an epoch, asks for durability with
`make_durable_through(n)`, and learns -- in the same completion stream as its operations -- when
everything through `n` has become durable, or that it has failed and which writes are suspect as a
result.

## What is here, and what is not

**Here** ([DI-D-50](../durable-ioring/DESIGN-NOTES.md#di-d-50)): `Dioring`, the threaded driver
that implements the contract over the Windows backend; the backend and its file target
(`windows_backend`), the thread-pool and pumped and signalled delivery of its completions, the
readiness `Event` and the harness check of it (`readiness`), the interrupt-time clock the driver
stamps with, and a way to read an `HRESULT` out of an `io::Error` (`error_code_of`). It forbids
`unsafe` code: a Win32 call no lower crate wraps safely is added to that crate.

**Not here:** the contract, the core that decides when writes are durable, the target and backend
traits, the in-memory backend and the conformance oracle. Those are
[durable-ioring](../durable-ioring/COMPONENT.md), which has no Windows dependency, so another
platform's driver can be built on it as this one is. The specification is that crate's
[CONTRACT.md](../durable-ioring/CONTRACT.md); its spelling as Rust types is
[API.md](../durable-ioring/API.md). This crate re-exports the portable crate's API, so a consumer
needs only this one.

A blocking push is not an API of either crate: a refused push names its cause, and the loop that
waits it out is a maintained example, [examples/blocking_push/](examples/blocking_push/main.rs), to
use as it stands or adapt ([DI-D-56](../durable-ioring/DESIGN-NOTES.md#di-d-56)).

## Opening a file

A file target is made from a file **this crate opened**, and from nothing else
([DI-D-60](../durable-ioring/DESIGN-NOTES.md#di-d-60)). How a handle was opened decides what its
target can claim, and the only call that reports it is one this repository does not make, so the
open mode is recorded by the type that opens the file instead of asked of a handle afterwards.
`OpenedFile<Cached>` is a file opened for cached I/O: it claims `BestEffort` and needs no alignment.
`OpenedFile<Unbuffered>` is opened with no intermediate buffering: it claims `SharedDevice`, needs
a sector's alignment (4096 unless told less with `with_sector_size`), and offers per-write
write-through. A consumer that needs the stronger warranty asks for `OpenedFile<Unbuffered>` in its
own signature, and a cached file cannot be passed. A handle opened elsewhere cannot be given to a
target in this version.

The data access of a handle is its type. `FileOptions::new()` asks for none, `read()` and `write()` add
the right to read and to write, and the file an open makes is an `OpenedFile<B, A>` whose `A` is
`NoData`, `ReadOnly`, `WriteOnly` or `ReadWrite`. A target needs a data right, so a file opened with
`NoData` cannot be made into one, and the program does not compile. `OpenedDisk` and `OpenedVolume` carry
the same parameter, so either can be opened for reading only. The other rights an open can ask for, the
attribute, delete and security rights, are a `FileRights` set given to `FileOptions::rights`; they gate
calls a consumer makes on the handle it is lent and not anything this crate does. There is no
constructor from a raw mask and no right for appending only, which would land every write at the end
of the file whatever offset the core names, so it cannot be asked for. What the file's security
allows is the kernel's to say.

`FileOptions::share` takes a `ShareMode`, a set of `READ`, `WRITE` and `DELETE` naming what other opens of
the file may request while this one is open; a file is opened sharing all three unless it is told
otherwise, and `ShareMode::NONE` is an exclusive open. An open that the sharing of either side does not
allow fails as a `SharingViolation`, an `io::ErrorKind::ResourceBusy` error whose cause carries what this
open asked for. `CreateFileW` reports both sides' conflicts as one error and no documented call reports
what the other open holds, so it cannot say which side conflicted. The kernel's own error is its source.

`FileOptions::random_access` and `FileOptions::sequential_scan` are the two cache hints
(`FILE_FLAG_RANDOM_ACCESS` and `FILE_FLAG_SEQUENTIAL_SCAN`). They are passed to an unbuffered file as
well as a cached one, never refused, and change nothing a target claims: the class, the alignment and
what it offers follow from the buffering mode alone. What the `CreateFileW` documentation says about them
on an unbuffered file can be read two ways, and the methods' own documentation says what was measured.

`FileOptions::overlapped` sets `FILE_FLAG_OVERLAPPED`. The ring takes such a handle and gives the result
for each operation that it gives without the flag; what the flag changes is when a completion is there and,
for an unbuffered file, the order a batch's completions come in. The handle the open lends is the same
overlapped handle, so I/O a caller makes on it directly, without an offset, is I/O on a handle for which
`CreateFileW` says the file pointer is not maintained.

The path is a `Wtf16String`, handed to Win32 as the caller gave it: nothing lengthens, shortens or
refuses it, so a path past `MAX_PATH` without a `\\?\` prefix, and an empty one, are what `CreateFileW`
makes of them. Convert a `Path` with `Wtf16String::from_os_str`, which loses nothing.

`FileOptions::disposition` takes a `Disposition`: `CreateNew`, `OpenExisting` (where the options start) or
`OpenAlways`, named as `CreateFileW` names them. `CREATE_ALWAYS` and `TRUNCATE_EXISTING`, the two that
empty a file that exists, are not offered in this version; a caller empties a file through the handle the
open lends, with `File::set_len(0)`. `FileOptions::open_reparse_point` sets `FILE_FLAG_OPEN_REPARSE_POINT`,
which no disposition implies: without it a link is followed, including by `CreateNew`.

`OpenedFile::open` blocks for as long as `CreateFileW` does. `OpenedFile::request` builds an `OpenRequest`
and opens nothing, fixing a relative path to the current directory as it is then, and
`OpenRequest::perform` makes the call on whatever thread the caller chooses. A request is `Send`, and
performing it again opens the file again. The refusal of an unbuffered file on a remote device is made in
`perform`, on the opened handle from its final path, so a file the open created is still there when it is
refused.

A file the open creates is given `FileOptions::attributes`, a `FileAttributes` set of `ARCHIVE`, `HIDDEN`,
`OFFLINE`, `READONLY`, `SYSTEM` and `TEMPORARY` (`FILE_ATTRIBUTE_NORMAL`, the absence of attributes, is the
empty set, and `FILE_ATTRIBUTE_ENCRYPTED` is not offered), and `OpenRequest::with_security_descriptor`, a
`SecurityDescriptor` built from SDDL, which the request owns and frees. The descriptor is on the request and
not in the options because it owns memory and cannot be copied, as the options can. Neither applies to a file
that already exists. Handles are never inheritable, and there is no option to make one so.

```rust
use win_durable_ioring::windows_backend::{
    Cached, Disposition, FileAttributes, FileOptions, OpenedFile, SecurityDescriptor,
};
use win_durable_ioring::Wtf16String;

let path = std::env::temp_dir().join(format!("durable-ioring-create-{}", std::process::id()));
let options = FileOptions::new()
    .read()
    .write()
    .disposition(Disposition::CreateNew)
    .attributes(FileAttributes::HIDDEN);
let descriptor = SecurityDescriptor::from_sddl(&Wtf16String::from("D:P(A;;GA;;;WD)"))?;

let created = OpenedFile::<Cached>::request(Wtf16String::from_os_str(path.as_os_str()), options)?
    .with_security_descriptor(descriptor)
    .perform()?;
drop(created);
# std::fs::remove_file(&path)?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

```rust
use win_durable_ioring::windows_backend::{
    Cached, Disposition, FileOptions, OpenedFile, Unbuffered, Win32FileTarget,
};
use win_durable_ioring::{IoTarget, WarrantyClass, Wtf16String};

let path = std::env::temp_dir().join(format!("durable-ioring-open-{}", std::process::id()));
std::fs::write(&path, [0u8; 8192])?;
let options = FileOptions::new().read().write();
// The open takes the path as the caller gave it, a `Wtf16String`: converting loses nothing.
let name = || Wtf16String::from_os_str(path.as_os_str());

let cached = Win32FileTarget::new(OpenedFile::<Cached>::open(name(), options)?);
assert_eq!(cached.warranty(), WarrantyClass::BestEffort);

let unbuffered = Win32FileTarget::new(OpenedFile::<Unbuffered>::open(name(), options)?);
assert_eq!(unbuffered.warranty(), WarrantyClass::SharedDevice);
assert!(unbuffered.capabilities().write_through());

// Built here, and made on the thread that may block.
let request = OpenedFile::<Cached>::request(name(), options.disposition(Disposition::OpenAlways))?;
let opened = std::thread::spawn(move || request.perform()).join().expect("the thread")?;
drop(opened);
# std::fs::remove_file(&path)?;
# Ok::<(), std::io::Error>(())
```

An unbuffered file is refused on a remote device, where the claim cannot hold. The handle is asked where it
is after the open, so a link is judged by the handle it opens: followed, the file it leads to; opened with
`open_reparse_point`, the link itself.

### By identity

A file has an identity, `FileId`: its volume's serial number and a 128-bit identifier, read from any handle to
it. `OpenedFile::open_by_id` opens it again by that, or by a 64-bit file ID or an object ID, on the volume of a
hint, which is any handle on that volume ([DI-D-70](../durable-ioring/DESIGN-NOTES.md#di-d-70)). Its options are
`ByIdOptions`, since `OpenFileById` takes no disposition and no attributes. The call never sees a `FileId`'s
volume, so the file it opens is asked its identity, and one from another volume is refused as an
`IdentityMismatch`.

```rust
use win_durable_ioring::windows_backend::{ByIdOptions, Cached, FileOptions, OpenedFile};
use win_durable_ioring::Wtf16String;

let path = std::env::temp_dir().join(format!("durable-ioring-by-id-{}", std::process::id()));
std::fs::write(&path, b"by identity")?;
let file = OpenedFile::<Cached, _>::open(
    Wtf16String::from_os_str(path.as_os_str()),
    FileOptions::new().read(),
)?;
let id = file.file_id()?;

// Any handle on the volume is a hint: here, another file's, opened by std.
let neighbour = path.with_extension("neighbour");
std::fs::write(&neighbour, b"")?;
let hint = std::fs::File::open(&neighbour)?;
let again = OpenedFile::<Cached, _>::open_by_id(hint, id, ByIdOptions::new().read())?;
assert_eq!(again.file_id()?, id);
# drop((file, again));
# std::fs::remove_file(&path)?;
# std::fs::remove_file(&neighbour)?;
# Ok::<(), std::io::Error>(())
```

### A raw volume

A raw volume is a target too ([DI-D-61](../durable-ioring/DESIGN-NOTES.md#di-d-61)): one handle, one
commit per volume, and **regions are offsets the consumer allocates inside it**, not targets of their
own. `OpenedVolume` opens it by a path that can only name a volume, a drive letter or a volume GUID,
and finds what the backing must know for itself with documented device controls
([win-volume-sys](../win-volume-sys/README.md)): the disks it sits on, which become its flush
domains (`disk_flush_domain` names a disk, so a consumer can declare the same domain for a file on
it), and the sector it is addressed in, which is the alignment of its operations. A volume that is
remote, or not a volume, is refused. It claims `SharedDevice` when opened unbuffered and `BestEffort` when opened cached, and never
`Complete`: a lock keeps other handles off the volume, not other writers off its disks. Opening one for writing needs an
administrator, and `VolumeOptions::lock` takes the volume lock, which a write to a volume with a
file system on it needs and which fails if anything else has the volume open.

```rust,no_run
use win_durable_ioring::windows_backend::{
    OpenedVolume, ReadWrite, Unbuffered, VolumeOptions, VolumePath, Win32FileTarget,
};

let path = VolumePath::drive('E')?;
// The data access is named in the type: a volume opened for reading only is `ReadOnly`.
let volume =
    OpenedVolume::<Unbuffered, ReadWrite>::open(&path, VolumeOptions::new().lock(true))?;
let target = Win32FileTarget::from_volume(volume);
# Ok::<(), Box<dyn std::error::Error>>(())
```

### A whole disk

`OpenedDisk` opens a physical disk by its number, `\\.\PhysicalDriveN`, rather than a volume on it
([DI-D-62](../durable-ioring/DESIGN-NOTES.md#di-d-62)). The disk is its own flush domain, named by
`disk_flush_domain` as a volume on it would name it, and its operations are aligned to its sector,
which a device control reports. It claims `SharedDevice` when opened unbuffered and `BestEffort` when opened cached, never
`Complete`, for the reason a volume does not. Windows documents that it blocks a direct write to a sector a mounted volume covers, so a
disk with volumes on it is written through them, or has them locked or is taken offline first; the
tests here use a disk with no partition. Opening one for writing needs an administrator. A disk and a
volume are opened sharing reading, writing and deletion unless `DiskOptions::share` or
`VolumeOptions::share` takes a `ShareMode` otherwise, and a conflict is the same `SharingViolation` a file
reports.

```rust,no_run
use win_durable_ioring::windows_backend::{
    DiskOptions, DiskPath, OpenedDisk, ReadOnly, Unbuffered, Win32FileTarget,
};

// The data access is named in the type: this one is opened for reading only.
let disk = OpenedDisk::<Unbuffered, ReadOnly>::open(DiskPath::number(3), DiskOptions::new())?;
let target = Win32FileTarget::from_disk(disk);
# Ok::<(), std::io::Error>(())
```

## Worked examples

Each example is dioring running the trace in its comments, compiled and run as a test, so a change
that breaks one breaks the build. In the comments, `->` is a call the consumer makes and `<-` an
entry it pops. Every write is in the default lineage, so the lineage is not shown; construction
returns a handle to it, and `tag(n)` is its epoch n as a write or a seal takes it, `epoch(n)` as a
query or an entry names it. A commit failure cannot be provoked on a healthy machine through a Win32
file, so each example arms one with dioring's fault seam, `fail_next_flush`, which makes the next
flush of a file report the error given whatever the device answered. Setting up the instance, and a
`pop_until` that pops until an entry it is looking for, are hidden.

### Healing a failure

```rust
# use std::time::{Duration, Instant};
# use win_durable_ioring::*;
# type Ring = Dioring<Vec<u8>>;
# const FILE: IoTargetKey = IoTargetKey(1);
# fn pop_until(ring: &mut Ring, stop: impl Fn(&EntryOf<Ring>) -> bool) -> EntryOf<Ring> {
#     let started = Instant::now();
#     loop {
#         match ring.pop().expect("pop") {
#             Some(entry) if stop(&entry) => return entry,
#             Some(_) => {}
#             None => {
#                 assert!(started.elapsed() < Duration::from_secs(10), "nothing arrived");
#                 std::thread::sleep(Duration::from_millis(1));
#             }
#         }
#     }
# }
# let path = std::env::temp_dir().join(format!("durable-ioring-heal-{}", std::process::id()));
# std::fs::write(&path, [0u8; 64])?;
# let file = windows_backend::OpenedFile::<windows_backend::Cached>::open(
#     win_durable_ioring::Wtf16String::from_os_str(path.as_os_str()),
#     windows_backend::FileOptions::new().read().write(),
# )?;
# let (mut ring, lineage) = Ring::new(Setup {
#     submission_queue_size: 64,
#     completion_queue_size: 64,
#     files: vec![TargetSetup::new(
#         FILE,
#         windows_backend::Win32FileTarget::new(file),
#     )],
#     buffers: Vec::new(),
#     provider: None,
#     diagnostics: None,
#     delivery: DeliveryMode::Pool(None),
# })
# .expect("an instance");
let tag = |n: u64| lineage.at(n); // what a write or a seal takes
let epoch = |n: u64| Epoch::new(lineage.lineage(), n); // what a query or an entry names

// -> write(tag 41, A) ; write(tag 41, B) ; write(tag 41, C)
for offset in [0, 8, 16] {
    ring.write(FILE, offset, vec![1; 8], tag(41), ()).expect("a write");
}
ring.fail_next_flush(FILE, 1117); // ERROR_IO_DEVICE, for the flush the seal issues
// -> make_durable_through(41)                 // seals everything <= 41
ring.make_durable_through(tag(41)).expect("a seal");
// <- WriteDone(A) ; WriteDone(B) ; WriteDone(C)
// <- Failed { failure: F1, cause: Commit, suspect: [A@41, B@41, C@41] }
let Entry::Failed(f1) = pop_until(&mut ring, |e| matches!(e, Entry::Failed(_))) else {
    unreachable!()
};
assert!(matches!(f1.cause, FailureCause::Commit { .. }));
assert_eq!(f1.suspect.writes().len(), 3);
assert_eq!(ring.durable_through(lineage.lineage()), Ok(None)); // nothing is durable yet
// -> write(tag 41, A')                        // refused: 41 is sealed (guarantee 6)
let refused = ring.write(FILE, 0, vec![2; 8], tag(41), ()).expect_err("41 is sealed");
assert!(matches!(refused.reason, PushRefusal::Sealed { .. }));
// -> write(tag 42, A') ; write(tag 42, B') ; write(tag 42, C')
for offset in [0, 8, 16] {
    ring.write(FILE, offset, vec![2; 8], tag(42), ()).expect("a write");
}
// -> heal(F1)                                 // "I have re-issued what I need"
ring.resolve(vec![(f1.token, Resolution::Heal)]).expect("heal");
// -> make_durable_through(42)                 // the first after the heal reaching the re-issue
ring.make_durable_through(tag(42)).expect("a seal");
// <- WriteDone(A') ; WriteDone(B') ; WriteDone(C')
// <- Durable { through: 42 }                  // F1 healed; 41 and 42 durable
pop_until(&mut ring, |e| matches!(e, Entry::Durable { through } if *through == epoch(42)));
assert_eq!(ring.epoch_state(epoch(41)), Ok(EpochState::Durable));
assert!(ring.failures().is_empty());
# drop(ring);
# std::fs::remove_file(&path)?;
# Ok::<(), std::io::Error>(())
```

Without the heal, the last entry is `Blocked { through: 42, by: F1 }`: 42's own commit succeeded, but
the high-water mark cannot pass the unresolved failure at 41.

### Failures and epochs are many-to-many

`P` is pushed before the seal, rather than after it as a consumer might, so that it is certainly
pushed before the failure is observed: the seal's commit is issued as soon as `A` completes.

```rust
# use std::time::{Duration, Instant};
# use win_durable_ioring::*;
# type Ring = Dioring<Vec<u8>>;
# const FILE: IoTargetKey = IoTargetKey(1);
# fn pop_until(ring: &mut Ring, stop: impl Fn(&EntryOf<Ring>) -> bool) -> EntryOf<Ring> {
#     let started = Instant::now();
#     loop {
#         match ring.pop().expect("pop") {
#             Some(entry) if stop(&entry) => return entry,
#             Some(_) => {}
#             None => {
#                 assert!(started.elapsed() < Duration::from_secs(10), "nothing arrived");
#                 std::thread::sleep(Duration::from_millis(1));
#             }
#         }
#     }
# }
# let path = std::env::temp_dir().join(format!("durable-ioring-many-{}", std::process::id()));
# std::fs::write(&path, [0u8; 64])?;
# let file = windows_backend::OpenedFile::<windows_backend::Cached>::open(
#     win_durable_ioring::Wtf16String::from_os_str(path.as_os_str()),
#     windows_backend::FileOptions::new().read().write(),
# )?;
# let (mut ring, lineage) = Ring::new(Setup {
#     submission_queue_size: 64,
#     completion_queue_size: 64,
#     files: vec![TargetSetup::new(
#         FILE,
#         windows_backend::Win32FileTarget::new(file),
#     )],
#     buffers: Vec::new(),
#     provider: None,
#     diagnostics: None,
#     delivery: DeliveryMode::Pool(None),
# })
# .expect("an instance");
let tag = |n: u64| lineage.at(n); // what a write or a seal takes
let epoch = |n: u64| Epoch::new(lineage.lineage(), n); // what a query or an entry names
let ops = |set: &SuspectSet<WindowsIds<u64>>| set.writes().iter().map(|w| w.op).collect::<Vec<_>>();

// -> write(tag 41, A) ; write(tag 43, P)      // 43 is open
let a = ring.write(FILE, 0, vec![1; 8], tag(41), ()).expect("a write");
let p = ring.write(FILE, 8, vec![1; 8], tag(43), ()).expect("a write");
ring.fail_next_flush(FILE, 1117);
// -> make_durable_through(41)                 // seals 41
ring.make_durable_through(tag(41)).expect("a seal");
// <- WriteDone(A) ; WriteDone(P)
// <- Failed { failure: F1, cause: Commit, suspect: [A@41, P@43] }
let Entry::Failed(f1) = pop_until(&mut ring, |e| matches!(e, Entry::Failed(_))) else {
    unreachable!()
};
assert_eq!(ops(&f1.suspect), [a, p]); // one failure, two epochs: sealed 41 and open 43
// -> write(tag 43, R)                         // still allowed: 43 is open
let r = ring.write(FILE, 16, vec![1; 8], tag(43), ()).expect("a write");
// <- WriteDone(R)
pop_until(&mut ring, |e| matches!(e, Entry::Op(done) if done.id == r));
// -> import_failure()                         // the consumer learned of an outside failure
let f2_id = ring.import_failure(ImportScope::All).expect("the whole instance").failure;
// <- Failed { failure: F2, cause: Imported, suspect: [A@41, P@43, R@43] }
let Entry::Failed(f2) = pop_until(&mut ring, |e| matches!(e, Entry::Failed(_))) else {
    unreachable!()
};
assert_eq!(f2.id, f2_id);
assert_eq!(ops(&f2.suspect), [a, p, r]); // epoch 43 now sits in F1 and F2

// The first ending: heal both. Re-issue A, P and R under tag 44, resolve both in one atomic
// call, and seal 44.
for offset in [0, 8, 16] {
    ring.write(FILE, offset, vec![2; 8], tag(44), ()).expect("a write");
}
ring.resolve(vec![(f1.token, Resolution::Heal), (f2.token, Resolution::Heal)])
    .expect("heal both");
ring.make_durable_through(tag(44)).expect("a seal");
// <- Durable { through: 44 }                  // 41 and 43 passed once both failures were resolved
pop_until(&mut ring, |e| matches!(e, Entry::Durable { through } if *through == epoch(44)));
assert_eq!(ring.epoch_state(epoch(41)), Ok(EpochState::Durable));
assert_eq!(ring.epoch_state(epoch(43)), Ok(EpochState::Durable));
# drop(ring);
# std::fs::remove_file(&path)?;
# Ok::<(), std::io::Error>(())
```

The second ending: **heal F1, abandon F2.** Epochs 41 and 43 both sit in F2, so both become
**abandoned** -- one abandonment is enough -- even though F1 was healed. The high-water mark moves
past them once every failure containing them is resolved: F2 at once, and F1 when its heal takes
effect, at the first seal made after it that reaches the re-issue. Any operation gated on 41 or 43
fails as never issued.

```rust
# use std::time::{Duration, Instant};
# use win_durable_ioring::*;
# type Ring = Dioring<Vec<u8>>;
# const FILE: IoTargetKey = IoTargetKey(1);
# fn pop_until(ring: &mut Ring, stop: impl Fn(&EntryOf<Ring>) -> bool) -> EntryOf<Ring> {
#     let started = Instant::now();
#     loop {
#         match ring.pop().expect("pop") {
#             Some(entry) if stop(&entry) => return entry,
#             Some(_) => {}
#             None => {
#                 assert!(started.elapsed() < Duration::from_secs(10), "nothing arrived");
#                 std::thread::sleep(Duration::from_millis(1));
#             }
#         }
#     }
# }
# let path = std::env::temp_dir().join(format!("durable-ioring-abandon-{}", std::process::id()));
# std::fs::write(&path, [0u8; 64])?;
# let file = windows_backend::OpenedFile::<windows_backend::Cached>::open(
#     win_durable_ioring::Wtf16String::from_os_str(path.as_os_str()),
#     windows_backend::FileOptions::new().read().write(),
# )?;
# let (mut ring, lineage) = Ring::new(Setup {
#     submission_queue_size: 64,
#     completion_queue_size: 64,
#     files: vec![TargetSetup::new(
#         FILE,
#         windows_backend::Win32FileTarget::new(file),
#     )],
#     buffers: Vec::new(),
#     provider: None,
#     diagnostics: None,
#     delivery: DeliveryMode::Pool(None),
# })
# .expect("an instance");
# let tag = |n: u64| lineage.at(n);
# let epoch = |n: u64| Epoch::new(lineage.lineage(), n);
# ring.write(FILE, 0, vec![1; 8], tag(41), ()).expect("a write");
# ring.write(FILE, 8, vec![1; 8], tag(43), ()).expect("a write");
# ring.fail_next_flush(FILE, 1117);
# ring.make_durable_through(tag(41)).expect("a seal");
# let Entry::Failed(f1) = pop_until(&mut ring, |e| matches!(e, Entry::Failed(_))) else {
#     unreachable!()
# };
# let r = ring.write(FILE, 16, vec![1; 8], tag(43), ()).expect("a write");
# pop_until(&mut ring, |e| matches!(e, Entry::Op(done) if done.id == r));
# ring.import_failure(ImportScope::All).expect("the whole instance");
# let Entry::Failed(f2) = pop_until(&mut ring, |e| matches!(e, Entry::Failed(_))) else {
#     unreachable!()
# };
// The trace above, to F2's arrival, is hidden. Then:
ring.resolve(vec![(f1.token, Resolution::Heal), (f2.token, Resolution::Abandon)])
    .expect("heal F1, abandon F2");
// <- Abandoned { failure: F2, suspect: [A@41, P@43, R@43] }
pop_until(&mut ring, |e| matches!(e, Entry::Abandoned { .. }));
assert_eq!(ring.epoch_state(epoch(41)), Ok(EpochState::Abandoned));
assert_eq!(ring.epoch_state(epoch(43)), Ok(EpochState::Abandoned));
// -> make_durable_through(44)                 // F1's heal takes effect here
ring.make_durable_through(tag(44)).expect("a seal");
// <- Durable { through: 44 }                  // "durable through 44" means durable or abandoned
pop_until(&mut ring, |e| matches!(e, Entry::Durable { through } if *through == epoch(44)));
assert_eq!(ring.durable_through(lineage.lineage()), Ok(Some(44)));
assert_eq!(ring.epoch_state(epoch(43)), Ok(EpochState::Abandoned));
# drop(ring);
# std::fs::remove_file(&path)?;
# Ok::<(), std::io::Error>(())
```
