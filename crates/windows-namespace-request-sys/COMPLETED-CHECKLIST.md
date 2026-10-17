# Completed checklists: windows-namespace-request-sys

Append-only. Newest groups at the bottom.

## Moved 2026-09-10 -- NR-1: the per-drive current-directory arm, pinned

### <a id="nr-11"></a>NR-1.1 -- Pin the `=X:` arm of drive-relative rooting. *(completed 2026-09-10 12:24:46 UTC-04:00)*

- [x] **NR-1.1** -- Decide how this crate's tests may control process-global
  state, then pin the `=X:` arm of drive-relative rooting.

  **Completed in the round that raised it, because the blocker turned out not to
  exist.** The item was written to defer the work: pinning the arm needs a
  controlled `=X:`, and the two routes to one -- mutating process-global state
  that other test threads share, or spawning a child process with a crafted
  environment -- looked like a decision about this crate's test shape rather than
  something to take in passing.

  Measurement dissolved the first route's objection within the hour.
  `GetFullPathNameW` **itself** writes the `=X:` entry -- when resolving for a
  drive other than the current one, and when the recorded entry is absent or
  rejected, in which case it is written as the drive root. (An accepted entry is
  left alone, and the current-drive form writes nothing.) The code under test
  therefore already mutates that state on the very path these tests exercise, so
  a test that sets it first introduces no hazard that resolving alone did not,
  and there was nothing left to decide. Isolation across the tests comes from
  their disjoint drive letters.

  `a_drive_relative_path_uses_that_drives_entry_verbatim_and_rewrites_a_bad_one`
  in [drive_entry.rs](src/full_path/tests/drive_entry.rs) now pins all three behaviours: an entry
  naming an existing directory is honoured verbatim (onto a *different* drive,
  which is what makes "that drive's own current directory" a convention rather
  than a guarantee); an entry naming nothing is rejected in favour of the drive
  root; and the call writes the entry back, creating it on a host that had none.
  It draws its drive letter from a list disjoint from every sibling test's, so
  no two can select the same one and race under libtest's thread-per-test model.

  *(Later correction, recorded here because the archive is history and the
  history was briefly wrong: this entry described the selection as a `W`/`U`
  **pair**, matching the helper as written. That helper validated only its first
  letter and returned the second unchecked, so the guarantee the pair implied did
  not hold. The lists are now three letters each, every candidate is checked
  against both the current drive and the probe drive, and the lists themselves
  live in one table in [drive_entry.rs](src/full_path/tests/drive_entry.rs) with a test enforcing
  that they stay disjoint and long enough. This note deliberately does NOT
  enumerate them: an earlier version did, and named five lists after a sixth had
  been added -- so a reader picking letters for a seventh would have consulted
  an inventory missing three of the letters already in use. The table is the
  inventory.)*

  The sibling test keeps its weaker form and now says so: without controlling
  the entry it can only bound the arm.

  *(Later correction: that sibling was named
  `a_drive_relative_path_is_rooted_at_that_drive_and_not_the_process_directory`,
  and the name claimed two things its assertions do not reach -- `ends_with`
  accepts any base, including the process directory, and an accepted entry is
  used verbatim so the result need not be on that drive at all. It is now
  `a_drive_relative_path_carries_its_component_and_the_current_drive_uses_the_process_directory`.)*

## Moved 2026-10-13 20:30:00 -04:00 -- NR-3: an allocation that fails is an error the caller receives

### <a id="nr-31"></a>NR-3.1 -- Every allocation of an `AlignedBuf` returns its failure, and `SecurityDescriptor` and `SecurityAttributes` stop being `Clone`. *(completed 2026-10-13 20:30:00 -04:00)*

