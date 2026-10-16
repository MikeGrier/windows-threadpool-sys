# Completed checklist: win-multi-file-tail-watcher

Append-only record of finished [CHECKLIST.md](CHECKLIST.md) items.

## Moved 2026-10-15 13:59:42 -04:00 -- TW-1.1: the crate

### <a id="tw-11"></a>TW-1.1 -- The crate exists: a Windows-only workspace member registered for release and publication, with its README as the crate documentation and an undocumented `unsafe` refused by the build. *(completed 2026-10-15 13:59:42 -04:00)*

Registered as `win-time-sys` was: a workspace member, in release-please's configuration and manifest
(at `0.0.0`, so the first release is `0.1.0`), and in the publish workflow's tag filter,
manual-dispatch choices and workspace-sibling list. `README.md` replaces `COMPONENT.md` and is the
crate documentation. Dependencies are the three the item named: [wtf-string](../wtf-string/),
[windows-overlapped-io-sys](../windows-overlapped-io-sys/) with `default-features = false` and `fs`
only (the feature that holds the observer's length query, so none of its other optional
dependencies come along), and `windows-sys` with no Win32 feature, since each arrives with the call
that needs it. Each path dependency carries its version, as the repository's dependency-version
check requires.

`lib.rs` denies `missing_docs`, `unsafe_op_in_unsafe_fn` and
`clippy::undocumented_unsafe_blocks`. Sabotage: a public function with no documentation, and an
`unsafe` block with no safety comment, each failed `cargo clippy`; the edit was reverted. The crate
has no code and so no unit tests. `tools/check-workflow-refs.ps1`,
`.github/release/check-dependency-versions.cjs` and `tools/check-publishable.ps1` all pass with it
registered; the last two need the release scripts' locked dependencies (`npm ci` in
`.github/release`).

## Moved 2026-10-15 14:18:13 -04:00 -- TW-1.2: opening a file

### <a id="tw-12"></a>TW-1.2 -- `HeldFile::open` opens a file by `Wtf16String` path as a read-only, overlapped endpoint, and hands out observers. *(completed 2026-10-15 14:18:13 -04:00)*

