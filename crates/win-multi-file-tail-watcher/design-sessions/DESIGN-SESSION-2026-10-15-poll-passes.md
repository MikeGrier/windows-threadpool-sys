# Design session, 2026-10-15: the poll as three passes, and a replan

Resulted in the amendment of [TW-D-7](../DESIGN-NOTES.md#tw-d-7) and
[TW-D-9](../DESIGN-NOTES.md#tw-d-9) (wording only), one working position (a registration's last
known length starts at its starting offset), and a replan of M2 to M4 in
[CHECKLIST.md](../CHECKLIST.md). The reasoning is in
[DESIGN-RATIONALE.md](../DESIGN-RATIONALE.md). This file is the discussion, not the decision.

## How it started

After TW-2.3 the questions were about the roster. How is it stored? A `BTreeMap<u64, Entry<T>>`
behind a `Mutex`, keyed by an id that only grows. Should the ids be scrambled one-to-one before
use? No: a B-tree has no use for spread keys, and the ordering is what resume would use. Is the
append-right, remove-anywhere pattern a problem? It costs some slack in the left-hand nodes and
nothing in time. What is the cost of the enumeration? Nothing is built yet; a walk is amortised
constant per entry, and a resume re-descends once per batch because an iterator cannot be held
across the lock.

The engineer asked for the cost guarantees. There are none for time: the probe is a system call on
files the crate does not control, and TW-3.4 (now TW-2.8) exists to measure rather than promise.
What can be stated is the lock hold per batch and the number of probes in a poll.

## The proposal

The engineer, asked how a copy of each entry under the lock would cost (about 24 bytes an entry,
so some 960 KB at 40,000, plus a reference-count bump per observer), proposed passes:

1. Under the lock, copy each entry's id, position/size and file observer.
2. After releasing it, query each observer for its current size.
3. If any changed, take the lock again and, while changed entries remain and the SQ has room, look
   the id up in the table and copy what is needed, including the `T`, into the SQ.
4. If the SQ fills, unlock, ring the doorbell, and wait for the caller's doorbell to continue.
5. When the changed entries run out, including when there were none, put a "done" marker in the SQ
   and ring the doorbell.

## What was settled

- **The lock rule** in TW-D-7 came from the engineer's own Q25 message, which asked for the file
  system, the SQ write and the doorbell to be outside any lock. The engineer's reading: it was never
  meant as "no lock at all", since something unknown to the crate always holds one. It is about the
  roster lock and operations that wait on the outside world; the SQ write never waits.
- **`T`.** The lock is unrelated to `T`, and `T` may be an integer or a boolean, so no `Arc<T>`. A
  reentrancy warning is the right remedy. (Found in passing: `Registration::value` already cloned
  `T` under the lock, so the comment saying the lock was never held across user code was wrong, and
  was corrected.)
- **Chunked allocation.** No copy should be one allocation larger than about 64 KiB. The engineer
  wondered whether the copy could itself be a `BTreeMap`; it would meet the limit, but the copy is
  only appended to and read in order, so a chunked sequence suits it.
- **Observers** are dropped as soon as they have been read, and the buffers are reused.
- **Space in the SQ.** The engineer's preference was to push `n` and then `n` rather than until
  refused, to avoid cloning `T` for a refused write, perhaps with a closure that fills the value.
  The queue's free-space count is a snapshot, so a push can still be refused; the clone wasted is
  one per stop.
- **The done marker** is another entry; it needs room like any.
- **The signal to continue** is the caller's doorbell, rung when it has made some room; "emptied" is
  not required. The crate resets it, retries, and only then arms the wait.
- **Continuing.** Redoing pass 3 from the saved list, not restarting at pass 1; the position kept is
  the last index pushed, and continuing starts after it.

## The replan

The engineer asked for the milestones to be reconsidered with no sacred cows. The stop and the
continuation are part of pass 3, so they belong with the engine, and a synchronous engine can be
tested through every stop deterministically. The thread pool then wraps it. The benchmark moved
earlier, since the three passes are separable and its numbers choose the batch size, the chunk size
and whether the roster's storage stays.

| Before | After |
|---|---|
| M2: `poll` on the thread pool, SQ sized not to fill; M3: stop, resume, churn, benchmark | M2: the engine, synchronous, with the stop point and the benchmark; M3: `poll` on the thread pool, parking and resuming, churn |
| Position in the roster | Index into the saved list of changes |
| "Next roster entry" as the shutdown bound | "Next step": one probe or one report |

Left open and put to the engineer: what a probe that fails does (TW-2.6), and whether a
registration's last known length should start at its starting offset (a working position).
