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