`HeldFile` keeps a windows-overlapped-io-sys `UnassociatedEndpoint` and gives out its observers
([TW-D-10](DESIGN-NOTES.md#tw-d-10)); the name is provisional, recorded as a working position
rather than a decision. `open` converts the path with `Wtf16Str::to_os_string`, which keeps unpaired
surrogates, and opens it through the endpoint's own `open` with read access only. That call always
sets `FILE_FLAG_OVERLAPPED` and leaves the sharing mode at read, write and delete, so neither is
re-stated here; the tests below show the sharing from outside. Nothing in this crate checks the
path first: a failed open is `std`'s `io::Error` carrying the Win32 code.

Measured, and each a test: a missing file is `ERROR_FILE_NOT_FOUND`; a missing directory and an empty
path are `ERROR_PATH_NOT_FOUND`; a directory is `ERROR_ACCESS_DENIED`; a file another handle holds with no
sharing is `ERROR_SHARING_VIOLATION`; a path containing a NUL is refused by `std` as `InvalidInput`
before any call, with no Win32 code. Another handle can still write to and delete a held file, and the
hold survives the delete. Read-only is shown from the other side: a new open that denies write sharing
succeeds beside the hold, which a hold with write access would make a sharing violation. Names with
spaces, accents, CJK, an astral character and an unpaired surrogate open, as does a path past
`MAX_PATH` in its verbatim form. Two holds of one path are independent, and every observer of one hold
shares its handle and keeps it open after the `HeldFile` drops.

That the handle is overlapped is not tested here, since the only handle to it is the unsafe borrow an
observer offers, whose contract forbids associating it. The sabotage manifest
([sabotage.json](sabotage.json)) is this crate's first: opening with write access, converting the
path lossily, and losing the Win32 code are each caught, and a differently spelled conversion
survives as the control.

## Moved 2026-10-15 14:24:34 -04:00 -- TW-1.3: reading the length

### <a id="tw-13"></a>TW-1.3 -- `HeldFile::len` reads the file's current length through an observer of the held handle. *(completed 2026-10-15 14:24:34 -04:00)*

`len` takes a new observer and asks it for `file_len`, which is `standard_info`'s end-of-file
([TW-D-2](DESIGN-NOTES.md#tw-d-2)). The path is not consulted. The method is named `len` and carries
no `is_empty`, since the query can fail.

Tested, each against a real file: lengths of 0, 1, 5, 4096, 4097 and 100000 bytes; a length that
tracks twelve writes by a writer that stays open; a shrink by another handle, repeated to the same
size and to zero; a file of 2^32 + 5 bytes; the length after the file is deleted while held; the
length after a rename with a different file recreated at the old path, which reads the held file
and not the new one; an observer that outlives the hold and sees a later append; two holds of one
path; fifty steady reads. The sabotage manifest catches reading the allocation size instead, a
32-bit cut and a cached first read, and a length read through `standard_info` survives as the control.

Not tested here: a read on the thread pool, and a file on a remote or non-NTFS volume. The
directory entry's lag behind the held handle is recorded under [TW-D-2](DESIGN-NOTES.md#tw-d-2) and is
not re-measured.

## Moved 2026-10-15 14:45:25 -04:00 -- TW-1.4: the starting offset

### <a id="tw-14"></a>TW-1.4 -- `HeldFile::open_at` validates a starting offset against the length read at open, and refuses one beyond it with its own error. *(completed 2026-10-15 14:45:25 -04:00)*

`open_at(path, offset)` opens as `open` does, reads the length through the new handle, and returns
`OpenError::OffsetBeyondLength { offset, len }` when `offset > len` ([TW-D-3](DESIGN-NOTES.md#tw-d-3)).
An offset equal to the length is accepted, so a parent that stopped at the end of a file can
restart there. `OpenError::Io` carries every other failure, with its Win32 code, including a
failed length read. The accepted offset is kept and read back with `start_offset()`; `open` is
`open_at` with offset 0 and returns `io::Result` as before. Whether the offset is the watcher's
first "last known length" is not decided here; it is the TW-2.x items' to settle.

Tested: offsets 0, 1, 5, 9 and 10 on a 10-byte file are accepted and 11, 12, 4096, 2^32,
2^32 + 3 and `u64::MAX` are refused with both numbers; an empty file takes only 0; the check is made
against the length at open, so a later shrink leaves an accepted offset in place and a fresh open
at it is refused; a file of 2^32 + 5 bytes accepts its length and one less and refuses one more; a
missing file is `Io` with `ERROR_FILE_NOT_FOUND` whatever the offset, and `Io` has a source where
the offset error has none.

The first sweep found a hole: with only small refused offsets, comparing as 32 bits survived,
because none had low 32 bits within the length. Offsets 2^32 and 2^32 + 3 close it. The manifest
now also catches `>=`, a dropped check and a forgotten offset, and a reversed comparison survives
as the control.

## Moved 2026-10-15 14:51:44 -04:00 -- TW-1.5 and M1: one file, real writers

M1 ("Register one file and read its length through a held handle") is complete: TW-1.1 to TW-1.4
above and TW-1.5 below. Its deliverable -- a file written to by a child process is opened by path
and its growing length is read through the held handle -- is the first test below.

### <a id="tw-15"></a>TW-1.5 -- Integration tests: one file, real writers. *(completed 2026-10-15 14:51:44 -04:00)*

[tests/real_writers.rs](tests/real_writers.rs), five tests across real process and file-system
boundaries:

- **A child process's output, tracked a write at a time.** `cmd.exe` is started with its stdout
  redirected to the file; each line the test writes to its stdin releases one 13-byte append, and
  the held length is waited for at exactly that value, so an overshoot fails at once. The child's
  `/q` is needed: with command echo on, the prompt and the command text land in the file too.
- **A shrink by another handle** is the smaller length, to 4096, 100, 0 and 7.
- **A missing path** is `ERROR_FILE_NOT_FOUND` (2), a missing directory `ERROR_PATH_NOT_FOUND` (3),
  and `open_at` reports the same code as `OpenError::Io`.
- **An offset past the length** is `OffsetBeyondLength` with both numbers, and one equal to the
  length is accepted.
- **Another process can append to and delete the file while it is held**, and the length is still
  read after the delete.

The shrink is by a second handle, not by the child: a redirected stdout keeps its own file
position, so a child writing after a truncation would extend the file with zeros, which is a
different case. [sabotage-integration.json](sabotage-integration.json) is the set these tests catch
without the unit tests: a cached length, the allocation size, a lost Win32 code, `>=` and a dropped
offset check; a length read through `standard_info` survives as the control.

## Moved 2026-10-15 15:05:48 -04:00 -- TW-2.1: the watcher and its roster

### <a id="tw-21"></a>TW-2.1 -- `Watcher<T: Clone>` registers files into a roster and returns an RAII `Registration` whose `Drop` unregisters. *(completed 2026-10-15 15:05:48 -04:00)*

`Watcher::register(path, offset, value)` opens the file as `HeldFile::open_at` does, moves `value`
into a roster entry beside the held file, and returns a `Registration`. Dropping the registration
removes the entry; it waits for nothing and may happen after the `Watcher` has dropped, since the
registration shares the roster ([TW-D-8](DESIGN-NOTES.md#tw-d-8)). There is no uniqueness and no
lookup by name ([TW-D-3](DESIGN-NOTES.md#tw-d-3)). On error `value` is dropped with it, so a caller
who wants it keeps a clone.

Two choices the item left open, recorded here because nothing else says so:

- **The roster is a `BTreeMap` keyed by an id that only grows.** Its order is the order of
  registration and a position in it survives removals, which is what a poll that stops and resumes
  ([TW-D-7](DESIGN-NOTES.md#tw-d-7)) will want. TW-2.4 may change it; nothing outside the module sees it.
- **The roster lock is held for a map operation only.** The entry is dropped after the guard is
  released, so closing the file and dropping `T` do not happen under it.

`Registration::value()` (a clone of `T`) and `start_offset()` read an entry back; they are what the
roster's fields are read through until a poll reads them. The last known length that
[TW-D-4](DESIGN-NOTES.md#tw-d-4) attaches to a registration is not stored yet: it is TW-2.4's to
define, along with what a registration starts from.

Tested: one registration, two registrations of one path, 200 unregistered in
several orders, 400 registered from eight threads at once, a registration dropped from another
thread, one that outlives its watcher, a failed open and a refused offset that each register nothing
and drop the value, the file held while registered and released on drop (an exclusive open fails with
a sharing violation and then succeeds), and `Send + Sync`. The sabotage manifest gains a registration
that stays registered, a repeated id, a dropped starting offset and a count read from the id counter;
a drop-order variant that nothing can observe survives as the control.

The shared test helpers (`TempDir`, `wtf`, `code`) moved to `src/test_support.rs` so the watcher's
tests and the held file's share one copy.

## Moved 2026-10-15 15:41:26 -04:00 -- TW-2.2: the SQ

### <a id="tw-22"></a>TW-2.2 -- The watcher owns a bounded submission queue of attention and poll-done entries, set at `Watcher::new` and drained by the caller. *(completed 2026-10-15 15:41:26 -04:00)*

`Watcher::new(sq_capacity)` creates the queue ([TW-D-6](DESIGN-NOTES.md#tw-d-6)), so it lives exactly
as long as the watcher; `Watcher::drain()` is the caller's side. **This changes TW-2.1's
constructor**, which took no argument, and removes its `Default`: a default capacity would be a
number nothing here has chosen.

- **Entries.** `SqEntry::Attention(Attention { value, last_known, current })` and
  `SqEntry::PollDone`, both public and both comparable. The attention carries a clone of the
  registration's `T`, so it does not depend on the registration surviving
  ([TW-D-4](DESIGN-NOTES.md#tw-d-4)).
- **Capacity** is a power of two of at least two, which is windows-waitable-queues' `slotwise_mpsc`
  rule; the decision in TW-D-6 adopts it as ours. A bad capacity is `SqCapacityError` carrying the
  request and the smallest and largest valid values. The dependency's own error type does not appear
  in this crate's interface.
- **Writing is crate-internal.** `SqWriter` is a clone-per-thread handle over the queue's producer
  and `SqPushError::{Full, Closed}` returns the entry. The watcher keeps one writer behind a lock
  that is taken only to clone it, so no writer waits on it to write. The poll (TW-2.4) is the first
  real caller; until then the writer is reached from tests only, and the three items carry an
  `expect(dead_code)` that stops compiling when the poll uses them, which is how they get removed.
- **Draining** is an iterator that takes entries oldest first until there are none, holds the
  reader's lock for as long as it lives, and does not hold up writers.

Not decided here: how the caller is told there is something to drain (the doorbells are TW-2.3), and
what a full queue does to a poll (M3).

Tested: capacities 2 to 1024 accepted and 0, 1, 3, 5, 100 and 1000 refused with the bounds; order
preserved across kinds and across 1000 entries through a four-slot queue; a full queue refusing with
the entry intact and accepting again after one read; exactly its capacity held; a writer with no
reader getting `Closed` and its entry back; four cloned writers from four threads losing nothing and
keeping each one's order; four writers retrying on `Full` against a draining caller; an entry
outliving the registration it names; and a drain that stops when there is nothing and resumes when
there is. The sabotage manifest gains a queue of double the asked capacity, `Full` reported as
`Closed`, a capacity error that misnames the request, and a drain that yields nothing; the last is
caught by the concurrent test running past the sweep's hang bound, not by an assertion, so it is the
weakest of the four. A different spelling of the empty read survives as the control.

## Moved 2026-10-15 15:58:04 -04:00 -- TW-2.3: the doorbells

### <a id="tw-23"></a>TW-2.3 -- The watcher owns two manual-reset doorbell events, one each way, each exposed as a waitable handle. *(completed 2026-10-15 15:58:04 -04:00)*

`Watcher::doorbell()` is the event the crate rings and the caller waits on; `resume_doorbell()` is
the one the caller rings, with `ring_resume()`, and the crate will wait on. Both are `BorrowedHandle`s
tied to the watcher and live exactly as long as it does. The caller can clear the first with
`reset_doorbell()`; the crate's own ring and reset (`ring_doorbell`, `reset_resume`) are crate-internal
until the poll and its resume use them, under the same `expect(dead_code)` as the SQ's writer.

`Doorbell` is a small crate-internal wrapper over `CreateEventW` (manual-reset, initially clear) with
`ring`, `reset` and `handle`. `windows-waitable-queues` has its own doorbell but does not export it, so this one is written
here. The reset-before-read ordering is recorded in [TW-D-6](DESIGN-NOTES.md#tw-d-6) and on
`Watcher::doorbell`.

**This changes `Watcher::new` again.** It now returns `CreateError::{Capacity, Io}` where it returned
the capacity error alone, because creating an event can fail. `SqCapacityError` is still the payload of
the first.

Not decided: whether `drain` should reset the doorbell itself. The ordering rule makes that the safe
place, and a caller that drops a drain early would be left waiting for entries still queued. TW-D-6
says it is open.

Tested, against real events: a new doorbell is clear; a ring is not consumed by a wait and stays until
a reset; two rings take one reset; a reset of a clear one does nothing; two hundred ring and reset
rounds; two doorbells are two events; a wait begun before the ring is released by it; one ring releases
four waiters at once, which an auto-reset event would not; a wait on a clear one times out; the watcher's
two are distinct and clear at creation, each rung and reset in its own direction, not shared between
two watchers, and a thread blocked on the caller's doorbell is woken by the crate's ring. The sabotage
manifest gains an auto-reset event, one that starts rung, a ring that does nothing, a reset that does
nothing, and a resume handle that is the other event; a differently written null check survives as the
control.

The concurrent SQ test now gives its writers the same deadline as its reader. The sweep had been
catching "a drain yields nothing" only by running past its hang bound, because a stalled reader left
the scope waiting on writers that could never get room; it is now caught by the test's own failure.
The TW-2.2 entry's remark that this was the weakest catch no longer holds.

## Moved 2026-10-15 17:21:39 -04:00 -- TW-2.4: the chunked buffer

### <a id="tw-24"></a>TW-2.4 -- `Chunked<T>`, an append-only sequence in chunks of at most 64 KiB that keeps its chunks across a clear, for the poll's copies. *(completed 2026-10-15 17:21:39 -04:00)*

`src/chunked.rs`, crate-internal. The interface is `push`, `get`, `len`, `iter`, `clear`, `truncate`
and `retain`; the first three passes of the poll ([TW-D-7](DESIGN-NOTES.md#tw-d-7)) will use it, which
is why the module carries an `expect(dead_code)` that stops compiling when TW-2.5 does.

- **Chunks are allocated whole and never grow.** Items per chunk is `64 KiB / size_of::<T>()`, so a
  24-byte poll entry is 2,730 to a chunk. A pushed item never moves, and a push past a chunk's end
  allocates the next chunk and nothing else. An item of zero size, or larger than a chunk, does not
  compile.
- **Clearing keeps the chunks**, and so does a truncate: `clear` and `truncate` drop the items and
  leave every allocation, so a buffer filled once is refilled with no allocation. Held addresses are
  the test of it.
- **`retain` compacts in place by swapping**, so it needs neither `Default` nor `Clone` of the item,
  keeps the accepted items in their order, and drops each rejected one exactly once.

Not decided, and not scheduled by itself: whether the buffers should give chunks back after an
unusually large poll. A buffer never shrinks, so the high-water mark stays allocated. TW-2.8 now
lists it among the things the benchmark informs, since the answer depends on how large and how
frequent the large polls are.

Tested: empty, one chunk, several chunks, and the indexes either side of each chunk edge; items per
chunk for six item sizes including one exactly a chunk and one larger than half of one; no chunk
allocation over 64 KiB for each of those sizes; 40,000 poll entries taking 15 chunks; an item not
moving when later chunks are pushed; clear and truncate keeping addresses over twenty refills; drops
counted with a reference-counted item through clear, truncate and retain; retain keeping everything,
nothing, the ends, runs across chunk edges and every third item; retain on strings; and
`Send + Sync`. The sabotage manifest gains a megabyte chunk, chunks that grow by doubling, a clear
that frees its chunks, a truncate that forgets the length or cuts one late, a retain that does not
move, and a chunk index of the wrong width; a retain that also swaps an item with itself survives as
the control.

## Moved 2026-10-15 17:35:41 -04:00 -- TW-2.5: last known length and the snapshot pass

### <a id="tw-25"></a>TW-2.5 -- Each roster entry holds a last known length starting at its starting offset, and the snapshot pass copies id, last known length and an observer into a chunked buffer one batch per lock. *(completed 2026-10-15 17:35:41 -04:00)*

Crate-internal; nothing public changes, which is why this is a `chore`. Both new pieces are in
`src/watcher.rs`, and the snapshot's tests are in `src/watcher/snapshot_tests.rs`.

- **`last_known`.** `register` sets it from the held file's starting offset. This is the working
  position recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md#working-positions-not-decisions), built that
  way and still not put to the engineer. Nothing writes it afterwards yet; reporting does, in TW-2.7.
- **`snapshot`** appends one `SnapshotItem { id, last_known, observer }` (24 bytes) per registration
  to a `Chunked` buffer, in registration order. It takes the roster lock once per batch, found by id
  range after the last id copied, so the lock is held for at most a batch of map work and an entry is
  never copied twice. The batch is 256, provisional: TW-2.8 chooses it. The observer's reference
  count is taken, and the chunk allocated when one is needed, under the lock.
- **The pause between batches** is a closure that runs with the lock released. The engine passes
  nothing now; it is where the stop check goes (M4), and it is how the tests change the roster in
  the middle of a pass.
- **Roster changes during a pass.** A registration dropped before the pass reaches it is skipped; one
  dropped after it was copied still has a working observer, which keeps the file open; one made
  while the pass is still short of the end is seen, and one made after it is the next pass's.

Tested: starting offsets of 0, 1, 4, 9 and 10 as last known lengths; an empty roster; forty
registrations copied in order with each observer reading its own file's length; each registration's
own last known length; the same result at batch sizes from 1 to past the roster's size; the lock
taken the right number of times and free during every pause, checked with `try_lock`; each of the
roster changes above; a snapshot that appends and changes nothing; one path registered twice as two
items with their own handles; an observer that outlives its registration and its watcher; the
default batch over 600 registrations; and a snapshot run 100 times against another thread
registering and dropping.

The sabotage manifest gains a last known length of zero at registration and in the copy, a cursor
that skips the next registration, a batch that is not honoured, and the lock held across the pause;
the cursor written as a `match` survives as the control. The last of those is caught by a hang, not
an assertion: with the lock held, the tests that register from inside the pause deadlock before the
`try_lock` check can fail, so the sweep reports it past its hang bound.

## Moved 2026-10-15 19:03:30 -04:00 -- TW-2.6.1: classifying a probe failure

### <a id="tw-261"></a>TW-2.6.1 -- One predicate classifies a probe failure as transient or terminal from its Win32 code; the terminal list is `ERROR_INVALID_HANDLE` and `ERROR_FILE_INVALID`. *(completed 2026-10-15 19:03:30 -04:00)*

`src/probe_failure.rs`, crate-internal, so a `chore`. It carries the module-level
`expect(dead_code)` that stops compiling when the probe pass (TW-2.6) and the diagnostic sink
(TW-2.6.2) use it. ([TW-D-13](DESIGN-NOTES.md#tw-d-13))

- **`classify(&io::Error) -> FailureClass`** answers `Terminal` for a code on `TERMINAL_CODES` and
  `Transient` for everything else, including an error that carries no code. Every site that needs the
  distinction asks it; there is one list to add to as the system produces codes nobody listed.
- **`win32_code(&io::Error) -> Option<u32>`** is the code as a `u32`, which is what the terminal SQ
  entry and the diagnostic event will carry. Only a plain Win32 code is read. An error made from a
  message carries none, and a code in `HRESULT` form is not recognised as the Win32 code inside it:
  the probe's errors come from `GetFileInformationByHandleEx` and are plain, so reading an `HRESULT`
  would be a guess.
- **Sign.** `std` hands a Win32 code back as an `i32`, so a code above `i32::MAX` arrives negative;
  `win32_code` casts the bit pattern back, and a test reads `u32::MAX` and `0x8007_0006` through it.

Tested: each listed code is terminal and none is listed twice; the codes either side of every listed
one are transient; nine codes that may pass (access denied, not found, general failure, invalid
parameter, network name deleted, not ready, operation aborted, sharing violation, virtual circuit
disconnected); zero and the extremes; errors with no code; an `HRESULT` that wraps
`ERROR_INVALID_HANDLE`; and the code read back unchanged. The sabotage manifest gains a list missing
each of its two codes, an inverted test, an error with no code made terminal, a match by nearness,
and a negative code read as its magnitude; the list searched with an explicit loop survives as the
control.

Not known: whether the nine transient codes above are all transient in practice. They are
transient because they are not listed, which is the default, not because anything was observed.

## Moved 2026-10-15 19:32:00 -04:00 -- TW-2.6.2: the diagnostic sink

### <a id="tw-262"></a>TW-2.6.2 -- The watcher reports typed diagnostic events to a sink the caller supplies through `Watcher::with_diagnostics`, in the shape of durable-ioring's `DiagnosticSink`. *(completed 2026-10-15 19:32:00 -04:00)*

`src/diagnostics.rs`. Public: `DiagnosticSink`, `Diagnostic`, `DiagnosticKind`, `NoDiagnostics`,
`Watcher::with_diagnostics` and `Registration::id`. ([TW-D-13](DESIGN-NOTES.md#tw-d-13))

- **The shape is durable-ioring's** ([DI-D-51](../durable-ioring/DESIGN-NOTES.md#di-d-51)): typed
  events, never text; a `wants(kind)` question before an event is built; a default that wants
  nothing. The types are this crate's own, because durable-ioring's enums are its own. Display on the
  event is for a text sink.
- **The first event is `Diagnostic::ProbeFailed { registration, win32_code }`**, for a transient
  failure; both enums are `#[non_exhaustive]`. The code is an `Option<u32>`, since an error need not
  carry one.
- **`Watcher::with_diagnostics(sq_capacity, sink)`** supplies the sink, and `Watcher::new` is the
  same with `NoDiagnostics`. A second constructor and not a setup struct, because nothing else
  needs to be supplied yet; the batch size may change that when TW-2.8 chooses it.
- **A sink's contract**, stated on the trait: called from whatever thread observed the event,
  possibly with a lock of the watcher's held, so it returns promptly and never calls back into the
  watcher.
- **`Registration::id`** was added so that an event's registration means something to the caller. It
  is the roster's own number: unique, handed out in order, never reused, and the one the snapshot
  copies.
- **The watcher reaches its sink through `diagnose`**, which builds the event only for a sink that
  wants the kind. It is crate-internal and carries an `expect(dead_code)` that stops compiling when
  the probe pass (TW-2.6) uses it.
- **Not mirrored:** durable-ioring's `StderrDiagnostics` and `FanOutDiagnostics`. Shared machinery
  would bring them; see TW-2+.1.

Tested: the event's kind, text and equality; `NoDiagnostics` wanting nothing; a watcher made with `new`
building no event; a sink that wants nothing being asked and given nothing, with no event built; a
sink that wants the kind receiving each event, in order, with the event built once each; one sink
shared by two watchers; eight threads reporting at once; a bad capacity refused by both
constructors; a registration's id unique, ordered and never reused, and equal to the one the
snapshot copies; and `Send + Sync`. The sabotage manifest gains a sink that is not asked, an event
built before the question, an event built and dropped, a sink replaced by the default, an id of zero,
and a text that drops the code; the question written as an early return survives as the control.

## Moved 2026-10-15 19:38:28 -04:00 -- TW-2.6.3: the terminal SQ entry

### <a id="tw-263"></a>TW-2.6.3 -- `SqEntry::Terminal` is a third entry kind carrying a clone of `T`, the Win32 code and the last length reported, and `SqEntry` is `#[non_exhaustive]`. *(completed 2026-10-15 19:38:28 -04:00)*

`src/sq.rs`. Public: `SqEntry::Terminal` and `Terminal { value, win32_code, last_known }`.
([TW-D-13](DESIGN-NOTES.md#tw-d-13), [TW-D-6](DESIGN-NOTES.md#tw-d-6))

- **`Terminal<T>`** has a clone of the registration's `T`, the Win32 code as a `u32` (not an
  `Option`: the predicate only calls a failure terminal when it carries a listed code), and the
  length last reported, so a caller knows where it stood. Like the others it is `Clone` and
  comparable, and it owns its value independently of the registration.
- **Nothing writes one yet.** The report pass (TW-2.7) does, under the same `expect(dead_code)` as
  the writer, and marks the roster entry terminal once it is written (TW-2.6.4).
- **`SqEntry` is `#[non_exhaustive]`**, so this kind and any later one is not a break for a caller.
  Two documentation examples pin it: a match with a wildcard arm compiles, and one without does not
  (`compile_fail,E0004`). Only code outside the crate can see the attribute, which is what a
  doctest is.

Tested: the fields; equality by every field and by kind, and `Clone`; the three kinds coming out in
the order they went in; a full queue and a closed one each handing a terminal entry back intact; one
drained by the caller among the others; and the value owned independently of the registration.

Sabotage: a new manifest, [sabotage-doctests.json](sabotage-doctests.json), because only a doctest
sees the attribute. It has `SqEntry` losing `#[non_exhaustive]` (caught) and the variants declared
in another order (survives, as the control). Its `testArgs` has no `--lib`: the harness cannot
pre-build doctests, so that manifest runs the whole suite per entry and is kept to what only a
doctest can catch. The first version of the control did not survive: moving a variant without its
documentation left the other undocumented, and the crate denies missing documentation.

## Moved 2026-10-15 20:12:57 -04:00 -- TW-2.6.4: the terminal state on a roster entry

### <a id="tw-264"></a>TW-2.6.4 -- A roster entry has a terminal state, holding the Win32 code, which the snapshot skips and which `mark_terminal` sets once. *(completed 2026-10-15 20:12:57 -04:00)*

Crate-internal (a `chore`), in `src/watcher.rs`. ([TW-D-13](DESIGN-NOTES.md#tw-d-13))

- **The state is `terminal: Option<u32>` on the entry**: the Win32 code that made it terminal, so the
  reason is kept with the fact. A terminal entry stays registered, with its `T`, until its
  registration is dropped.
- **`Watcher::mark_terminal(id, code)`** sets it, returns `true` when it did, and `false` when the
  registration is not there or is already terminal, so the first code stays and a second report
  cannot overwrite it. The report pass (TW-2.7) calls it once the terminal entry is written, which
  is why it is crate-internal and carries an `expect(dead_code)`. It takes the roster lock itself;
  if TW-2.7 needs the lookup, the write and the mark under one hold of the lock it will restructure
  this, and nothing else depends on its shape.
- **The snapshot skips a terminal entry, and counts it as looked at.** The first version counted only
  what it copied, so a batch holding terminal entries looked short and ended the pass with live
  registrations unvisited. The count is now of entries scanned, skipped or not, and tests put runs
  of terminal entries at the start, between live ones and at every batch size.

Tested: a new registration is not terminal; marking sets the code once and the first stays;
marking a dropped or unknown id does nothing; a snapshot without the terminal ones and with the
others in order; runs of terminal entries at the start and between live ones for every batch size
up to past the roster's; a roster that is wholly terminal giving an empty snapshot; a terminal
registration staying registered, keeping its value and last known length, and leaving every later
snapshot; an entry marked between batches skipped if not yet reached and kept if already copied; and
eight threads marking the same fifty entries marking each exactly once.

The sabotage manifest gains a terminal entry snapshotted, a pass that counts only what it copied
(the defect above), a second mark overwriting, a mark that records no code, and a mark of an absent
registration reporting success; the count written out survives as the control. The entry for the
lock held across the pause is updated for the renamed variable.

## Moved 2026-10-15 20:23:37 -04:00 -- TW-2.6: the probe pass

### <a id="tw-26"></a>TW-2.6 -- The probe pass reads each snapshotted observer's length with no lock held, drops each observer as read, and keeps the changes and terminal failures in a second chunked buffer. *(completed 2026-10-15 20:23:37 -04:00)*

Crate-internal (a `chore`): `src/watcher/probe.rs`, with `Watcher::probe`, and `Chunked::drain_each`
in `src/chunked.rs`. ([TW-D-7](DESIGN-NOTES.md#tw-d-7), [TW-D-13](DESIGN-NOTES.md#tw-d-13))

- **`probe(&mut snapshot, &mut changed)`** reads each snapshotted observer with `file_len`, in the
  snapshot's order, and appends a `Change { id, last_known, outcome }` for each registration whose
  length is not the last reported one, in either direction. An unchanged one adds nothing. It holds no
  roster lock, and a test registers a new file from inside the read to show it.
- **`drain_each` hands each item over and drops it as the call returns**, so an observer is dropped as
  soon as it has been read and the file is not held open by the snapshot a moment longer than the
  probe needs. The snapshot is empty afterwards and keeps its chunks. A test shows the file open
  to an exclusive open before the probe and free after it.
- **`Outcome` is `Changed { current }` or `Terminal { win32_code }`**, 32 bytes in all, so a chunk of
  the changed list holds 2,048.
- **A failed probe is classified by the predicate of TW-2.6.1.** A transient failure is reported to the
  sink as `Diagnostic::ProbeFailed { registration, win32_code }` and nothing is kept: the registration
  is probed again next pass. A terminal one is kept with its code for the report pass. A failure
  that the predicate calls terminal but that carries no code cannot occur, and would be treated as
  transient.
- **The probe marks nothing.** `mark_terminal` and `last_known` are the report pass's, once an entry
  is written (TW-2.7); a test checks that a probe leaves both alone.
- **The read is a parameter of an inner function** (`probe_with`), so that a test can make a probe
  fail with a code. Nothing the tests can do to a real file produces `ERROR_INVALID_HANDLE`.

Tested: nothing for a file at its last reported length; a file registered at zero reported from zero
and an empty file at zero not; growth after the snapshot, read as the length at the probe; a shrink;
a mixed set keeping only the changed ones in order; the snapshot emptied and the roster unchanged;
observers released; nothing read for an empty snapshot and each observer read once; no roster lock
held; a transient failure reported once to the sink with the registration and code, not kept, and
probed again next pass; one with no code reported without one; a terminal failure kept with its code
and not a diagnostic, for `ERROR_INVALID_HANDLE` and `ERROR_FILE_INVALID`; changes, terminal
failures and transient ones interleaved in id order; a sink that wants nothing asked once per
failure and given nothing; and 2,200 changes crossing a chunk of the changed list. For
`drain_each`: items in order across chunks, the buffer empty with its chunks in place, nothing
called on an empty buffer, each item dropped as its call returns, a refill that allocates nothing,
and `clear` restoring a buffer whose callback panicked.

The sabotage manifest gains an unchanged length reported, only growth reported, a change that reports
the current length as the last known one, a terminal failure not told apart, one that loses its code,
a transient failure not reported, and a snapshot left full; the observer dropped by name survives as
the control.

## Moved 2026-10-15 20:59:44 -04:00 -- TW-2.7: the report pass and the stop point

### <a id="tw-27"></a>TW-2.7 -- The report pass writes the changed list to the SQ a batch at a time under the roster lock, a refused write stops the pass without waiting, and the engine continues from the kept index. *(completed 2026-10-15 20:59:44 -04:00)*

Crate-internal (a `chore`): `src/watcher/engine.rs`. ([TW-D-4](DESIGN-NOTES.md#tw-d-4),
[TW-D-6](DESIGN-NOTES.md#tw-d-6), [TW-D-7](DESIGN-NOTES.md#tw-d-7),
[TW-D-13](DESIGN-NOTES.md#tw-d-13))

- **`Engine` owns the two buffers, the SQ writer and the kept index**, and takes the watcher per
  call. `run` starts a poll when none is in flight (snapshot, then probe) and then reports; a poll
  that stopped is continued by calling `run` again, which skips the copy and the probe and resumes at
  the kept index. It returns `RunOutcome::Complete` or `Stopped`.
- **Per batch, under one hold of the roster lock**: look the id up (a dropped registration, or one
  already terminal, is skipped), clone `T`, write the attention or terminal entry, and on success make
  the reported length the entry's last known one or mark it terminal. The lookup, write and mark are
  one hold, so `mark_terminal` is now a method of the roster `Entry`; the `Watcher` method of the
  same name is test-only.
- **A refused write stops the pass without waiting.** The index stays at that change, the entry is
  neither advanced nor marked, and the `T` cloned for it is dropped after the lock is released. A
  terminal entry that is refused is therefore written again on the next run.
- **Poll-done follows the last change and needs room like any entry**; when there is none the pass
  stops with the index at the end.
- **A pause between batches runs with the lock released** (a parameter of the inner function, so a
  test can look at the lock and drop a registration in it).
- **`SqPushError::Closed` is `unreachable!`**: the watcher holds the SQ's reader.
- **Not done here**: ringing the doorbell is the thread-pool poll's, after the lock is released
  (TW-3.1). The report batch is provisional until TW-2.8.

Tested: empty and unchanged watchers; attentions in registration order then poll-done; no repeats
and the last known length current; growth reported from the length last reported; a shrink; a
capacity-2 SQ stopping and continuing across runs; poll-done waiting for room; a pass probed before
a stop and a later poll seeing the growth since; a registration dropped or added while stopped; the
refused clone dropped; no observer held while stopped; terminal entries written once, marked, and
not probed again; one made terminal after the probe not reported; a refused terminal entry staying
unmarked and retried; a transient failure going to the sink with the registration probed again;
every batch size from 1 to past the list; the lock free in each pause; a drop in a pause; 600
changes through a capacity of 64; and new polls after completion. The sabotage manifest retargets the
three entries that named the old `Watcher::mark_terminal` and adds eight for the engine and a control.

## Moved 2026-10-15 21:09:16 -04:00 -- TW-2.8: timing the passes

### <a id="tw-28"></a>TW-2.8 -- Timings of each pass against roster sizes from 1,000 to 50,000 are captured by ignored tests, with the roster map measured alone; no constant is changed by it. *(completed 2026-10-15 21:09:16 -04:00)*

Crate-internal (a `chore`): `src/watcher/engine/bench.rs`, and the capture
[bench-captures/2026-10-15.txt](bench-captures/2026-10-15.txt), three consecutive runs with the host
and base commit in its header. The figures live there and are not restated here.

- **Run** with `cargo test --release -p win-multi-file-tail-watcher --lib bench -- --ignored
  --nocapture --test-threads=1`. The tests are ignored, so the ordinary run does not pay for them,
  and they sit inside the crate because the passes are private.
- **Populations** are 1,000, 5,000, 20,000 (the stated top end, in [DESIGN-NOTES.md](DESIGN-NOTES.md)) and 50,000 as headroom. At most 4,096 distinct files are
  created and registered round-robin, so a large roster is not a large directory.
- **Measured per size**: registering; the snapshot with the span of each batch's lock hold; the
  probe with every file a change; the report with every change written and the span of each batch's
  hold; a whole poll in which nothing changed; a whole poll through an SQ of 64, drained at each
  stop; a random replace-one churn; and dropping everything in random order. The permutation is a
  fixed LCG, so a run is repeatable.
- **The roster map alone**, with an entry-sized value and no file behind it: append, batched range
  walk and random remove, so the map's share of the figures above can be told from the file's.
- **What it does not show**: all files share one host, one disk and one run of three; growth is not
  appended between polls, so no figure is for a mixed changed/unchanged population; the probe is
  single-threaded, so nothing here bears on [TW-D-5](DESIGN-NOTES.md#tw-d-5)'s thread count except
  the single-thread cost per file.
- **No constant was changed.** `SNAPSHOT_BATCH`, `REPORT_BATCH`, the chunk size, the roster map and
  the buffers' retention are decisions the engineer makes from the capture; each stays provisional
  until then.