Raised by the engineer's review of [durable-ioring's DI-9.7.3](../durable-ioring/COMPLETED-CHECKLIST.md#di-973),
which found the first move onto `io_buffers::AlignedBuf` (`IB-2+.4`) had left a panic where there had been an
abort; the engineer rejected both ("I do not find either acceptable"). Done as one change, recorded here as
finished work: `QueryFileInformation::perform` returns `ERROR_NOT_ENOUGH_MEMORY`; `SecurityDescriptor::capture`
has a `SecurityCaptureFailure::Allocate`; `try_clone` replaces `Clone` on `SecurityDescriptor` and
`SecurityAttributes`; a request's `try_clone` reports `HandleCaptureFailure::CopySecurity`. A breaking change to
the published crate. The decision is in [DESIGN-NOTES.md](DESIGN-NOTES.md) under `D-8`; the sabotages that
pin it, and the one blind spot, are in [sabotage.json](sabotage.json), a manifest this change introduced.
Two of those sabotages -- a copy of an open request that forgets its security attributes -- would have survived
before: nothing tested that a copy kept them, so this change added the tests.

## Moved 2026-10-15 16:13:07 -04:00 -- NR-2.1: where a volume resides, through the one device-control helper

### <a id="nr-21"></a>NR-2.1 -- `FSCTL_QUERY_VOLUME_NUMA_INFO` on a file or directory handle, through the crate's one overlapped-safe `DeviceIoControl` helper. *(completed 2026-10-15 16:13:07 -04:00)*

`QueryVolumeNumaInfo` reports the NUMA node the volume under a file or directory handle resides on, as the
device wrote it; a failure is the device's own code, and a reply whose size is not the structure's is its own
error rather than a read. The call goes through a private helper, so every later control is the same call on an
overlapped handle and on a plain one and posts nothing to a completion port the handle belongs to. The decision
is [D-20](DESIGN-NOTES.md#d-20) and how it was reached is in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md). The code
is `2d6b757a`. `windows-ioring-sys`' unrun
[file-handle-numa-spike.rs](../windows-ioring-sys/design-sessions/spikes/file-handle-numa-spike.rs) records what
was and was not known about this control beforehand.

Measured here: a file on the system volume answers node `0`, and the `NUL` device answers
`ERROR_INVALID_FUNCTION`. This machine has one node, so a node other than `0`, and a volume that advertises no
proximity domain, were not observed. Ten sabotages pin it, in [sabotage.json](sabotage.json). Six cover the entry and the
controls; four cover the helper's pending path, which is driven through its `overlapped_call` seam with an
overlapped `ReadFile` on a named pipe, because no file-system control pends on demand. What this left to do is
`NR-2.2` (an input buffer for the helper, and a control that pends on demand, to exercise `DeviceIoControl`
itself through that path) and `NR-2.6` (the duplicate in `win-volume-sys`).

## Moved 2026-10-15 17:36:13 -04:00 -- NR-2.2: the runs of disk a volume is made of

### <a id="nr-22"></a>NR-2.2 -- `IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS` on a volume handle, with the helper's input buffer and its first real pending control. *(completed 2026-10-15 17:36:13 -04:00)*

`QueryVolumeDiskExtents` returns the runs of disk a volume is made of as a `Vec<DiskExtent>`, asks again at the
count the device names when the first buffer is too small, and refuses a reply that does not hold what it counts.
The `DeviceIoControl` helper now takes an input buffer, which `NR-2.3` and `NR-2.4` need, and
`FSCTL_REQUEST_OPLOCK`, which takes input and pends until another handle writes, is the first real control to
reach it through the pending path. The decisions are [D-21](DESIGN-NOTES.md#d-21) and the amended
[D-20](DESIGN-NOTES.md#d-20); how they were reached is in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md). The code
is `0b20b59a`.

Measured here: the system volume reports one run, and a buffer below the count plus one extent is refused with
`ERROR_INVALID_PARAMETER`, where `ERROR_INSUFFICIENT_BUFFER` was expected. A volume that spans disks was not
observed, so the multi-extent retry is tested against a fake device that answers as the real one was measured
to. Thirteen sabotages pin it, in [sabotage.json](sabotage.json), all caught. The checklist's premise that this
control needed the input buffer was wrong (`NR-2.3` and `NR-2.4` do); the buffer was added here regardless, as
scheduled. What this left to do is `NR-2.6`, which now covers two duplicates in `win-volume-sys`, and the
`IOCTL_STORAGE_*` codes of `NR-2.3` and `NR-2.4`, which have not been checked to work through the helper.

## Moved 2026-10-15 17:58:20 -04:00 -- NR-2.3: a disk's device unique identifier

### <a id="nr-23"></a>NR-2.3 -- `IOCTL_STORAGE_QUERY_PROPERTY` with `StorageDeviceUniqueIdProperty` on a disk handle, through a shared `storage_property` module. *(completed 2026-10-15 17:58:20 -04:00)*

`QueryStorageDeviceUniqueId` reports a disk's device unique identifier, which names the device and not the slot
it is in, as the bytes the device gave: the header, and the device ID descriptor, device descriptor and drive
layout signature it points to, each delimited from its own header and bounds-checked, none parsed. The query
structure and the learn-the-size loop are in `storage_property`, which `NR-2.4` reuses. The decision is
[D-22](DESIGN-NOTES.md#d-22) and how it was reached is in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md). The code
is `29a02f6e`.

Measured here: the control works through the helper; a buffer of exactly the header gets the header alone, one
between that and the reply gets `ERROR_MORE_DATA` with the header written, and a buffer or input shorter than its
structure is `ERROR_BAD_LENGTH`. The disk reports its device ID descriptor and neither other section. Unlike the
disk-extents entry, the `ERROR_MORE_DATA` half is reachable here, so the loop is tested against the real disk for
every first-buffer size. Twenty-one sabotages pin it, in [sabotage.json](sabotage.json), all caught. Not observed:
a disk that reports the other two sections, and one on another bus. What this left to do is `NR-2.4`, and the
question whether the identifiers inside the descriptors belong in this crate, which `D-22` records as a working
position and not a decision.

## Moved 2026-10-15 18:38:54 -04:00 -- NR-2.7: the generic storage property query

### <a id="nr-27"></a>NR-2.7 -- The generic `IOCTL_STORAGE_QUERY_PROPERTY` entry, `QueryStorageProperty`: any property, any query type, additional parameters, reply as bytes. *(completed 2026-10-15 18:38:54 -04:00)*

Raised by the engineer when `NR-2.3` was reviewed ("Generic property query: yes"; the identifiers inside the
descriptors stay unparsed, "at this time, no. We can revisit later if necessary"). `QueryStorageProperty` is the
same call as `QueryStorageDeviceUniqueId` with the property and the query type chosen by the caller, both as
newtypes over the raw values and neither validated, additional parameters held shared so a copy never allocates,
and the reply returned as exactly the bytes its header says. A query for whether a property exists returns no
structure, so it is one call with no output and an empty reply. The decision is
[D-22](DESIGN-NOTES.md#d-22) and how it was reached, including a probe whose zero bytes wrongly suggested
additional parameters were ignored, is in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md). The code is `7aec7e85`.

Measured here: an exists query on a real property succeeds with nothing written, an unknown property is
`ERROR_INVALID_FUNCTION` for both questions, and a mask query and an undefined query type are
`ERROR_NOT_SUPPORTED` for the properties tried, so what a type other than standard and exists returns is not
established. The unique-id property reads the first additional byte and the device property ignores them, which
is how their arrival at a real device is shown. Thirty-three sabotages cover `NR-2.3` and this item together, in
[sabotage.json](sabotage.json), all caught. What this left to do is `NR-2.4`.

## Moved 2026-10-15 19:01:45 -04:00 -- NR-2.4: the NUMA node a disk is attached to

### <a id="nr-24"></a>NR-2.4 -- `IOCTL_STORAGE_QUERY_PROPERTY` with `StorageDeviceNumaProperty` on a disk handle, the node carried unaltered. *(completed 2026-10-15 19:01:45 -04:00)*

`QueryStorageDeviceNumaNode` reports the NUMA node a disk is attached to, per disk so that a volume spanning
disks on different nodes reports each (the disks come from `QueryVolumeDiskExtents`) rather than one false
answer. The reply is the three `u32`s of `STORAGE_DEVICE_NUMA_PROPERTY`, and the node is carried as the device
wrote it: the checklist's "documented unknown value" was not found documented, so no sentinel is invented. The
decision is [D-23](DESIGN-NOTES.md#d-23) and how it was reached is in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md).
The code is `f46d95fe`.

Two things are stated in the record that the item did not anticipate. The property is documented but its
description names no reply structure and the structure's own reference page was not found; the structure is in
the public SDK header, and `D-23` marks that footing as one the engineer can overrule. And this property answers
a short buffer differently from the unique-id property (`ERROR_INSUFFICIENT_BUFFER` below the header, and the
header alone as a success between the header and the reply), which corrected `storage_property`'s documentation,
which had stated the other property's behaviour as the call's. Measured here: a twelve-byte reply and node zero on
a one-node machine; a node other than zero was not observed. Six sabotages pin it, in
[sabotage.json](sabotage.json), all caught. What this left to do is `NR-2.5` and `NR-2.6`, which is now unblocked.

## Moved 2026-10-15 19:42:00 -04:00 -- NR-2.5: the dependency a virtual disk has on its backing file

### <a id="nr-25"></a>NR-2.5 -- `GetStorageDependencyInformation` (virtdisk) on a volume, root directory or disk handle, with the version as an input and names copied out of the reply. *(completed 2026-10-15 19:42:00 -04:00)*

`QueryStorageDependencyInformation` asks which volumes or disks a volume hosts, or with `HOST_VOLUMES` which host
it: the link from a virtual disk to the file that backs it, for durable-ioring's flush domains. The decision is
[D-24](DESIGN-NOTES.md#d-24) and how it was reached is in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md). The code
is `0306fda0`.

The feature is `Win32_Storage_Vhd`. The call returns its status directly, and one status measured here is an
Win32 error (`ERROR_VIRTDISK_NOT_VIRTUAL_DISK`, `0xC03A0015`), which is carried as returned. The version is an input, and a buffer under 72 bytes is
`ERROR_INVALID_PARAMETER`, so the first size is raised to 72. **No entry was ever observed**: attaching a virtual
disk needs an elevated token, which this machine's account does not have. The entries and names are tested
against replies built from the structure's declared layout and a fake device, 25 sabotages pin them in
[sabotage.json](sabotage.json) (all caught), and `D-24` and the manifest's `notCoveredHere` say what an elevated
run on a machine that can attach a VHD would settle. What this left to do in this crate is `NR-2.6`.

## Moved 2026-10-15 20:38:00 -04:00 -- NR-2.6: where the storage-locality entries live

### <a id="nr-26"></a>NR-2.6 -- Decided 2026-10-15: the storage-locality entries and their helper belong in `win-volume-sys`, so the duplicate is closed by moving them there and deleting this crate's copies. *(completed 2026-10-15 20:38:00 -04:00)*

The item asked whether `win-volume-sys`' `control()` or this crate's helper should survive. It was
answered by a different question, the crate they belong in. The engineer ruled that volume-oriented
operations belong in the volume crate, and that its scope is physical disks, partition management,
raw partition access and volume management. This crate's charter is
namespace operations, and `NR-2.1` to `NR-2.5` and `NR-2.7` are not those: they went here because the
checklist placed them here, and that placement was not questioned when it should have been. The
decision is [WV-D-5](../win-volume-sys/DESIGN-NOTES.md#wv-d-5), and a handle that must be shared is a
`SharedHandle` ([WV-D-6](../win-volume-sys/DESIGN-NOTES.md#wv-d-6)). What this left to do is the move,
which is [WV-M2](../win-volume-sys/CHECKLIST.md#wv-m2----take-in-the-storage-locality-entries), and
the deletion, which is `NR-2.8`.

## Moved 2026-10-15 23:09:28 -04:00 -- NR-2.8: the storage-locality entries leave this crate

### <a id="nr-28"></a>NR-2.8 -- The storage-locality entries and the device-control helper are deleted from this crate, and `D-20` to `D-24` are pointers at the `WV-D` rows they became. *(completed 2026-10-15 23:09:28 -04:00)*

Deleted after [WV-2.1 to WV-2.5](../win-volume-sys/CHECKLIST.md) landed: `QueryVolumeNumaInfo`,
`QueryVolumeDiskExtents`, the storage-property entries (unique id, disk NUMA, the generic query),
`QueryStorageDependencyInformation`, and the `device_control` module, with their tests, their
sabotage entries, the README table, and the `windows-sys` features nothing else needs
(`Win32_Storage_Vhd`, `Win32_System_IO`, `Win32_System_Ioctl`, and `Win32_System_Pipes` for tests). A
search for each name found no consumer outside this crate, and the workspace builds without them.
`D-20` to `D-24` in [DESIGN-NOTES.md](DESIGN-NOTES.md) are marked superseded at the top of the row and
point at [WV-D-7](../win-volume-sys/DESIGN-NOTES.md#wv-d-7), `WV-D-3`, `WV-D-9` (for both `D-22` and
`D-23`) and `WV-D-10`; their reasoning stays in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md). The code is
the commit that deletes `src/device_control.rs`.

None of the entries was in a release: they were added after `windows-namespace-request-sys-v0.3.1`.
The `feat(namespace-request)` commits that added them will still be read by release-please when this
crate is next released, so its changelog would list features that are not in the crate; the release
pull request needs those lines removed or overridden. That is a release action, not a checklist item.
The sabotage manifest here no longer has a control entry of its own, and the one remaining `survives`
entry is a declared blind spot and not a control.

## Moved 2026-10-15 23:17:03 -04:00 -- NR-4.1: the shared `Win32Error`

### <a id="nr-41"></a>NR-4.1 -- `Win32Error` is `win_error_codes::Win32Error`, re-exported under the same name, and `ERROR_VIRTDISK_NOT_VIRTUAL_DISK` survives its `io::Error` form. *(completed 2026-10-15 23:17:03 -04:00)*

This crate's own type converted a code that does not fit an `i32` to an `io::Error` made with
`io::Error::other` and a message, so the code did not survive it; the virtdisk errors, such as
`ERROR_VIRTDISK_NOT_VIRTUAL_DISK` (`0xC03A0015`), are of that kind. The shared type holds the code at
std's boundary as a bit-preserving cast, for every code. The decision is
[WE-D-1](../win-error-codes/DESIGN-NOTES.md#we-d-1). The code is the commit that adds the dependency
and deletes the type from `src/outcome.rs`.

Every existing path keeps working, since the re-export has the same name. `perform` still snapshots the
code in the statement after the call and now builds it with `Win32Error::last`, and the crate's own
out-of-memory report stays here as a function, `out_of_memory`, because it cannot be a method on a type
this crate no longer owns. A test takes `ERROR_VIRTDISK_NOT_VIRTUAL_DISK` through `perform`, runs cleanup
that overwrites the thread's error, and reads the code back from the `io::Error`; the type it replaced
returned no code there. The `Display` text now gives the code in hex as well, and nothing in the
workspace matched on the old text. Three sabotage entries cover the snapshot and the out-of-memory
report, with a control.

What this left to do is the same swap in the enumeration crate, `ERR-1`.

## Moved 2026-10-15 23:54:41 -04:00 -- NR-3.1: the SDDL conversion

### <a id="nr-31-sddl"></a>NR-3.1 -- `SecurityDescriptor::from_sddl` and `to_sddl`, the SDDL conversion public in both directions. *(completed 2026-10-15 23:54:41 -04:00)*

This is the NR-3.1 of the open-surface milestone. The earlier archive entry [`nr-31`](#nr-31) is a
different item that had the same number before the milestone was renumbered, which is why this anchor
carries a suffix.

The item asked for a constructor from an SDDL string; the engineer ruled that the conversion functions
are public, in both directions, so `to_sddl` came with it, and a `SecurityInformation` mask selects the
parts it writes. The decision is [D-25](DESIGN-NOTES.md#d-25). The code is the commit that adds
`src/security/sddl.rs`.

Windows allocates what it returns, which is copied into the crate's own descriptor or string and freed
on every path, so no raw pointer reaches a caller. A string Windows refuses is its own code, unaltered;
an interior NUL is refused first. The length Windows reports for the written text was measured to be
unreliable and is not used. A file created through `OpenFile::with_security` with a descriptor from text
is given its DACL, read back through `GetSecurityInfo`.

Thirteen sabotage entries cover it, two of them declared blind spots: that the memory Windows allocated
is freed, and the kind reported when Windows refuses to write text, which no input was found to
provoke. A control entry is included. What this left to do is `NR-3.2`, and the release that
[DI-13.5](../durable-ioring/CHECKLIST.md) waits on.

## Moved 2026-10-16 00:06:41 -04:00 -- NR-3.2: the ReOpenFile entry

### <a id="nr-32-reopen"></a>NR-3.2 -- A `ReOpenFile` entry beside `OpenFile` and `OpenFileByIdentifier`, owning a duplicate of the original handle with nothing defaulted. *(completed 2026-10-16 00:06:41 -04:00)*

This is the NR-3.2 of the open-surface milestone. The decision is [D-26](DESIGN-NOTES.md#d-26). The code
is the commit that adds `src/reopen.rs`.

The request owns a duplicate of the handle it reopens, and the access, the share mode and the flags
all start at zero and are carried verbatim. Each refusal is the call's own code. The tests assert what
was measured: the three share-mode conflicts, access wider than the original's, `FILE_FLAG_NO_BUFFERING`
added to a handle opened without it, the `NUL` device, and the access denied that every directory
handle tried was answered with, which is recorded as measured and unexplained.

Thirteen sabotage entries cover it, a control among them; none is a declared blind spot. What this left
to do is `NR-3.3` and the path work, and the release that
[DI-13.6](../durable-ioring/CHECKLIST.md) waits on.

## Moved 2026-10-16 00:18:28 -04:00 -- NR-3.3: the captured root and the graft

### <a id="nr-33-graft"></a>NR-3.3 -- The captured root and the graft, as one internal type (`AnchoredPath`), additive and used by nothing yet. *(completed 2026-10-16 00:18:28 -04:00)*

This is the NR-3.3 of the open-surface milestone. The decision is [D-19](DESIGN-NOTES.md#d-19), with what
the implementation settled written at its end. The code is the commit that adds `src/anchored.rs`.

The item asked for a type that holds the path as given and the current directory captured when it was
built, and produces the path to hand to Win32. It grafts a clearly safe relative child onto the directory
at construction and stores the result beside the path as given, so `perform` allocates nothing. The
item's test list said `NUL` grafts; D-19's rule 5 lists it among the names that do not, and the rule
governs, which is recorded in D-19.

Thirty-two sabotage entries cover it, one a declared blind spot (that the directory is not read for a
path whose shape refuses a graft, which no test can observe) and one a control. The test that must change
the process current directory re-runs the test binary as a child, as `tests/unc_current_directory.rs` does.
What this left to do is `NR-3.4` to `NR-3.6`, one commit, which wire the type into `OpenFile` and
`WatchDirectory` and retire `prepare`.

## Moved 2026-10-16 12:56:56 -04:00 -- NR-3.4 to NR-3.6: the path entries take a `Wtf16String`

These three items were one commit, as the checklist said they would be, because
`win-durable-ioring` and `windows-platform-probes` reach `prepare` through path dependencies and a
commit that removed it without adapting them would not have built. The decisions are
[D-19](DESIGN-NOTES.md#d-19) and [D-27](DESIGN-NOTES.md#d-27). The code is the commit titled
`feat(namespace-request)!: open, watch and resolve by a Wtf16String, and retire prepare`.

### <a id="nr-34-entries"></a>NR-3.4 -- `OpenFile` and `WatchDirectory` take a `Wtf16String` and hold the graft type, with the path as given and the path handed to Win32 as two accessors. *(completed 2026-10-16 12:56:56 -04:00)*

Both hold an `AnchoredPath`; `path()` is the path as given and `win32_path()` the one handed to the
call. A test that moves the process current directory after building a request runs in a child
process, and shows that the request still names the directory it was built in. The crate's own
tests, doctests and README example were rewritten to the new constructors, and the integration tests
build their requests through two helpers in place of the old `prepared`.

### <a id="nr-35-nul"></a>NR-3.5 -- An interior NUL is refused where a request is built, in every entry that takes a path including `ResolveFullPath`, as a fallible constructor. *(completed 2026-10-16 12:56:56 -04:00)*

The item left open whether the constructors become fallible or `perform` reports it; the assistant's
recommendation, fallible constructors, was implemented on the engineer's instruction to implement
`NR-3.4`, and the reasons are in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md). The error is
`PathCaptureError`. The `SAFETY` comment in `full_path.rs` that claimed `Wtf16String` excludes an
interior NUL was wrong and is corrected.

### <a id="nr-36-retire"></a>NR-3.6 -- `prepare`, `PreparedPath`, `PathError`, `PathFailure` and the `path` module are retired, with the documentation that described them. *(completed 2026-10-16 12:56:56 -04:00)*

The module and its tests are deleted, and the lib documentation, the README example and the
references from `full_path.rs`, `close.rs` and `outcome.rs` follow. `D-9` and `D-18` already carried
a status line pointing at `D-19`.

Sabotage entries cover the accessors, the pointer handed to the call and the refusal of a NUL, with a
control among them, and the whole manifest was swept. `win-durable-ioring`'s open kept its empty-path refusal and its verbatim conversion, and the
`request_cost` probe keeps measuring `GetFullPathNameW` through a local stand-in for the retired
function. What this left to do is `NR-3.7` and the release of this crate.

## Moved 2026-10-16 13:22:38 -04:00 -- NR-3.7: the request_cost probe

### <a id="nr-37-probe"></a>NR-3.7 -- The `request_cost` probe in `windows-platform-probes` is rewritten against the new path design, not retired; done with that crate's `M6.1`. *(completed 2026-10-16 13:22:38 -04:00)*

The work and its decision are in
[windows-platform-probes' DESIGN-NOTES.md](../windows-platform-probes/DESIGN-NOTES.md#d-request-cost-follows-the-request-crate),
and the item is archived there as
[M6.1](../windows-platform-probes/COMPLETED-CHECKLIST.md#m61). The probe was first adapted just
enough to build by the commit of `NR-3.4` to `NR-3.6`, as this item said it would be, and then
rewritten.

## Moved 2026-10-16 18:52:01 -04:00 -- NR-5.1: the forms of a path, measured

### <a id="nr-51-forms"></a>NR-5.1 -- The decision for the path-form classifier, from measurements, before the type exists. *(completed 2026-10-16 18:52:01 -04:00)*

The decision and the table of measurements are [D-28](DESIGN-NOTES.md#d-28), with its Tier 2 entry in
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md). They were made with
[examples/path_form_measurements.rs](examples/path_form_measurements.rs), which reproduces them.
The four questions this item named are answered there: the NT prefix `\??\` is read by `CreateFileW` as
an NT path and not normalised, and by `GetFullPathNameW` as a rooted path; forward slashes in a
verbatim prefix make it normalised, so only the exactly-backslash form suppresses normalisation (the
claim of `D-19` rule 6, measured here for the first time); `UNC` is matched without regard to ASCII case;
and `\\.\UNC\` is a UNC form. The run also showed that `\\.\` is normalised, which corrects the reason
given for rule 6, and that `GetFullPathNameW` cannot be the oracle.

### <a id="nr-52-withdrawn"></a>NR-5.2 to NR-5.4 -- withdrawn, not done. *(withdrawn 2026-10-16 19:59:00 -04:00)*

Withdrawn by the engineer on 2026-10-16. They were a public `PathForm`, the graft rules derived from it,
and `win32_path_form()` on the path-taking entries, planned so that durable-ioring could refuse a path
that is evidently remote before the open. The engineer ruled that finding out late is acceptable and
that the check on the opened handle is the only one needed ([DI-13.1
(b)](../durable-ioring/COMPLETED-CHECKLIST.md#di-131), amended), so the one consumer dropped it, and a
new public type in a published crate was not worth what was left. What would reopen them is a consumer
that needs to know a path is remote before it makes any call. The graft rules stay as they are,
including rule 6, whose reason [D-28](DESIGN-NOTES.md#d-28) corrects. The reasons are in
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md).

A working position recorded with them, not a decision: the entries keep taking the `windows-sys` types
and raw access masks they take now, and the typed vocabulary for rights, share mode, disposition and
attributes belongs to durable-ioring's `DI-13.3`, `DI-13.4` and `DI-13.7`, so this crate stays the
policy-free catalogue.
