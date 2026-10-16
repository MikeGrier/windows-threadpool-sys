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
| <a id="tw-d-10"></a>TW-D-10 | **One crate. Each file is opened as a windows-overlapped-io-sys endpoint, and probed through that endpoint's observers.** Amended 2026-10-14: was a bare `SharedHandle`. Amended 2026-10-16: the reading layer holds the endpoint as an `EndpointHandle` and is its only submitter, through an I/O ring. |
| <a id="tw-d-11"></a>TW-D-11 | **Built in iterations: the first reads no bytes; reads through an I/O ring come next.** Amended 2026-10-16: was reads completed on the thread pool. |
| <a id="tw-d-12"></a>TW-D-12 | **Fixtures are Rust only.** |
| <a id="tw-d-13"></a>TW-D-13 | **A failed probe is classified by its Win32 code as transient or terminal, by one predicate.** Transient: no corrective action, a diagnostic, probed again next beat. Terminal: reported to the caller as its own SQ entry carrying the code, and the entry is no longer probed. Diagnostics go to a sink the caller supplies, in the shape of durable-ioring's. |
| <a id="tw-d-14"></a>TW-D-14 | **The passes work in batches of 256 under the roster lock, the buffers in chunks of at most 64 KiB, and the roster stays a `BTreeMap`; sharding the probe across threads is a known remedy that is not built.** |
| <a id="tw-d-15"></a>TW-D-15 | **A layer above the watcher reads the bytes each poll announces through an I/O ring, into owned buffers from a pool it keeps, and delivers them as chunks on an SQ of its own, as the reads complete.** No CQ: a chunk's `Drop` returns its buffer. |

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
- **A poll is in progress from the call until its poll-done entry has been written**, so one stopped
  on a full SQ still is. A caller who has read poll-done may poll again at once, which is why the
  gate is not a flag: poll-done has to be visible only once the gate is open, yet a poll-done the SQ
  refuses is not the end of the poll. The gate has a state between running and idle for the write,
  and a `poll` that finds it waits for the write's outcome.
