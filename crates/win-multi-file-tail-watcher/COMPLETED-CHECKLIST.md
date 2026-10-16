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

## Moved 2026-10-15 21:32:55 -04:00 -- TW-2.8.1: the sizes chosen from the captures

### <a id="tw-281"></a>TW-2.8.1 -- The batch sizes, the chunk size and the roster map are chosen from the captures, and sharding the probe is recorded as an available remedy that is not built. *(completed 2026-10-15 21:32:55 -04:00)*

Recorded as [TW-D-14](DESIGN-NOTES.md#tw-d-14), with its reasoning in
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md). Crate-internal (a `chore`): the "Provisional" comments in
`src/watcher.rs` and `src/watcher/engine.rs` now cite the decision.

- **Chosen**: batches of 256 for the snapshot and the report, chunks of at most 64 KiB, the roster
  stays a `BTreeMap`. All three are the values already in the code.
- **Sharding the probe is not built and nothing is queued for it**; the decision says so.
- **Retention was not decided** and moved to TW-2+.2 in [CHECKLIST.md](CHECKLIST.md), because the
  captures have no roster that shrinks.

## Moved 2026-10-15 21:39:56 -04:00 -- TW-2.9: the engine over real files written by children

### <a id="tw-29"></a>TW-2.9 -- The engine is tested over a few hundred real files, with child processes appending through stdout handles their parent opened, against an SQ far smaller than the number of changes. *(completed 2026-10-15 21:39:56 -04:00)*

Crate-internal (a `chore`): `src/watcher/engine/child_tests.rs`. The tests are in the crate rather than
in `tests/` because the engine is not public; an outside test over `poll` belongs to M3, once
TW-3.1 makes one.

- **The files that change are the ones a child writes**, as the engineer asked: each is a
  `cmd.exe` whose stdout is a file the parent opened and handed over, the way a parent gives a child
  its output. Shrinking is one parent-truncated file at the end, as an edge, and not a population.
- **Stepped**: 300 registrations, every twelfth a child's file, the rest unchanging files registered
  at their length, through an SQ of 8. Each pass releases every child by one line, waits until every
  file is exactly that long, and polls; the poll must report exactly the children, in roster order,
  each from the length the pass before left, then poll-done, and stop at least twice. Pass 3 stops
  mid-poll, drops a child's registration and makes another for a new file before continuing: the
  dropped one is skipped, the new one is first reported by the next poll and not by the one in flight,
  and the dropped one is silent from then on. After the children exit a poll is silent, and a
  truncated file is reported as the smaller length.
- **Concurrent**: 200 registrations, twenty of them children appending 400 lines each as fast as they
  can, through an SQ of 4, polled in a loop until the children are gone and a poll after that is
  silent. Whatever the timing, each child's attentions must be one unbroken chain: the first from its
  starting offset, each later one from exactly where the one before ended, every one a change, and
  the last at the full length. Only children are reported and nothing is terminal. The timing is not
  asserted: it was observed, once, to give each child several attentions across some seventy polls.

## Moved 2026-10-15 22:00:37 -04:00 -- TW-3.1: poll on the thread pool

### <a id="tw-31"></a>TW-3.1 -- `Watcher::poll` runs the engine on the thread pool and returns, one poll at a time, with the doorbell rung after each full batch and at the end. *(completed 2026-10-15 22:00:37 -04:00)*

