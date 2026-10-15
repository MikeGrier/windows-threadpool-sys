# Checklist: win-multi-file-tail-watcher

Reports length changes across a large population of files, on a beat the caller owns. See
[DESIGN-NOTES.md](DESIGN-NOTES.md) for the decisions every item cites, and
[README.md](README.md) for what the crate is.

Each milestone ends in a deliverable exercised by integration tests: every one of them crosses a
real file system, process, or thread-pool boundary. Unit tests accompany each item as usual.

## M2 -- The poll engine: three passes and a stop point, driven synchronously

Deliverable: the engine, called directly with no thread pool, over hundreds of real files and an SQ
far smaller than the number of changes, reports every change exactly once across stops and
continuations, then a poll-done entry. The engine comes before the thread pool because the stop and
the continuation are part of the third pass, and a synchronous engine can be tested through every
one of them deterministically ([TW-D-7](DESIGN-NOTES.md#tw-d-7)).

- [x] **TW-2.1** -- `Watcher<T: Clone>` registers files into a roster and returns an RAII `Registration` whose `Drop` unregisters. -> [completed 2026-10-15](COMPLETED-CHECKLIST.md#tw-21)

- [x] **TW-2.2** -- The watcher owns a bounded submission queue of attention and poll-done entries, set at `Watcher::new` and drained by the caller. -> [completed 2026-10-15](COMPLETED-CHECKLIST.md#tw-22)

- [x] **TW-2.3** -- The watcher owns two manual-reset doorbell events, one each way, each exposed as a waitable handle. -> [completed 2026-10-15](COMPLETED-CHECKLIST.md#tw-23)

- [x] **TW-2.4** -- `Chunked<T>`, an append-only sequence in chunks of at most 64 KiB that keeps its chunks across a clear, for the poll's copies. -> [completed 2026-10-15](COMPLETED-CHECKLIST.md#tw-24)

- [x] **TW-2.5** -- Each roster entry holds a last known length starting at its starting offset, and the snapshot pass copies id, last known length and an observer into a chunked buffer one batch per lock. -> [completed 2026-10-15](COMPLETED-CHECKLIST.md#tw-25)

- [ ] **TW-2.6.1** -- **Classify a probe failure.** One predicate, in one place, from an
  `io::Error` to transient or terminal. Terminal: `ERROR_INVALID_HANDLE` and `ERROR_FILE_INVALID`;
  anything else, and an error carrying no Win32 code, is transient. The set grows as the system
  produces codes, so it is a single list that is easy to extend. A unit test for each named code
  and for several that are not. ([TW-D-13](DESIGN-NOTES.md#tw-d-13))

- [ ] **TW-2.6.2** -- **The diagnostic sink**, in the shape of durable-ioring's `DiagnosticSink`:
  this crate's own typed `Diagnostic` and `DiagnosticKind`, a `wants` check before an event is
  built, `NoDiagnostics` as the default, a collecting sink for tests, and an optional
  `Arc<dyn DiagnosticSink>` supplied when the watcher is made. The first event is a transient probe
  failure, carrying the registration's id and the Win32 code. Every message goes through it.
  ([TW-D-13](DESIGN-NOTES.md#tw-d-13))

- [ ] **TW-2.6.3** -- **The terminal SQ entry.** A third `SqEntry` kind carrying a clone of `T`, the
  Win32 code as a `u32`, and the last length reported; `SqEntry` becomes `#[non_exhaustive]`.
  ([TW-D-13](DESIGN-NOTES.md#tw-d-13), [TW-D-6](DESIGN-NOTES.md#tw-d-6))

- [ ] **TW-2.6.4** -- **A terminal state on a roster entry**, which the snapshot skips and which is
  set only by the report pass once the terminal entry is written.
  ([TW-D-13](DESIGN-NOTES.md#tw-d-13))

- [ ] **TW-2.6** -- **The probe pass.** Reads each snapshotted observer's current length with no
  lock held, drops each observer as soon as it has been read, and keeps the id, last known and
  current length of those that differ, in id order, in a second chunked buffer. A probe that fails
  is classified: a transient failure is reported through the diagnostic sink and the entry is
  probed again next beat; a terminal one is kept in the list for the report pass to write.
  ([TW-D-2](DESIGN-NOTES.md#tw-d-2), [TW-D-4](DESIGN-NOTES.md#tw-d-4),
  [TW-D-13](DESIGN-NOTES.md#tw-d-13))

- [ ] **TW-2.7** -- **The report pass and the stop point.** Walks the changed list from a recorded
  index. Per batch, under the roster lock: look the id up (a dropped registration is skipped), clone
  `T`, write the attention to the SQ, and on success make the reported length the entry's last known
  one. A terminal failure in the list is written the same way as a terminal entry, and on success
  marks the roster entry terminal. A refused write stops the pass without waiting and keeps the
  index; a clone made for a refused write is dropped after the lock is released. The poll-done entry follows the last
  attention, needs room like any entry, and stops the pass the same way when there is none. The
  engine runs to a stop or to completion and says which; continuing is calling it again from the
  kept index. `T::clone` runs under the lock, so a `Clone` that calls back into the watcher would
  deadlock; the public documentation says so. ([TW-D-4](DESIGN-NOTES.md#tw-d-4),
  [TW-D-6](DESIGN-NOTES.md#tw-d-6), [TW-D-7](DESIGN-NOTES.md#tw-d-7))

- [ ] **TW-2.8** -- **Benchmark the passes against population size**, from 1,000 files to the
  expected top end, each pass separately: the snapshot (the lock hold per batch and in total), the
  probe, and the report; and the roster's walk and churn under appended registrations and random
  removals. The numbers choose the batch size, the chunk size, whether the roster's `BTreeMap`
  stays, and whether the buffers should give chunks back after an unusually large poll. They also inform the thread count, which [TW-D-5](DESIGN-NOTES.md#tw-d-5) leaves open;
  sharding the probe pass is deliberately not scheduled until a measurement calls for it.
  Comparison against per-file timers is out of scope ([TW-D-1](DESIGN-NOTES.md#tw-d-1)).

- [ ] **TW-2.9** -- **Integration tests: the engine over real files.** Hundreds of files, some
  grown and some shrunk by child processes between runs, against an SQ far smaller than the number
  of changes. Every change is reported exactly once across stops, with the right last known and
  current pair, and then poll-done; a run with no writes reports only poll-done; a registration
  dropped between a stop and its continuation is skipped, and one added meanwhile is seen by the
  next run; an attention for a dropped registration carries a usable `T`.

## M2+ -- Decisions to revisit once M2 is done

Parked, not pending: the milestone that is next after M2 pulls these in.

- [ ] **TW-2+.1** -- **Decide whether the diagnostic sink's machinery moves to a crate both this one
  and durable-ioring use**, generic over the event type, or whether this crate's mirror stays. The
  mirror is deliberate duplication, so that the sink could be built without touching another
  crate's layering; the decision is to merge it or to delete the question, and is made when there
  is more than one event to compare. ([TW-D-13](DESIGN-NOTES.md#tw-d-13))

## M3 -- The poll on the thread pool

Deliverable: `poll` runs the engine on the thread pool, one at a time. When the SQ fills it parks,
and resumes when the caller rings back. A watcher over hundreds of files with child-process writers
is polled repeatedly while registrations are dropped and added, with no hang and no crate callback
running between polls.

- [ ] **TW-3.1** -- **`poll` submits the engine to the thread pool and returns.** One poll at a
  time: a second `poll` is refused at run time while one is in flight, which includes one parked on
  a full SQ. The crate-to-caller doorbell is rung when entries are ready, when the SQ fills and when
  poll-done is written, after the roster lock is released. No crate callback runs between polls.
  ([TW-D-5](DESIGN-NOTES.md#tw-d-5), [TW-D-6](DESIGN-NOTES.md#tw-d-6))

- [ ] **TW-3.2** -- **Park on a full SQ and resume on the caller's doorbell**, through a thread-pool
  wait the crate registers. The resumed callback resets the resume doorbell, retries, and only then
  arms the wait again, so a ring that arrives in between is not lost. The pass continues from the
  kept index. ([TW-D-5](DESIGN-NOTES.md#tw-d-5), [TW-D-6](DESIGN-NOTES.md#tw-d-6),
  [TW-D-7](DESIGN-NOTES.md#tw-d-7))

- [ ] **TW-3.3** -- **Registration and unregistration during a running poll.** Dropping a
  registration mid-poll neither waits nor invalidates attentions already written; a registration
  made mid-poll may or may not be seen. A registration call waits at most for one batch of map
  work. ([TW-D-8](DESIGN-NOTES.md#tw-d-8))

- [ ] **TW-3.4** -- **Integration tests: polls over real files on the thread pool.** Hundreds of
  changing files with child-process writers against an SQ of two slots: every change is reported
  across parks and resumes; registrations dropped and added mid-poll cause no failure and no hang; a
  concurrent `poll` is refused; between polls no crate callback runs.

## M4 -- Shutdown

Deliverable: shutdown requested from inside the caller's own doorbell callback stops the watcher
from any state without blocking, and the subsequent `Drop` waits at most for a running poll to reach
its next step, which is one probe or one report.

- [ ] **TW-4.1** -- **The shutdown request**: non-blocking, idempotent, and `poll` refused after it.
  ([TW-D-9](DESIGN-NOTES.md#tw-d-9))

- [ ] **TW-4.2** -- **Stopping the poll**: the request sets the stop flag and returns without
  draining; a queued resume callback returns at once on seeing the flag, and a running poll stops at
  its next step. The poll-done entry is written as normal when there is space.
  ([TW-D-9](DESIGN-NOTES.md#tw-d-9))

- [ ] **TW-4.3** -- **`Drop` of the watcher**: sets the stop flag if no request was made, then stops
  and drains the resume wait and waits for a running poll to return; never depends on the poll-done
  entry having been written or read. ([TW-D-9](DESIGN-NOTES.md#tw-d-9))

- [ ] **TW-4.4** -- **Integration tests: shutdown from every state.** From the caller's doorbell
  callback on a thread-pool wait, asserting the request returns while a poll is inside a probe;
  while parked on a full SQ; during a running poll over a large roster; repeated requests; `Drop`
  with and without a prior shutdown, each bounded by one step; no crate callback runs after `Drop`
  returns.

## M4+ -- Reads on the thread pool (parked behind M4)

Coarse on purpose: its scope is set when M4 is done.

- [ ] **TW-4+.1** -- **Bind each kept endpoint to the thread pool** (`ThreadpoolIo`), with probes
  continuing through observers. ([TW-D-10](DESIGN-NOTES.md#tw-d-10))

- [ ] **TW-4+.2** -- **Asynchronous reads of announced bytes, completed on the thread pool.**
  ([TW-D-11](DESIGN-NOTES.md#tw-d-11))
