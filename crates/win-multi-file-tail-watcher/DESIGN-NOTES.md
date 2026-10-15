# Design notes: win-multi-file-tail-watcher (Tier 1)

_Written 2026-10-14 19:03 -04:00 - anchored to commit `57471c8f`._

Current decisions for this crate. How they were reached, and what else was considered, is in
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md); the full discussion is the
[inception design session](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md), and
the question-by-question worksheet it produced is [OPEN-QUESTIONS.md](OPEN-QUESTIONS.md).

Repository-level decision this crate sits under: the `win-` prefix, in the workspace
[DESIGN-NOTES.md](../../DESIGN-NOTES.md#new-crates-take-the-win-prefix).

## Decision index

| ID | Decision |
|---|---|
| <a id="tw-d-1"></a>TW-D-1 | **What the crate is for.** Report length changes across a large population of files, on a beat the caller owns, more efficiently than one timer per file. |
| <a id="tw-d-2"></a>TW-D-2 | **A file's length is read through a handle the crate holds open**, never by path and never from directory change notifications. |
| <a id="tw-d-3"></a>TW-D-3 | **Registration is by `Wtf16String` path with a starting offset, opens and validates at once, and returns an RAII value carrying a caller `T`.** |
| <a id="tw-d-4"></a>TW-D-4 | **An attention reports a length change in either direction: a clone of `T`, the last reported length, and the current length.** The crate attaches no meaning to the change and is never told what was consumed. |
| <a id="tw-d-5"></a>TW-D-5 | **The caller owns the beat; `poll` initiates one poll and returns; one poll at a time; no thread wakeups between polls.** |
| <a id="tw-d-6"></a>TW-D-6 | **Delivery is one crate-created submission queue and two crate-created manual-reset doorbell events.** No CQ. |
| <a id="tw-d-7"></a>TW-D-7 | **A poll is three passes -- copy, probe, report -- and a full SQ stops the third without waiting, to continue from a kept position when the caller rings back; the roster lock is never held across a probe or a doorbell ring.** Amended 2026-10-15: was a roster position and "no lock across a probe, an SQ write, or a doorbell". |
| <a id="tw-d-8"></a>TW-D-8 | **Registration and unregistration are legal during a poll, and neither waits for it.** |
| <a id="tw-d-9"></a>TW-D-9 | **Cancellation is shutdown: an explicit, non-blocking, idempotent request; `Drop` drains the crate's own callbacks and is allowed to block, which after a request means at most a running poll reaching its next step.** Amended 2026-10-15: said `Drop` afterwards had nothing to wait for. |
| <a id="tw-d-10"></a>TW-D-10 | **One crate. Each file is opened as a windows-overlapped-io-sys endpoint, and probed through that endpoint's observers.** Amended 2026-10-14: was a bare `SharedHandle`. |
| <a id="tw-d-11"></a>TW-D-11 | **Built in iterations: the first reads no bytes; reads on the thread pool come next.** |
| <a id="tw-d-12"></a>TW-D-12 | **Fixtures are Rust only.** |

Working positions, which carry no decision ID and which nothing should bind to as settled, are at
the end under [Working positions](#working-positions-not-decisions).

## TW-D-1: what the crate is for

The ultimate consumers are off-machine listeners watching the stdout and stderr of many child
processes, which write that output to files. They want a reasonably current view of it. The crate
reports when each watched file's length changes, polling all of them on one beat the caller drives,
in place of one timer per file firing on its own schedule.

What causes a length to change is not the crate's concern: it knows registrations, not processes
([TW-D-8](#tw-d-8), and see the rationale for end of stream).

**Out of scope: comparative evidence against per-file timers.** That belongs to the component that
chooses between the approaches. The cost of a full poll at a given population is this crate's own
behaviour and is the crate's to observe.

## TW-D-2: a file's length is read through a held handle

Each registered file is opened once and the handle is kept. On every beat the crate asks for the
file's current length through that handle (`GetFileInformationByHandleEx` with `FileStandardInfo`).

- **Not by path.** A path-based query opens and closes the file each time, and on NTFS each one
  also updates the directory entry and produces a change notification. Nothing depends on those
  notifications.
- **Not from directory notifications or the directory entry.** Both lag the file's real length by
  many seconds on NTFS and ReFS.
- **"Buffered" means the operating system's view only.** The crate has no insight into or control
  over the writing process; it reports exactly the bytes that have reached the file system.

The measurements behind these points are recorded in the
[design session](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#experiments-re-verifying-the-premises).
The harness that produced them was not kept ([TW-D-12](#tw-d-12)), so nothing re-checks them.

## TW-D-3: registration

- **By path, as `Wtf16String`** from [wtf-string](../wtf-string/), converted losslessly to the path
  the endpoint open takes.
- **Opens at once, as an endpoint** ([TW-D-10](#tw-d-10)): read-only, sharing read, write and delete,
  with `FILE_FLAG_OVERLAPPED` so that reads can later complete on the thread pool
  ([TW-D-11](#tw-d-11)).
- **The path must exist.** A failed open is an error carrying the Win32 error code.
- **Takes a starting offset, checked against the length read at open.** An offset beyond the
  length is a registration error with its own `Err` value, distinct from an open failure. The
  starting offset is what lets a restarted parent resume where it left off.
- **No uniqueness and no lookup by name.** Registering one path twice gives two independent
  registrations.
- **The watcher is generic over a caller type `T: Clone`.** A `T` is moved into each registration
  and identifies it in every attention ([TW-D-4](#tw-d-4)).
- **Registration returns an RAII value whose `Drop` unregisters.**

## TW-D-4: what an attention carries

A poll produces one *attention* for each registration whose current length differs from the length
last reported for it. An attention carries:

- **a clone of the registration's `T`**, so it never depends on the registration still existing
  ([TW-D-8](#tw-d-8)). A caller wanting cheap clones may choose an `Arc` as its `T`;
- **the last known length**, meaning the last length the crate *reported*; and
- **the current length**.

Reporting the attention makes the current length the new last known length.

- **Either direction.** A shrink is reported like growth. The crate gives a change no meaning; the
  receiver interprets the pair.
- **Replacement is not detected.** The held handle follows the file it opened.
- **The crate is never told what was consumed.** Consumption is the caller's own state, and is what
  the caller would persist to re-register with a starting offset after a restart. There is no path
  from caller to crate for it ([TW-D-6](#tw-d-6)).
- **One outstanding attention per file is the intent, not an invariant.** Nothing may be correct
  only because it holds; an attention carries absolute lengths, so a duplicate is redundant, not
  conflicting.

## TW-D-5: the beat

- **The caller owns the beat.** The crate runs no timer and never starts a poll on its own.
- **`poll` initiates a poll and returns.** Completion is signalled by a poll-done entry in the SQ
  ([TW-D-6](#tw-d-6)). No synchronous form.
- **One poll at a time.** A second `poll` while one is in progress, or any `poll` after shutdown
  ([TW-D-9](#tw-d-9)), is refused. Because a poll outlives the call that started it, this is a
  runtime check, not an exclusive borrow. One poll at a time gives a single resume position
  ([TW-D-7](#tw-d-7)) and a single writer for each registration's last known length.
- **Threads: the fewest that finish a poll within a reasonable beat.** The cost of one length query
  and the beat interval are not yet known, so no thread count is fixed. The design must leave the
  querying itself shardable across threads without committing to it.
- **No thread wakeups between polls.** The process should quiesce as far as possible. State that
  sits still -- held handles, the roster, memory, an idle thread-pool wait -- is not what this
  requirement is about.

## TW-D-6: delivery

- **One submission queue (SQ), created by the crate.** The crate writes entries; the caller drains
  them. There is no completion queue: nothing flows from caller to crate except the resume
  doorbell.
- **Two kinds of SQ entry**: an attention ([TW-D-4](#tw-d-4)), and a poll-done entry marking the end
  of a poll. The poll-done entry is ordinary: it is queued after the attentions and waits for space
  like them.
- **Two doorbells, both Windows events created by the crate, both manual-reset.** Manual-reset
  because an auto-reset event can lose a signal, which
  [windows-waitable-queues](../windows-waitable-queues/src/doorbell.rs) established.
  - *Crate to caller*: rung when a batch is ready, when the SQ is full, and when the poll-done entry
    is written. The caller learns which by reading the SQ.
  - *Caller to crate*: rung when the caller has made space in the SQ, so a stopped poll can resume.
- **The SQ holds a fixed number of entries, set by the caller when the watcher is created, and is a
  power of two of at least two.** It is many-writer and one-reader, so a poll can be sharded across
  threads without a change here, and a writer that finds it full is refused and gets its entry back
  rather than waiting ([TW-D-7](#tw-d-7)). The drain takes entries oldest first until there are none
  and is the caller's only way to them. The storage is windows-waitable-queues'
  [slotwise_mpsc](../windows-waitable-queues/src/slotwise_mpsc.rs), whose capacity rule this
  decision adopts as its own: that crate's behaviour is not the specification, and a replacement
  would have to keep these properties.
- **Each doorbell is a handle the caller can wait on, and resetting is not automatic.** Ringing
  stays rung, for every waiter, until a reset. The reader of a doorbell resets it *before* reading
  what it announces, so an entry written after the reset rings it again; resetting after the read
  can erase a ring for an entry not yet read, which is the lost wakeup that a manual-reset event
  was chosen to avoid. The watcher offers the reset and the ring for the caller's side and keeps
  the other two to itself. It does not reset in `drain`, which a caller who stops draining early
  would turn into a wait for entries that are still queued; whether it should is open.
- **Creating a watcher can fail for two reasons**, a capacity the SQ cannot have and a doorbell the
  operating system would not create, and the error says which.
- **The crate creates both, rather than the caller supplying them**, because the reset mode is a
  correctness property and because the SQ then lives exactly as long as the watcher.

## TW-D-7: the poll is three passes; a full SQ stops the third

**Amended 2026-10-15:** this decision kept a position in the roster and said no lock is held across a
probe, an SQ write, or a doorbell ring. The poll is now three passes and the kept position is an
index into a saved list of changes; the lock rule is about the roster lock and about operations that
can wait on something outside the crate. The reasoning, and the engineer's original wording, are in
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md).

- **Pass 1, copy.** Under the roster lock, in batches by id range, copy each entry's id, last known
  length and an observer of its handle ([TW-D-10](#tw-d-10)) into a chunked buffer. The lock is
  taken per batch, so it is held for a bounded amount of map work. The batch size is not chosen.
- **Pass 2, probe.** With no lock held, read each observer's current length, drop the observer, and
  keep the id, last known and current length of those that differ, in id order.
- **Pass 3, report.** From a kept index into that list, per batch under the roster lock: look the id
  up (a dropped registration is skipped), clone `T`, write the attention to the SQ, and on success
  make the reported length the entry's last known one. Then the poll-done entry, which is an
  ordinary entry that needs room like the others.
- **A refused write stops the pass without waiting.** The index is kept, the crate-to-caller
  doorbell is rung once the lock is released, and the poll stays in flight. When the caller rings
  the resume doorbell the pass continues from the index. The list is not probed again, so an
  attention reports the length read in pass 2; growth since is the next poll's.
- **The lock rule.** The roster lock is not held across a probe or a doorbell ring, the two
  operations that can wait on something outside the crate. It is held across the SQ write, which
  never waits (a refused write returns the entry), and across `T::clone`: the lock does not depend
  on `T`, but a `Clone` that calls back into the watcher would deadlock against it, and must not.
  Other locks exist in the process that this crate does not control; the rule is about its own.
- **No allocation for these copies is larger than 64 KiB.** They are chunked buffers, held between
  polls so there is none after warm-up.
- **Roster changes between a stop and its continuation need no special handling.** A dropped
  registration's id is not found and is skipped; one made since is seen by the next poll; nothing is
  repeated, because ids are never reused ([TW-D-8](#tw-d-8)).

## TW-D-8: registration and unregistration during a poll

- **Unregistering during a poll is legal, and `Drop` of the registration value does not wait.** The
  poll's own handle reference keeps the file open until it lets go.
- **No promise that attentions stop at unregistration.** One may already be in the SQ, or be
  written by a poll that already held its reference. The client must tolerate attentions for
  registrations it has dropped.
- **A registration made during a poll may or may not be seen by that poll.** The next poll sees it.

## TW-D-9: cancellation is shutdown

**Amended 2026-10-15:** this decision had the request drain the resume wait and `Drop` wait for
nothing. Draining waits for a callback inside a probe, so the request could block, and the caller
may drop without reading the poll-done entry, so `Drop` could always have a callback to wait for.
The drain moved to `Drop`, below.

- **Cancelling means stopping the watcher for good**, not one poll. No use was found for cancelling
  a single poll and polling again, so that is not offered; nothing here forecloses adding it.
- **Shutdown is an explicit request that does not block and may be repeated.** It is typically made
  from the caller's own callback for a crate notification.
- **What the request does:** set the stop flag, and return. It does not drain. A queued resume
  callback sees the flag and returns at once; a running poll stops at its next step, which is one
  probe or one report.
- **Requested is not quiescent.** After the request a crate callback may still be running (a poll
  finishing its current probe) or queued (a resume callback about to see the flag).
- **The end of polling reaches the caller as the ordinary poll-done entry**, when there is space
  for it. A caller that does not care need not drain for it.
- **`Drop` drains, and is allowed to block** (the engineer, 2026-10-15: "Drop has to be allowed to
  block"). It stops and drains the resume wait and waits for a running poll to return, following the
  repository's [teardown-drains decision](../../DESIGN-NOTES.md#teardown-drains), and never depends
  on the poll-done entry having been written or read. After a shutdown request that wait is at most
  a running poll reaching its next step and returning; without one, `Drop` sets the stop
  flag itself first, so the bound is the same. No signal of quiescence is offered for a caller to
  avoid the wait ([OPEN-QUESTIONS.md](OPEN-QUESTIONS.md) Q36).

## TW-D-10: one crate, endpoints and observers

**Amended 2026-10-14:** this decision first held each handle as a bare `SharedHandle`; M16.1 in
windows-overlapped-io-sys replaced that with observers, below.

- **Everything is one crate**, whatever its internal layers. Changes it drives in other crates are
  acceptable.
- **Each file is opened as an endpoint of [windows-overlapped-io-sys](../windows-overlapped-io-sys/)**,
  and the crate keeps that endpoint. From the reads iteration on it is the only thing that submits
  I/O on the handle.
- **Probes run through the endpoint's *observers*.** An observer shares the handle (that crate keeps
  it in a `SharedHandle` from [win-shared-os-owned-handle](../win-shared-os-owned-handle/)), queries
  the file's length, and can never become a second endpoint. A poll takes its own observer, which is
  what lets it probe with no lock held ([TW-D-7](#tw-d-7)) while registration and unregistration
  proceed ([TW-D-8](#tw-d-8)). The handle closes when the last holder drops.
- **This makes windows-overlapped-io-sys' M16 a prerequisite of iteration 1**, not only of reads: the
  observer and its length query come from M16.2 and M16.3, archived in its
  [COMPLETED-CHECKLIST.md](../windows-overlapped-io-sys/COMPLETED-CHECKLIST.md#m162). The decision and its alternatives are in
  that crate's [DESIGN-NOTES.md](../windows-overlapped-io-sys/DESIGN-NOTES.md) -> "observers share the
  handle; only the endpoint does I/O".

## TW-D-11: iterations

- **Iteration 1 reads no bytes.** It opens, probes and reports; only tests read the bytes an
  attention announces.
- **The next iteration probably adds asynchronous reads completed on the Windows thread pool**,
  which is why handles are opened overlapped now ([TW-D-3](#tw-d-3)). Later iterations may build
  further.
- **The pieces are built as layers, each usable without the ones above it.** Their names and
  boundaries are not yet chosen; they emerge as the work proceeds.

## TW-D-12: fixtures are Rust only

Non-Rust fixtures are not kept. The PowerShell harness used to establish [TW-D-2](#tw-d-2)'s
premises was deleted for that reason; its results survive only as the tables in the design session.

## Working positions (not decisions)

Stated so they can be built on, and expected to be refined during implementation. Nothing should
bind to them as settled.

- **A registration's last known length starts at its starting offset** ([TW-D-4](#tw-d-4)). So a
  restarted parent that registers a file with the offset it saved is told, by the first poll, of
  whatever the file has gained since; offset 0 on a file that already has content reports all of it.
  The alternative, starting at the length read at registration, would report nothing until the next
  write. The engineer has not been asked.
- **The resume doorbell is waited on by a thread-pool wait the crate registers**, and the poll
  resumes from its callback. An armed, idle wait wakes no thread, so it is consistent with
  [TW-D-5](#tw-d-5).
- **The caller sets the SQ's capacity**, presumably when the watcher is created. TW-2.2 takes it
  at `Watcher::new`; see [TW-D-6](#tw-d-6).
- **The type that holds one file is `HeldFile`** (TW-1.2). A working name taken from this file's
  own phrase "held handle", not a chosen layer: [TW-D-11](#tw-d-11) leaves the names to emerge.
  Nothing here should bind to it.
- **The expected population tops out around 20,000 files** (some 10,000 jobs, each with stdout and
  stderr), a guesstimate the engineer regards as an extreme.