A `feat` of `win-multi-file-tail-watcher`: `Watcher::poll`, `PollError`, and a dependency on
windows-threadpool-sys. The mechanism is `src/watcher/poll.rs`; the reasoning is under
[TW-D-5](DESIGN-NOTES.md#tw-d-5).

- **`poll` returns at once.** It takes the gate and submits a `ThreadpoolWork` made by the first
  call, so a watcher never polled makes none. The callback runs `Engine::run` against its own view
  of the shared state, which owns no work object, and the engine lives in the callback behind a
  mutex. `T` must be `Send + 'static`.
- **The gate is three states**, idle, running and finishing. A poll is in flight from the call until
  its poll-done entry is written, a poll stopped on a full SQ included. `finishing` exists so that a
  caller who has read poll-done may poll again at once, which a flag cleared after the write could
  not give: a `poll` that finds `finishing` waits for the write's outcome, which is a few
  instructions away. A refused poll-done goes back to running.
- **The doorbell is rung with the roster lock released**: after each full batch, and once more when
  the run ends, whether it stopped on a full SQ or wrote poll-done. The ring after poll-done follows
  the gate opening.
- **Not here**: nothing resumes a poll that stopped on a full SQ, so such a poll stays in flight and
  every later `poll` is refused; the wait that resumes it is TW-3.2. The same holds for shutdown
  (M4): dropping the watcher drains the work object and waits for a running callback, and a parked
  poll has returned.
- **`expect(dead_code)`** on the items the poll now uses is removed.

Tested: a poll of an empty watcher; each change then poll-done, in order; a second poll reporting
only what changed since; a poll stopped on a full queue still in flight and refusing twice; eight
threads polling a stopped poll with exactly one accepted; 300 rounds of read-poll-done-then-poll
with none refused; 2,000 rounds of a reader polling in the instant poll-done is visible, which
fails every run when the gate's finishing step is removed; a poll-done the queue refuses leaving the
poll in flight; the gate making a `begin` that finds it finishing wait for the write's outcome, in
both outcomes; a watcher polled and dropped at once, twenty times; a registration outliving a
polled watcher; the error messages. For the ringing, with a counting stand-in for the doorbell:
three rings for 600 changes (two full batches and the end), one for nothing to report, and one for
a stop.

## Moved 2026-10-15 22:27:52 -04:00 -- TW-3.2: parking on a full SQ and resuming

### <a id="tw-32"></a>TW-3.2 -- A poll that stops on a full SQ is parked on a thread-pool wait on the resume doorbell, and the resumed callback resets it, retries from the kept index, and only then arms again. *(completed 2026-10-15 22:27:52 -04:00)*

A `feat` of `win-multi-file-tail-watcher`: a poll stopped on a full SQ now continues when the caller
rings `ring_resume`. The mechanism is in `src/watcher/poll.rs`; the decision text is under
[TW-D-6](DESIGN-NOTES.md#tw-d-6), and the working position it was is removed.

- **Two thread-pool objects, one engine.** The first `poll` makes a work item and a wait; both
  callbacks run the same engine behind one lock, so a resume and the start of the next poll cannot
  overlap. The wait owns a duplicate of the resume doorbell's event, which is the same event, and
  the work item's callback holds the wait through an `Arc` so there is no cycle with the callbacks.
- **Armed only while stopped.** The work callback arms the wait when its run stops; the wait callback
  re-arms only when its retry stops again. A poll that completes leaves nothing armed, so a ring
  between polls starts nothing.
- **Order inside the resumed callback**: reset the doorbell, then retry, then arm. A ring that lands
  after the reset finds the event rung when the wait is armed, and wakes it at once; clearing after
  the retry would erase it.
- **That order is not verified by a sabotage.** Retrying before the reset gives the same entries in
  every test, because the ring it would lose has to land in the few instructions between a refused
  write and the reset, and no test can put a caller's ring there on demand. It rests on the
  argument above and on the checklist item's own wording.
- **The watcher's drop drains the work item and then the wait**, so nothing arms the wait after it
  is drained. A watcher dropped while its poll is parked, or as one resumes, neither hangs nor
  crashes.

Tested: a parked poll continuing on each ring through two stops to poll-done and then a new poll
being accepted; 600 changes through a queue of 64, each reported once and in order, with the caller
ringing as the callback runs; the resumed callback leaving the resume doorbell clear; a ring made
before the poll parks not lost; a ring between polls starting nothing; the watcher dropped while
parked and as a poll resumes, twenty times each.

## Moved 2026-10-15 22:42:29 -04:00 -- TW-3.3: registering and dropping during a poll

### <a id="tw-33"></a>TW-3.3 -- Registering and dropping registrations during a running or parked poll is tested on the thread pool and needed no change: neither waits for the poll, attentions already written keep their value, and a registration made mid-poll is seen by the next poll. *(completed 2026-10-15 22:42:29 -04:00)*

Crate-internal (a `chore`): tests only, in `src/watcher/poll/tests.rs`. The behaviour is what
[TW-D-8](DESIGN-NOTES.md#tw-d-8) says and what the earlier items built, so there was no code to
change; this item is the proof on the real thread-pool poll.

- **Why nothing changed.** A registration's `Drop` takes the roster lock for one removal and waits
  for nothing else; the poll holds that lock one batch at a time and never across a probe, and an
  attention holds a clone of `T`, not a reference to the registration. The batch bound on a
  registration call is the property the pause tests in the engine and snapshot modules already
  establish (the lock free in each pause, and a registration made from inside a probe's read), not a
  new one.
- **Not measured as a latency.** A bound on how long a call waits would have to be tight enough to
  tell one batch from a whole pass, which a unit-test budget cannot give; it is the lock-free-pause
  tests that carry it.

Tested, with the poll parked on a full queue so the moment is exact: a registration dropped before
its attention is written is skipped when the poll resumes; one dropped after its attention is
written leaves that attention in the queue with its value, counted through an `Arc`; a registration
made while parked is not in this poll and is in the next; fifty registrations and drops on another
thread complete while the poll is parked; and four threads registering and dropping 250 times each
while polls run through a queue of four, with no failure and only the original registrations left.

## Moved 2026-10-15 22:51:22 -04:00 -- TW-3.4: polls over real files on the thread pool

### <a id="tw-34"></a>TW-3.4 -- Polls over real files that child processes write run through a queue of two slots in tests of the public API, reporting every change across parks and resumes while registrations come and go. *(completed 2026-10-15 22:51:22 -04:00)*

Crate-internal (a `chore`): `tests/poll_real_writers.rs`, driving only `Watcher`, `poll`, the two
doorbells and `drain`. This closes M3.

- **Why two slots.** Two is the smallest queue the crate accepts, and with it any poll of more than
  one change parks, because poll-done needs a slot of its own. That makes the number of parks and
  resumes as large as it can be, which is where a lost ring or a lost change would show. It is a
  stress setting and not a recommended one; the in-crate tests use larger queues for the ordinary
  shapes.
- **Stepped**: 300 registrations, every fifth a child's file (sixty children, each appending through
  a stdout handle its parent opened), the rest unchanging files registered at their length. Each pass
  releases every child by one line, waits for exact lengths, and polls to poll-done; the poll must
  report exactly the live children, in roster order, each from where the pass before ended, and take
  more than twenty rounds. On the third pass the poll is parked after its first round: a second
  `poll` is refused, a child's registration is dropped before its attention is written, and a file
  is registered; the dropped one is skipped, and the new one is reported by the next poll only. After
  the children exit a poll is silent, and then nothing runs between polls: a ring of the resume
  doorbell starts no poll and the doorbell stays quiet for 300 ms.
- **Concurrent**: 150 registrations, thirty of them children appending 300 lines each as fast as
  they can, while another thread registers and drops a file every 200 microseconds, all through two
  slots. Each child's attentions must be one unbroken chain from its starting offset to its full
  length; the registrations made mid-poll are reported as the file's length from zero when they are
  reported at all; nothing else is reported and nothing is terminal; and only the original
  registrations remain.
- **Checked against two mutants** by hand: a poll that stops without arming its wait, and a resumed
  callback that does not re-arm. Both fail these tests, which wait up to thirty seconds for a doorbell
  that never comes. The first run of that check hung, because a failing concurrent test left its
  churn thread running and the scope waited for it; the thread now stops when the test unwinds.

## Moved 2026-10-15 23:14:12 -04:00 -- TW-4.1: the shutdown request

### <a id="tw-41"></a>TW-4.1 -- `Watcher::shutdown` sets a stop flag and returns, may be repeated, and `poll` is refused with `PollError::Shutdown` after it. *(completed 2026-10-15 23:14:12 -04:00)*

A `feat` of `win-multi-file-tail-watcher`: `Watcher::shutdown` and `PollError::Shutdown` (the error
type was already `#[non_exhaustive]`, so the variant is not a break). The decision text is under
[TW-D-9](DESIGN-NOTES.md#tw-d-9).

- **The request** stores a flag in the shared state and returns. It never blocks, so it can be made
  from a callback for the doorbell, and it may be repeated. It drains nothing.
- **`poll` after it is refused with `Shutdown`**, ahead of `InFlight`, so a parked poll is reported
  as shut down and not as in flight. The flag is read before the gate is taken and again after, so a
  request that lands between the two is refused as well.
- **Everything else still works**: registering, dropping registrations, and draining.
- **Not here**: stopping a poll that is already running or parked is TW-4.2, and what `Drop`
  waits for is TW-4.3. After this item a request does not yet stop a poll in flight.
- **The second look at the flag is not verified by a sabotage.** It closes a race between the first
  look and the gate that no test can land on demand, and removing it leaves every test green.

Tested: a poll after the request refused, with the request repeated before and after; a parked poll
refused as shut down and not as in flight; registering, dropping and draining still working; a
shutdown from another thread ending a loop of polls after some had run; and the error's text and
source.

## Moved 2026-10-15 23:25:15 -04:00 -- TW-4.2: stopping the poll

### <a id="tw-42"></a>TW-4.2 -- A shutdown stops a running or parked poll at its next step (a batch of the copy, a file probed, a change reported) and the poll goes on to poll-done, which is always delivered. *(completed 2026-10-15 23:25:15 -04:00)*

Crate-internal (a `chore`): behaviour of the already-public `shutdown`, in `src/watcher.rs`,
`src/watcher/probe.rs`, `src/watcher/engine.rs` and `src/watcher/poll.rs`. The decision text is under
[TW-D-9](DESIGN-NOTES.md#tw-d-9).

- **Three stop points, each reading the flag**: the copy before each batch, the probe before each
  file, and the report before each change. A stopped copy or probe leaves nothing to report; a
  stopped report abandons the rest of the list, and the poll goes on to its ordinary poll-done. The
  registrations whose changes were abandoned keep their last known length.
- **Poll-done is always delivered, and the resume wait is armed as it is after any stop.** The
  checklist item says a queued resume callback "returns at once on seeing the flag". The first
  reading made one attempt and gave up if the queue refused poll-done; the engineer corrected it
  ("the poll-done should always be delivered"). So the resumed callback runs the engine, which with
  the flag set goes straight to poll-done, and arms the wait again when the queue refuses it: the
  poll stays parked until the caller drains and rings, and then writes it. A shutdown makes the
  callback quick, not absent.
- **Not here**: what `Drop` waits for is TW-4.3. A poll parked on a full queue whose caller never
  rings again simply stays parked until the watcher is dropped.

Tested: a shutdown before the poll starts probing nothing and writing only poll-done; one made from
inside the first probe stopping at the next file with nothing reported and no last known length
advanced; one made after the first report batch writing that batch and poll-done and leaving the
rest unreported; one made after the first batch of the copy ending it there; a parked poll stopped
by a shutdown writing only poll-done when the caller drains and rings; and a poll-done refused after
a shutdown, through three rings with the queue still full, being written once the caller drains and
rings, with `poll` still refused as shut down.

## Moved 2026-10-16 00:06:59 -04:00 -- TW-4.3: dropping the watcher

### <a id="tw-43"></a>TW-4.3 -- Dropping a `Watcher` stops it as `shutdown` does, waits for a running poll to reach its next step and return, and never waits for poll-done or for a parked poll. *(completed 2026-10-16 00:06:59 -04:00)*

Crate-internal (a `chore`): `impl Drop for Watcher` in `src/watcher.rs`, with the drain that was
already in `Poller`'s drop since TW-3.2. The decision text is under
[TW-D-9](DESIGN-NOTES.md#tw-d-9).

- **`Drop` sets the stop flag first**, whether or not `shutdown` was called, so a poll that is
  running stops at its next step and the wait that follows is bounded by one step. The fields then
  drop: the work item is drained, then the resume wait, which waits for any running callback and
  disarms, so when `Drop` returns no callback of the crate runs again.
- **What it does not wait for**: a poll parked on a full queue has returned from its callback, so
  there is nothing to wait for; and poll-done is never waited for, written or read. A watcher may be
  dropped with entries still in the queue.
- **The callbacks' own view of the shared state also runs this `Drop`**, when the work item drops
  its closure. That only sets a flag that is already set, and happens only inside the poller's drop.
- **The wait for a running poll is shown, not only argued.** A test holds a poll inside `T::clone`,
  which runs under the roster lock in the report, drops the watcher on another thread, and sees the
  drop not return until the clone does.

Tested: the flag set on drop with and without a prior shutdown; a watcher with a parked poll
returning from drop at once, and then no callback of the crate running when its resume doorbell is
rung and its own doorbell staying quiet, both through duplicated events; and the drop waiting for a
running poll and returning when it does. The earlier tests that poll and drop at once, and that drop
while parked or as a poll resumes, cover the same drain.

## Moved 2026-10-16 00:16:06 -04:00 -- TW-4.4: shutdown from every state

### <a id="tw-44"></a>TW-4.4 -- Shutdown requested from the caller's own callback on a thread-pool wait is tested from a parked poll, a poll inside its probe and a poll held inside the report, with repeated requests, drops bounded by a step, and nothing running after `Drop`. *(completed 2026-10-16 00:16:06 -04:00)*

Crate-internal (a `chore`): `tests/shutdown_every_state.rs`, public API only. This closes M4.

- **The caller's callback** is a thread-pool wait on a duplicate of the watcher's doorbell, from
  windows-threadpool-sys, whose callback calls `shutdown()` and reports how long the call took. The
  doorbell is also rung by hand on a duplicate where the poll would not ring it itself.
- **Parked on a full queue**: the queue fills, the doorbell rings, the callback requests; the request
  returns inside a second, the caller drains and rings, and poll-done is delivered, with `poll` then
  refused as shut down.
- **Inside the probe**: twenty thousand files registered at their length, so a poll is all probe and
  reports nothing; the doorbell is rung by hand just after `poll`, and the poll reaches poll-done in
  less than half the time a whole pass took, measured on the same watcher first. A probe that
  ignored the flag fails this, which the earlier tests of the probe could not show because the copy
  stops first.
- **Held inside the report**: a value whose `Clone` blocks holds the poll inside the report with the
  roster lock held. The request from the callback returns without waiting for it; once released the
  poll writes the one change it had in hand and abandons the rest, and delivers poll-done.
- **Repeated requests**: five from the callback, four hundred from four other threads, and more
  after; `poll` stays refused.
- **Drop bounded by a step**: during a poll over twenty thousand files, with and without a prior
  request, the drop returns in well under half a pass. Measured while writing it, a pass was about
  seventy milliseconds and a drop under two hundred microseconds. The comparison is skipped on a
  machine where a pass is under ten milliseconds, since there it shows nothing.
- **Nothing runs after `Drop`**: with a poll parked, the watcher is dropped, then its resume doorbell
  is rung through a duplicate; its own doorbell, also duplicated, stays quiet.
- **Checked against three mutants** by hand: `Drop` not setting the flag, the probe ignoring it, and
  the report ignoring it. Each fails these tests.

## Moved 2026-10-16 00:35:34 -04:00 -- TW-5.1: the endpoint held as a backend holds it

### <a id="tw-51"></a>TW-5.1 -- `HeldFile` holds its endpoint as an `EndpointHandle`, consumed from the `UnassociatedEndpoint` when the file is opened, and lends it to the crate for the reading engine. *(completed 2026-10-16 00:35:34 -04:00)*

Crate-internal (a `chore`): `src/held_file.rs`. The decision is
[TW-D-10](DESIGN-NOTES.md#tw-d-10), amended for the reads.

- **The open is unchanged**: `UnassociatedEndpoint::open` with the same arguments, then
  `into_endpoint_handle`, the route windows-overlapped-io-sys gives an endpoint into a backend.
  Observers, probes, `len` and `open_at` are as before, and no public signature changed.
- **`endpoint_handle`** lends the `EndpointHandle` to the crate, under `expect(dead_code)` until the
  reading engine (TW-5.5) submits through it.
- **What makes the change hold is the type**: an `EndpointHandle` is not `Clone` and can only be made
  by consuming an endpoint, so there is no sabotage entry for it. The four entries that target the
  open line were re-run against the new code and behaved as declared.

Tested: the handle the `EndpointHandle` lends is the handle every observer shares, and a second hold
of the same path has another; an observer still reads the length after the `HeldFile` drops.

## Moved 2026-10-16 12:15:51 -04:00 -- TW-5.2: the buffer pool and the chunk

### <a id="tw-52"></a>TW-5.2 -- A pool of owned buffers of one size, all allocated and zeroed when it is made, lends each as a buffer a ring read can take and a chunk of the bytes read, both given back on drop, with a doorbell for a starved reader. *(completed 2026-10-16 12:15:51 -04:00)*

Crate-internal (a `chore`): `src/pool.rs`, and a dependency on io-buffers for the buffer contract the
ring takes. Everything is `pub(crate)` under `expect(dead_code)` until the reading engine (TW-5.5)
uses it; the chunk becomes public with the client's entries (TW-5.3). The decision text is under
[TW-D-15](DESIGN-NOTES.md#tw-d-15).

- **`BufferPool::new(buffer_len, count)`** checks both before allocating anything: a buffer of 1 to
  `u32::MAX` bytes, since a ring read's length is a `u32`, and at least one buffer. Then it reserves
  and zeroes every buffer, so the memory is fallible rather than an abort, and every byte is
  initialized as the buffer contract requires.
- **`take` lends a `PoolBuffer`**, which implements `IoBuf` and `IoBufMut` over a boxed slice whose
  address moving the value does not change, so a ring read can take it by ownership. Dropping it
  gives the buffer back, made into a chunk or not.
- **`into_chunk(len)` keeps the first `len` bytes as what was read**; the `Chunk` dereferences to
  exactly those, so the stale bytes of a reused buffer are never visible. A length past the buffer
  panics rather than clipping.
- **The doorbell** is manual-reset and the pool's own: a refused take arms it, and the first buffer
  given back after that rings it once, outside the pool's lock. Arming and the free list share one
  lock, so a buffer given back between a waiter's reset and its next take is simply taken.

Tested: dimensions out of range refused before allocating, and a count that cannot be reserved
refused as an allocation error; every buffer available at its length and zeroed; takes up to the
count and then refused; a buffer given back whether or not it became a chunk; buffers distinct and
not overlapping; a returned buffer lent again at the same address; the address stable across moves
and equal for reading and writing; a chunk showing only the bytes read, also from a reused buffer; an
empty chunk and a full one; a chunk longer than its buffer panicking; the doorbell rung only by the
first return after a refusal, and again after a reset and a second refusal; a chunk dropped on
another thread giving its buffer back and ringing; lent buffers outliving the pool; eight threads
taking and giving back with no buffer lost or gained; the errors' text and sources; and the types
being `Send` and `Sync`. Ten sabotage entries, nine caught and one control that survives, each run
on its own.

## Moved 2026-10-16 12:33:29 -04:00 -- TW-5.3: the reading layer's SQ and its entries

### <a id="tw-53"></a>TW-5.3 -- The reading layer's SQ, made by the same function and capacity rule as the watcher's, carries data entries whose range is computed from their chunk, shrink, terminal and poll-done entries, with its own doorbell pair. *(completed 2026-10-16 12:33:29 -04:00)*

Crate-internal (a `chore`): `src/read_sq.rs`, and `src/sq.rs` made generic over the entry type. The
decision text is under [TW-D-15](DESIGN-NOTES.md#tw-d-15).

- **One queue, one capacity rule.** `sq.rs`'s writer, reader, push error and constructor are now
  generic over the entry (`QueueWriter<E>`, `QueueReader<E>`, `PushFailure<E>`, `queue_of`), and the
  watcher's names are aliases of them, so no caller changed. `ReadSq::new` is made by the same
  `queue_of`, so the two SQs cannot disagree about which capacities are valid; a test compares their
  errors capacity by capacity.
- **`ReadEntry<T>`**: `Data`, `Shrink`, `Terminal` (the watcher's own type, passed through) and
  `PollDone`, `#[non_exhaustive]`. **`Data` computes its range from the chunk**, so a range that
  does not cover exactly the bytes delivered cannot be made; its fields are private with accessors and
  `into_parts`. A range that would end past `u64::MAX` panics rather than wraps.
- **`ReadSq<T>`** holds the queue and the two manual-reset doorbells, with the watcher's discipline
  (TW-D-6): `writer`, `push`, `drain`, `capacity`, and ring and reset on each doorbell. A refused
  entry comes back whole, its chunk still lent; entries left in a dropped SQ give their buffers back.
- **Not public yet.** Everything is `pub(crate)` under `expect(dead_code)` until the reading layer
  hands the SQ to a client, and the names (`ReadEntry`, `Data`, `Shrink`, `ReadSq`, `ReadDrain`) are
  working names until the layer's own is chosen. The `#[non_exhaustive]` matters only once they are
  public, so its compile-fail doctest is left for then.
- **A sabotage note: an abort, not a memory fault.** "A full queue is reported as closed" now ends
  the suite with a deliberate abort rather than exit 101: since TW-3.1 a poll on the thread pool
  reaches the engine's `unreachable!` for a closed SQ, and a panic cannot unwind out of the
  `extern "system"` callback, so Rust aborts ("panic in a function that cannot unwind"). Rust aborts
  on Windows through `__fastfail`, which the OS reports for every cause as `0xC0000409`
  (`STATUS_STACK_BUFFER_OVERRUN`, a name left over from the stack-cookie check); no buffer overran.
  The panic message preceding the abort in the run's stderr is what tells the two apart. The SQ's own
  tests still fail on the mutant deterministically; the abort only ends the run first.

Tested: capacities refused and accepted by the watcher's rule, with equal errors; a data entry's range
from its chunk, empty, and ending at `u64::MAX`, and one that would overflow panicking; `into_parts`;
every kind drained in the order written, and the SQ empty after; a full SQ handing back an entry with
its chunk still lent; chunks given back when their entries drop, including entries left in a dropped
SQ; the two doorbells separate, starting unrung, staying rung for every waiter until reset; four
writers on their own threads each landing every entry in order; the SQ and entries crossing threads.
Four new sabotage entries, and the one entry whose target line moved was retargeted; each run on its
own, with the other three that target `sq.rs`.

## Moved 2026-10-16 12:46:12 -04:00 -- TW-5.4: planning reads from the watcher's entries

### <a id="tw-54"></a>TW-5.4 -- Each of the watcher's entries is planned without a ring: growth as reads of at most one buffer each, computed as they are taken, a shrink as a shrink entry, a terminal entry passed through, and poll-done kept for the engine. *(completed 2026-10-16 12:46:12 -04:00)*

Crate-internal (a `chore`): `src/read_plan.rs`. The decision text is under
[TW-D-15](DESIGN-NOTES.md#tw-d-15).

- **`plan(entry, buffer_len) -> Plan<T>`** matches the watcher's `SqEntry` with no wildcard arm, so a
  kind the watcher adds does not compile here until it is planned. An attention whose length grew is
  `Plan::Reads`; one that shrank is `Plan::Deliver(ReadEntry::Shrink)`; a terminal entry is
  `Plan::Deliver(ReadEntry::Terminal)` unchanged; poll-done is `Plan::PollDone`, its own variant
  because the engine must hold it until every read of its poll has been delivered. An attention with
  an unchanged length, which the watcher does not write, plans no reads.
- **`buffer_len` is a `NonZeroU32`**, so a zero-length read cannot be planned, and a read's length
  fits the ring's `u32` by type. The engine takes it from the pool, whose lengths are 1 to
  `u32::MAX` (TW-5.2).
- **`Reads<T>` is computed as it is taken**: `front` names the next read and leaves it planned, and
  the iterator's `next` takes it. That is the shape the engine needs, since an empty pool or a full
  ring SQ stops it holding a read it has not submitted. Any range costs the same to plan, up to
  `0..u64::MAX`.

Tested: growth shorter than, equal to, and one byte past a buffer; whole numbers of buffers; reads
starting at the length last reported; one-byte buffers; every growth of 1 to 40 bytes against every
buffer of 1 to 17 bytes from five starting lengths, contiguous, in order, none longer than a buffer,
and the expected count; growth past `u32::MAX` split at the largest buffer; growth ending at
`u64::MAX`; `front` not taking, `remaining` shrinking per read, a finished plan staying finished; the
largest range planned without walking it; shrinks including to nothing and by one byte; a terminal
entry unchanged; poll-done. Six sabotage entries, each run on its own: five caught, and an equivalent
spelling of the empty check as a control that survives. The truncation mutant was at first caught
only by a hang, the test collecting billions of short reads; that test now takes at most four reads,
and the mutant fails it outright.

## Moved 2026-10-16 14:06:07 -04:00 -- TW-5.5: the reading engine, driven synchronously against a ring

### <a id="tw-55"></a>TW-5.5 -- The reading engine, driven synchronously against a ring, drains the watcher's SQ, reads each attention's announced bytes into pool buffers with the handle addressed raw and an observer in the sidecar, delivers a data entry per read as it completes, and stops without waiting on an empty pool, a full ring or a full client SQ and continues from where it stopped. *(completed 2026-10-16 14:06:07 -04:00)*

Crate-internal (a `chore`): `src/read_engine.rs`, a dependency on windows-ioring-sys, and small
changes to the pool, the watcher and the diagnostics. The decision text is under
[TW-D-15](DESIGN-NOTES.md#tw-d-15); the choices made while building it, and what was set aside, are
in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md).

- **`ReadEngine::run`** does everything that can be done without waiting and returns why it stopped:
  `Idle`, `PoolEmpty`, `RingFull` or `ClientFull`. It resets the resume and pool doorbells first and
  rings the client's once at the end if it wrote.
- **Files are registered through the engine**, whose watcher is over a `Tracked<T>` built from the
  open file by the new crate-internal `Watcher::register_with`, so each attention carries what its
  reads need and no public type changed.
- **A lent buffer has a window** (`PoolBuffer::limit_to`), because a ring read fills the whole buffer
  it is given and a read must not pass what was announced.
- **A new `Diagnostic::ReadFailed`** reports a read that failed; a read of nothing, or end of file,
  delivers nothing and is no failure.
- **Not exercised:** the submit a failed submit left queued, since `SubmitIoRing` cannot be made to
  fail on demand; said so at the definition and in the design notes.
- **TW-6.3 added**, for whether a failed read should be an entry on the client's SQ.

Tested: whole growths in one and many buffers, the starting offset, several files, no change, a
second poll, two polls queued together with poll-done after every read of its own, shrink, terminal,
a short read, a read limited to the announced range, a read past the end, a registration dropped
after its attention, the means to read taken from the open file, each way a read settles, an empty
pool and a returned buffer, a full client SQ and its refused entry, the reads held to the ring's
queue, a ring SQ that fills, the doorbells, creation errors, and the poll groups and the split as
pure functions. Twenty-two sabotage entries, each run on its own and caught; the one that first
survived (a batch not counting its own pushes) needed a test that starts a batch part way through the
queue, since the engine pops every available completion before it submits more. An existing pool
entry was retargeted at the window.

## Moved 2026-10-16 14:25:50 -04:00 -- TW-5.6: the reading engine over real files

### <a id="tw-56"></a>TW-5.6 -- The reading engine is tested over real files that child processes write through inherited handles, through a pool of a few buffers and a client SQ of two: every byte delivered once, chunks the client keeps stopping the reads and dropping them resuming them, and registrations dropped with reads in flight. *(completed 2026-10-16 14:25:50 -04:00)*

Tests only (a `test`): `src/read_engine/child_tests.rs`. They are in the crate and not in
`tests/` because the engine is not public, as for the poll engine's
[child_tests.rs](src/watcher/engine/child_tests.rs); the child-process helpers they share moved to
`test_support.rs`, and that module now uses them from there.

- **Reassembly:** two hundred children each append thirty lines to a file of their own, polled while
  they write, read through four buffers of eight bytes (shorter than a line) and a client SQ of two.
  Each file's pieces meet by range with no gap or overlap from offset 0, are exactly the bytes the
  file holds, and there is one poll-done for each poll.
- **Chunks kept:** with four buffers, the client keeps each chunk it takes. The engine stops with
  `PoolEmpty` and nothing in flight, and stays stopped; dropping the chunks gives the buffers back
  and the reads go on to the file's end.
- **Registrations dropped in flight:** reads are submitted for sixty files with the pool empty and
  reads in flight, every registration is dropped, and the reads still complete with every byte of
  every file delivered once.

No new sabotage entries: what these tests would catch is already caught by the unit tests, and they
add the real files, the real child writers and the polls running beside them.

Milestone M5 is complete.

## Moved 2026-10-16 15:22:15 -04:00 -- TW-6.1: the reading layer on the thread pool

### <a id="tw-61"></a>TW-6.1 -- The reading engine runs on the thread pool with no thread of the caller's: the watcher's doorbell, a completed read, a chunk returned to an empty pool and the client's resume doorbell each drive it through a wait of their own, which stays armed and idle between polls. *(completed 2026-10-16 15:22:15 -04:00)*

Crate-internal (a `chore`): `src/read_layer.rs`, with changes to the engine and the ring dependency's
`threadpool` feature. The decision text is under [TW-D-15](DESIGN-NOTES.md#tw-d-15); why the waits
stay armed is in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md).

- **`ReadLayer`** owns the engine behind a lock and three thread-pool waits on duplicates of the
  watcher's, the pool's and the client's resume doorbells, plus the ring's `CompletionWake`. Each
  callback runs the engine and, for a wait, watches again. It offers `register`, `poll`, the client's
  SQ and `take_failure`.
- **The engine's run** now resets the watcher's doorbell too, so all three are reset before it
  looks, and rings the watcher's resume doorbell after taking entries from its SQ, which is what
  continues a poll parked on a full SQ. The client SQ is shared (`Arc`) so a client drains it without
  the engine's lock.
- **The waits stay armed.** The checklist said nothing armed between polls; the ring's completion
  event cannot be detached, and a wait re-arming only itself avoids a cross-thread arm that draining
  does not exclude. An armed wait on an event nobody rings wakes no thread (TW-D-5).
- **Drop** stops and drains each wait, then the ring's wake, then releases the engine. The bound with
  reads in flight is TW-7.1's.

Tested: a poll read and delivered with no call after it, nothing to read, several files, a client that
stops draining and the layer stalling until it rings back (the run count does not move while stalled),
a returned chunk continuing the reads with no ring from the client, a poll parked on a watcher SQ of
two going on as the layer drains it, nothing running between polls, a second poll reading only new
bytes, registration ids, dropping an idle layer, and the setup run. Two engine tests cover the
watcher's doorbell reset and its resume ring. Nine sabotage entries, each run on its own and the full
sweep run once: eight caught and one control that survives. One entry first survived because the
test's client was ringing the resume doorbell itself; the client now rings nothing.

Still open from here: TW-6.2 tests this on children's files through a small pool and a client SQ of
two.

## Moved 2026-10-16 15:55:03 -04:00 -- TW-6.2: the layer on the thread pool over children's files

### <a id="tw-62"></a>TW-6.2 -- The layer is tested on the thread pool over files that child processes write, through a pool of four small buffers and a client SQ of two: polls repeated while the children write, every byte delivered once, a stall on kept chunks that dropping them continues, and a drop that releases every file. *(completed 2026-10-16 15:55:03 -04:00)*

Tests only (a `test`): `src/read_layer/child_tests.rs`, in the crate and not in `tests/` because the layer
is not public. The helpers of the layer's unit tests are shared with it, and the child-process
helpers come from `test_support.rs`.

- **Polls while children write:** a hundred and fifty children each append thirty lines to a file of
  their own, and `ReadLayer::poll` is called again as each poll's poll-done arrives, through a
  watcher SQ of eight (so polls park and are continued), four buffers of eight bytes and a client SQ
  of two. Each file's pieces meet by range from offset 0 and equal the file's bytes, each file is read
  to its length, and there is one poll-done for each poll.
- **Chunks kept:** the client keeps a chunk for each of four buffers; the layer stops and its run
  count does not move; dropping the chunks is all that continues it, with no ring from the client.
- **Drop:** after its polls the layer is dropped with its registrations, and every file can then be
  deleted and made again, so no handle of the layer's is left open.

No new sabotage entries: the layer's unit tests already catch each mutant that matters here, and
these add real files, real writers and many polls, which is what catches a stall or a lost
wake-up under load. They were run three times in a row against flakiness.

M6 is not complete: [TW-6.3](CHECKLIST.md) is a decision awaiting the engineer.

## Moved 2026-10-16 16:23:46 -04:00 -- TW-6.3: a failed read is classified as a failed probe is

### <a id="tw-63"></a>TW-6.3 -- A failed read is classified as a failed probe is: a terminal one is delivered on the client's SQ as a terminal entry and reaches the diagnostic sink, a transient one reaches the sink alone. *(completed 2026-10-16 16:23:46 -04:00)*

Crate-internal (a `feat` in effect, but nothing public changed): `src/read_engine.rs` and
`src/probe_failure.rs`. The decision is the engineer's, 2026-10-16, and is in
[TW-D-15](DESIGN-NOTES.md#tw-d-15): terminal errors must appear in the SQ, transient ones go to the
diagnostic channel only for now.

- **One predicate.** `classify_code` is `classify` for a code already known, which a read's is (the
  ring reports it inside an `HRESULT`), and `classify` now calls it, so a read and a probe cannot
  disagree about which codes are terminal.
- **A terminal read failure** becomes the layer's terminal entry: the Win32 code, the registration's
  value, and `last_known` the offset the read began at. It is delivered before its poll's poll-done,
  and the sink is told as well.
- **A transient one** reaches the sink alone, as before.
- **The possibility the engineer named** -- a recurring transient failure entering the SQ, though
  not its first occurrence -- is recorded in the design notes and schedules nothing: an actual case
  is wanted first.

Tested: each listed terminal code delivers the entry before poll-done with the sink told and the
buffer returned; a transient code and a code-less error deliver nothing; `classify_code` agrees with
`classify` over the terminal set and neighbours, and a missing code is transient. Two sabotage
entries, each run on its own and caught.

Milestone M6 is complete.

## Moved 2026-10-16 16:53:21 -04:00 -- TW-7.1: shutdown and drop with reads in flight

### <a id="tw-71"></a>TW-7.1 -- Shutdown and `Drop` with reads in flight: shutdown is a non-blocking, repeatable request that stops reads being planned or submitted, reads in flight complete and are delivered, every poll-done is still delivered, and `Drop` waits only for the reads already in flight. *(completed 2026-10-16 16:53:21 -04:00)*

Crate-internal (a `chore`): `src/read_engine.rs`, `src/read_layer.rs` and a stop handle in
`src/watcher.rs`. The decisions are under [TW-D-15](DESIGN-NOTES.md#tw-d-15), restating
[TW-D-9](DESIGN-NOTES.md#tw-d-9) for reads; the alternatives are in
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md).

- **`ReadLayer::shutdown`** sets the engine's stop flag and the watcher's stop request, both without
  the engine's lock, and wakes the engine. After a stop the engine abandons what was planned and
  plans no read from an attention that arrives; every other entry goes on, so each poll-done is
  still delivered after the reads of its own poll.
- **Reads are completed, not cancelled**: the ring is `Owned`, which has no raw cancel.
- **`Drop` requests shutdown first**, then drains the waits and releases the engine, whose ring runs
  down the reads already in flight. The working position that said "at most one read to finish or be
  cancelled" is replaced by that.

Tested: after a stop four reads in flight are delivered with poll-done and no more are submitted;
attentions after a stop are skipped while terminal entries and poll-done go through; a stop leaves
the watcher working; shutdown refuses polls, repeats, and returns while another thread holds the
engine's lock; shutdown while stalled on kept chunks delivers poll-done once they return with fewer
bytes than the files hold; a layer dropped with reads in flight returns and every file can then be
deleted. Four sabotage entries, each run on its own and the full sweep once: three caught and one
control that survives, since the order of a drop's shutdown request against a callback running during
its drain cannot be arranged by a test.

M7 is not complete: TW-7.2 is its tests.