- **`poll` needs `T: Send + 'static`**, because the poll runs on the thread pool, and the
  thread-pool work object is made by the first `poll`, so a watcher that is never polled makes none.
  Dropping the watcher drains it, which waits for a running poll to return; a poll parked on a full
  SQ has returned and is not waited for. ([TW-D-9](#tw-d-9) decides the rest of shutdown.)
- **Threads: the fewest that finish a poll within a reasonable beat.** The beat interval is not
  known, so no thread count is fixed. The design leaves the querying itself shardable across threads
  without committing to it; what is known of the query's cost and of sharding it is in
  [TW-D-14](#tw-d-14).
- **No thread wakeups between polls.** The process should quiesce as far as possible. State that
  sits still -- held handles, the roster, memory, an idle thread-pool wait -- is not what this
  requirement is about.

## TW-D-6: delivery

- **One submission queue (SQ), created by the crate.** The crate writes entries; the caller drains
  them. There is no completion queue: nothing flows from caller to crate except the resume
  doorbell.
- **Three kinds of SQ entry**: an attention ([TW-D-4](#tw-d-4)), a terminal entry for a file that
  can no longer be probed ([TW-D-13](#tw-d-13); amended 2026-10-15, there were two), and a poll-done
  entry marking the end of a poll. The poll-done entry is ordinary: it is queued after the
  attentions and waits for space like them. The entry type is `#[non_exhaustive]`, so a further
  kind is not a break.
- **Two doorbells, both Windows events created by the crate, both manual-reset.** Manual-reset
  because an auto-reset event can lose a signal, which
  [windows-waitable-queues](../windows-waitable-queues/src/doorbell.rs) established.
  - *Crate to caller*: rung when a batch is ready, when the SQ is full, and when the poll-done entry
    is written. The caller learns which by reading the SQ.
  - *Caller to crate*: rung when the caller has made space in the SQ, so a stopped poll can resume.
    A thread-pool wait the crate registers watches it, and is armed only while a poll is stopped:
    its callback resets the doorbell, retries the report from the kept index, and only then arms
    again, so a ring that lands in between finds the event already rung and wakes the wait at once
    instead of being lost. The wait owns a duplicate of the event, which is the same event. A
    stopped poll's armed wait wakes no thread, consistent with [TW-D-5](#tw-d-5); between polls
    nothing is armed, and a ring then starts nothing.
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
  taken per batch, so it is held for a bounded amount of map work. The batch size is chosen in
  [TW-D-14](#tw-d-14).
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
- **The request is `Watcher::shutdown`, and a `poll` after it is refused** with
  `PollError::Shutdown`, which is reported ahead of "in flight" and also when a poll is parked. The
  flag is looked at before the poll's gate is taken and again after, so a request that lands between
  the two is refused as well. Registering and dropping registrations still work after it.
- **A poll stops at its next step**: a batch of the copy, one file probed, one change reported. What
  is left is abandoned, and the poll goes on to its ordinary poll-done, which is always delivered:
  one the SQ refuses leaves the poll parked, and the resumed callback arms the wait again, so it is
  written when the caller drains and rings the resume doorbell, as after any other stop. The
  engineer, 2026-10-15: "the poll-done should always be delivered"; an earlier reading of "returns
  at once" gave up after one refused attempt.
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

**Amended 2026-10-16:** who submits I/O on the handle, below, now that reads go through an I/O ring
([TW-D-15](#tw-d-15)).

- **Everything is one crate**, whatever its internal layers. Changes it drives in other crates are
  acceptable.
- **Each file is opened as an endpoint of [windows-overlapped-io-sys](../windows-overlapped-io-sys/)**,
  and the crate keeps that endpoint. From the reads iteration on it is the only thing that submits
  I/O on the handle.
- **The reading layer ([TW-D-15](#tw-d-15)) is that submitter, and the I/O ring is its backend.**
  Each file's endpoint is held as an `EndpointHandle`, the form an endpoint takes when it is consumed
  into a backend, as windows-threadpool-sys' `ThreadpoolIo` holds it; it still hands out observers,
  so probes are unchanged. A read addresses the handle by its raw value and carries an observer in
  its sidecar, so the handle stays open until the read's completion is popped even if the
  registration is dropped meanwhile; that observer is what discharges the raw address's contract.
  Neither windows-overlapped-io-sys nor windows-ioring-sys changes for this. Decided by the
  assistant at the engineer's request ("you decide!"); the alternatives are in
  [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md).
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

**Amended 2026-10-16:** the next iteration reads through an I/O ring ([TW-D-15](#tw-d-15)), not
through `ThreadpoolIo` as first planned.

- **Iteration 1 reads no bytes.** It opens, probes and reports; only tests read the bytes an
  attention announces.
- **The next iteration reads the announced bytes through an I/O ring** ([TW-D-15](#tw-d-15)). Handles
  are opened overlapped for it ([TW-D-3](#tw-d-3)): on a handle without `FILE_FLAG_OVERLAPPED` a ring
  read completes inline during submit. Later iterations may build further.
- **The pieces are built as layers, each usable without the ones above it.** Their names and
  boundaries are not yet chosen; they emerge as the work proceeds.

## TW-D-12: fixtures are Rust only

Non-Rust fixtures are not kept. The PowerShell harness used to establish [TW-D-2](#tw-d-2)'s
premises was deleted for that reason; its results survive only as the tables in the design session.

## TW-D-13: a failed probe is transient or terminal

The engineer, 2026-10-15: a failed probe's error code is inspected to decide what to do.

- **Transient** (a failure that may not recur): no corrective action. A message goes to the
  diagnostic sink, and the entry is probed again on the next beat. Messages are not aggregated: one
  per entry per beat. A sink that does not want the kind costs a branch ([below](#tw-d-13-sink)).
- **Terminal** (the handle can never answer again): the caller is told, by an SQ entry of its own
  that carries a clone of `T`, the Win32 code as a `u32`, and the last length reported, so the code
  is not lost. The report pass writes it as it writes an attention, and marks the roster entry
  terminal only once it is written, so a full SQ retries it. A terminal entry is no longer probed or
  snapshotted, stays registered until dropped, and is reported once.
- **One predicate decides which**, in one place, and every site that needs the distinction asks it.
  The terminal set starts at `ERROR_INVALID_HANDLE` and `ERROR_FILE_INVALID` (the volume under the
  file was externally altered). **The complete set is not known** and grows as the system produces
  codes nobody listed.
- **An unlisted code is transient**: the safe failure is to keep probing and say so.
- <a id="tw-d-13-sink"></a>**Diagnostics go to a sink the caller supplies, in the shape of
  durable-ioring's `DiagnosticSink` ([DI-D-51](../durable-ioring/DESIGN-NOTES.md#di-d-51)):** typed
  events and not text; a sink says which kinds it `wants` before an event is built; the default,
  `NoDiagnostics`, wants none; supplied through `Watcher::with_diagnostics` as an
  `Arc<dyn DiagnosticSink>`, and `Watcher::new` is the default. A sink is called
  from whatever thread observed the event, returns promptly, and does not call back into the
  watcher. The shape is mirrored, not shared: durable-ioring's event and kind enums are its own.
  Whether the machinery moves to a crate both use is a later decision, tracked in
  [CHECKLIST.md](CHECKLIST.md).

## TW-D-14: batch and chunk sizes, the roster map, and sharding

The engineer, 2026-10-15, from the captures [bench-captures/2026-10-15.txt](bench-captures/2026-10-15.txt)
and [bench-captures/2026-10-15-query-threads.txt](bench-captures/2026-10-15-query-threads.txt).

- **A batch is 256 entries**, for the snapshot (`SNAPSHOT_BATCH`) and for the report
  (`REPORT_BATCH`). The capture records the roster lock's hold per batch for both.
- **A chunk holds at most 64 KiB** ([TW-D-7](#tw-d-7)); unchanged.
- **The roster stays a `BTreeMap`.** The capture times the map alone, beside the file opens and
  closes that registration and unregistration cost.
- **Sharding the probe pass across threads is not built.** The capture times a bare
  `GetFileInformationByHandleEx(FileStandardInfo)` loop, the same loop through `file_len`, and both
  across 1 to 16 threads over the same handles. It is the remedy to reach for if a poll is found too
  long for its beat, and the SQ already admits many writers ([TW-D-6](#tw-d-6)). This decision
  schedules no work: nothing is queued for it, so that its absence from the checklist is
  intentional.
- **Retention is not decided**: whether the buffers give chunks back after an unusually large poll.
  The captures include no roster that shrinks, so they say nothing about it; it is
  [TW-2+.2](CHECKLIST.md).

## TW-D-15: reading the announced bytes through an I/O ring

The engineer, 2026-10-16, in place of the reads through `ThreadpoolIo` first planned.

- **A layer above the watcher reads what each poll announces**, through an I/O ring from
  [windows-ioring-sys](../windows-ioring-sys/). It owns a `Watcher<T>` and is that watcher's only
  consumer; the watcher keeps its own SQ and doorbells and stays usable without it
  ([TW-D-11](#tw-d-11)). The layer's name is not chosen.
- **What is read is what the attention announced**: `last_known..current`, split into reads of one
  buffer each. A shrink is reported with no read, and a terminal entry passes through. Growth after
  the probe is the next poll's, as it is for attentions ([TW-D-7](#tw-d-7)).
- **Owned buffers from a pool the layer keeps** (the engineer: "owned buffers I would say"). Each read
  moves a buffer into the ring, which hands it back with the completion. An empty pool submits no
  read until a buffer comes back; it never allocates past its size.
- **The client receives the bytes on an SQ of the layer's own**, with the watcher's doorbell
  discipline ([TW-D-6](#tw-d-6)). A data entry carries a clone of `T`, the byte range of the file, and
  a chunk that holds the buffer and dereferences to the bytes read into it. Shrink and terminal
  entries pass the watcher's through, and poll-done follows every read of its poll and is always
  delivered ([TW-D-9](#tw-d-9)).
- **Chunks are delivered as their reads complete**, not in file order (the engineer: "deliver as they
  complete, consumers can deal with reordering instead of us"). Each carries its range, which is what
  reordering needs.
- **No CQ.** The one thing that flows back is that the client is done with a buffer, and the chunk's
  `Drop` carries it: the buffer returns to the pool, and a layer waiting for one is woken.
- **The pool allocates every buffer, zeroed, when it is made**, and wakes a layer waiting for one
  by a manual-reset doorbell of its own: a take it refuses arms it, and the first buffer given back
  after that rings it, once. The waiter resets it before taking again, as for the watcher's
  doorbells ([TW-D-6](#tw-d-6)). A buffer lent for a read goes back when it drops whether or not it
  became a chunk, and lent buffers may outlive the pool.
- **Three things stop the reads without waiting**: an empty pool, a full ring SQ, a full client SQ.
  Each continues from where it stopped when a chunk drops, a read completes, or the client rings
  back, as a poll does on a full SQ ([TW-D-7](#tw-d-7)).
- **Who submits the reads** is in [TW-D-10](#tw-d-10).
- **Each read finds its file in the attention it plans from.** Files are registered through the
  reading layer, and the value the watcher is made over is the caller's `T` together with the
  registration's id, the handle's value and an observer of the handle, all taken when the file is
  opened. The watcher's public entries do not change, and nothing is looked up when a read is
  planned. The observer travels with the attention and then with the read, so a registration
  dropped after its attention was written still has its bytes read.
- **A read asks for exactly the bytes planned.** A ring read fills the whole buffer it is given, so
  the buffer is lent with a window of the planned length, and a read can never run past what was
  announced and be announced again by the next poll.
- **Delivery is in the order entries are made, one at a time.** An entry the client's SQ refuses is
  kept, and while it is held nothing else is taken, so a full SQ also leaves completions in the ring,
  where the buffers they hold are accounted for, and the layer holds at most one entry the client has
  not taken. Poll-done is delivered after every read of its own poll is settled, and after the
  poll-dones before it; the reads of a later poll may be delivered before it.
- **"The ring is full" is its completion queue.** A submit empties the ring's SQ, so a refused push
  is answered by submitting and going on; what stops the reads is the ring holding as many reads --
  queued, in flight, or completed and not popped -- as its completion queue does, at the size the
  ring reports. A read popped makes room.
- **On the thread pool, four events run the engine, each by a wait of its own.** They are the
  watcher's doorbell (a poll wrote entries), the ring's completion event (a read finished), the
  pool's doorbell (a chunk came back to an empty pool) and the client's resume doorbell (room in its
  SQ). A wait's callback runs the engine and watches again; the run resets all three doorbells before
  it looks ([TW-D-6](#tw-d-6)), so watching again does not spin. A run that took entries from the
  watcher's SQ rings the watcher's resume doorbell, which is what continues a poll parked on a full
  SQ. A run's ring error is kept for the owner and the layer goes on.
- **The waits stay armed between polls and are idle.** The ring's completion event is attached for
  the ring's life and cannot be detached, and a wait that is armed on an event nothing rings wakes no
  thread, which is what [TW-D-5](#tw-d-5) asks for. Each wait re-arms only itself, because arming a
  wait from another thread is not excluded by draining it.
- **Shutdown stops the reads and nothing else** ([TW-D-9](#tw-d-9)). It is a request that does not
  block and may be repeated: it shuts the watcher down, stops the engine from planning or submitting
  any read, and wakes the engine. What was planned and not submitted is abandoned, reads in flight
  are left to complete and are delivered, attentions that arrive afterwards are not read, and
  terminal and shrink entries and every poll-done are still delivered, so a poll's poll-done is
  never lost to shutdown. The request needs no lock the engine holds, because a callback may hold it
  for a step.
- **Reads are completed, not cancelled.** The ring the layer owns holds only operations it holds
  buffers for, and cancelling one needs a ring that can also hold operations it holds nothing for,
  which is the thing that ring was made not to be. A read of a file ends on its own.
- **Dropping the layer stops and drains each wait, then releases the engine**, having requested
  shutdown first. What it waits for is a running engine step, a running poll reaching its next step,
  and the reads already in flight, which complete in parallel: it never waits for a read that was
  not submitted, nor for the client to drain or to take a poll-done.
- **A read that returns nothing delivers nothing.** That is a read past the end of a file that
  shrank, whether reported as zero bytes or as end of file, and its buffer goes back. Neither is read
  again.
- **A failed read is classified as a failed probe is** ([TW-D-13](#tw-d-13), by the same predicate).
  The engineer, 2026-10-16: terminal errors must appear in the SQ, and transient ones go to the
  diagnostic channel only at this time. Either way the sink is told, as
  `Diagnostic::ReadFailed` ([TW-D-13](#tw-d-13-sink)); a terminal one is also delivered as the
  layer's terminal entry, with the Win32 code, and with `last_known` the offset the failed read began
  at, since the bytes from there are not read again and a client registers again at it. The bytes of
  a failed read are not announced again either way.
- **Possibility, not a plan: a recurring transient read failure may enter the SQ.** The engineer,
  2026-10-16: the first occurrence probably would not, but a recurrence might, and most recurring
  transient errors are suspected to be terminal. Nothing here schedules work for it: an actual case
  of one is wanted first.
- **Registered buffers are not built.** They would need windows-ioring-sys to lend one registered
  buffer to another thread while the ring fills its neighbours, which its `RegisteredBuffers::get`
  does not (it borrows the whole registration mutably). This decision schedules no work for them, so
  their absence from the checklist is intentional.

## Working positions (not decisions)

Stated so they can be built on, and expected to be refined during implementation. Nothing should
bind to them as settled.

- **A registration's last known length starts at its starting offset** ([TW-D-4](#tw-d-4)). So a
  restarted parent that registers a file with the offset it saved is told, by the first poll, of
  whatever the file has gained since; offset 0 on a file that already has content reports all of it.
  The alternative, starting at the length read at registration, would report nothing until the next
  write. The engineer has not been asked.
- **The caller sets the SQ's capacity**, presumably when the watcher is created. TW-2.2 takes it
  at `Watcher::new`; see [TW-D-6](#tw-d-6).
- **The type that holds one file is `HeldFile`** (TW-1.2). A working name taken from this file's
  own phrase "held handle", not a chosen layer: [TW-D-11](#tw-d-11) leaves the names to emerge.
  Nothing here should bind to it.
- **The expected population tops out around 20,000 files** (some 10,000 jobs, each with stdout and
  stderr), a guesstimate the engineer regards as an extreme.
- **The reading layer's pool is sized by the caller** ([TW-D-15](#tw-d-15)): the buffer size and
  count when the layer is created, with any defaults left to measurement.
- **A short read is delivered as the bytes it read**, labelled with that range. The rest of the
  announced range is not retried: a file that shrank under the read is the next poll's shrink.
- **The submit a failed submit left queued is not tested.** `SubmitIoRing` cannot be made to fail on
  demand, so no test reaches `submit_queued`'s work; it is written and not exercised.
