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

- [x] **TW-2.6.1** -- One predicate classifies a probe failure as transient or terminal from its Win32 code; the terminal list is `ERROR_INVALID_HANDLE` and `ERROR_FILE_INVALID`. -> [completed 2026-10-15](COMPLETED-CHECKLIST.md#tw-261)

- [x] **TW-2.6.2** -- The watcher reports typed diagnostic events to a sink the caller supplies through `Watcher::with_diagnostics`, in the shape of durable-ioring's `DiagnosticSink`. -> [completed 2026-10-15](COMPLETED-CHECKLIST.md#tw-262)

- [x] **TW-2.6.3** -- `SqEntry::Terminal` is a third entry kind carrying a clone of `T`, the Win32 code and the last length reported, and `SqEntry` is `#[non_exhaustive]`. -> [completed 2026-10-15](COMPLETED-CHECKLIST.md#tw-263)

- [x] **TW-2.6.4** -- A roster entry has a terminal state, holding the Win32 code, which the snapshot skips and which `mark_terminal` sets once. -> [completed 2026-10-15](COMPLETED-CHECKLIST.md#tw-264)

- [x] **TW-2.6** -- The probe pass reads each snapshotted observer's length with no lock held, drops each observer as read, and keeps the changes and terminal failures in a second chunked buffer. -> [completed 2026-10-15](COMPLETED-CHECKLIST.md#tw-26)

- [x] **TW-2.7** -- The report pass writes the changed list to the SQ a batch at a time under the roster lock, a refused write stops the pass without waiting, and the engine continues from the kept index. -> [completed 2026-10-15](COMPLETED-CHECKLIST.md#tw-27)

- [x] **TW-2.8** -- Timings of each pass against roster sizes from 1,000 to 50,000 are captured by ignored tests, with the roster map measured alone; no constant is changed by it. -> [completed 2026-10-15](COMPLETED-CHECKLIST.md#tw-28)

- [x] **TW-2.8.1** -- The batch sizes, the chunk size and the roster map are chosen from the captures, and sharding the probe is recorded as an available remedy that is not built. -> [completed 2026-10-15](COMPLETED-CHECKLIST.md#tw-281)

- [x] **TW-2.9** -- The engine is tested over a few hundred real files, with child processes appending through stdout handles their parent opened, against an SQ far smaller than the number of changes. -> [completed 2026-10-15](COMPLETED-CHECKLIST.md#tw-29)

## M2+ -- Decisions to revisit once M2 is done

Parked, not pending: the milestone that is next after M2 pulls these in.

- [ ] **TW-2+.1** -- **Decide whether the diagnostic sink's machinery moves to a crate both this one
  and durable-ioring use**, generic over the event type, or whether this crate's mirror stays. The
  mirror is deliberate duplication, so that the sink could be built without touching another
  crate's layering; the decision is to merge it or to delete the question, and is made when there
  is more than one event to compare. durable-ioring's `StderrDiagnostics` and `FanOutDiagnostics`
  were not mirrored, so shared machinery would bring them here. ([TW-D-13](DESIGN-NOTES.md#tw-d-13))

- [ ] **TW-2+.2** -- **Decide whether the buffers give chunks back after an unusually large poll**, and how many they keep. Needs a measurement the captures do not have: a roster that grows and then shrinks, polled across the change. ([TW-D-14](DESIGN-NOTES.md#tw-d-14))

## M3 -- The poll on the thread pool

Deliverable: `poll` runs the engine on the thread pool, one at a time. When the SQ fills it parks,
and resumes when the caller rings back. A watcher over hundreds of files with child-process writers
is polled repeatedly while registrations are dropped and added, with no hang and no crate callback
running between polls.

- [x] **TW-3.1** -- `Watcher::poll` runs the engine on the thread pool and returns, one poll at a time, with the doorbell rung after each full batch and at the end. -> [completed 2026-10-15](COMPLETED-CHECKLIST.md#tw-31)

- [x] **TW-3.2** -- A poll that stops on a full SQ is parked on a thread-pool wait on the resume doorbell, and the resumed callback resets it, retries from the kept index, and only then arms again. -> [completed 2026-10-15](COMPLETED-CHECKLIST.md#tw-32)

- [x] **TW-3.3** -- Registering and dropping registrations during a running or parked poll is tested on the thread pool and needed no change: neither waits for the poll, attentions already written keep their value, and a registration made mid-poll is seen by the next poll. -> [completed 2026-10-15](COMPLETED-CHECKLIST.md#tw-33)

- [x] **TW-3.4** -- Polls over real files that child processes write run through a queue of two slots in tests of the public API, reporting every change across parks and resumes while registrations come and go. -> [completed 2026-10-15](COMPLETED-CHECKLIST.md#tw-34)

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
