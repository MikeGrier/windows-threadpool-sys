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

- [x] **TW-4.1** -- `Watcher::shutdown` sets a stop flag and returns, may be repeated, and `poll` is refused with `PollError::Shutdown` after it. -> [completed 2026-10-15](COMPLETED-CHECKLIST.md#tw-41)

- [x] **TW-4.2** -- A shutdown stops a running or parked poll at its next step (a batch of the copy, a file probed, a change reported) and the poll goes on to poll-done, which is always delivered. -> [completed 2026-10-15](COMPLETED-CHECKLIST.md#tw-42)

- [x] **TW-4.3** -- Dropping a `Watcher` stops it as `shutdown` does, waits for a running poll to reach its next step and return, and never waits for poll-done or for a parked poll. -> [completed 2026-10-16](COMPLETED-CHECKLIST.md#tw-43)

- [x] **TW-4.4** -- Shutdown requested from the caller's own callback on a thread-pool wait is tested from a parked poll, a poll inside its probe and a poll held inside the report, with repeated requests, drops bounded by a step, and nothing running after `Drop`. -> [completed 2026-10-16](COMPLETED-CHECKLIST.md#tw-44)

## M5 -- Reading the announced bytes through an I/O ring, driven synchronously

Deliverable: the reading layer, driven directly with no thread pool, over hundreds of files that
child processes write and a pool far smaller than the bytes announced, delivers every announced byte
exactly once across chunks labelled with their ranges, and then poll-done. Driven synchronously first
for the same reason as M2: every stop and continuation can then be tested deterministically
([TW-D-15](DESIGN-NOTES.md#tw-d-15)). These items replace TW-4+.1 and TW-4+.2, which planned the reads
through `ThreadpoolIo`.

- [x] **TW-5.1** -- `HeldFile` holds its endpoint as an `EndpointHandle`, consumed from the `UnassociatedEndpoint` when the file is opened, and lends it to the crate for the reading engine. -> [completed 2026-10-16](COMPLETED-CHECKLIST.md#tw-51)

- [x] **TW-5.2** -- A pool of owned buffers of one size, all allocated and zeroed when it is made, lends each as a buffer a ring read can take and a chunk of the bytes read, both given back on drop, with a doorbell for a starved reader. -> [completed 2026-10-16](COMPLETED-CHECKLIST.md#tw-52)

- [x] **TW-5.3** -- The reading layer's SQ, made by the same function and capacity rule as the watcher's, carries data entries whose range is computed from their chunk, shrink, terminal and poll-done entries, with its own doorbell pair. -> [completed 2026-10-16](COMPLETED-CHECKLIST.md#tw-53)

- [x] **TW-5.4** -- Each of the watcher's entries is planned without a ring: growth as reads of at most one buffer each, computed as they are taken, a shrink as a shrink entry, a terminal entry passed through, and poll-done kept for the engine. -> [completed 2026-10-16](COMPLETED-CHECKLIST.md#tw-54)

- [x] **TW-5.5** -- The reading engine, driven synchronously against a ring, drains the watcher's SQ, reads each attention's announced bytes into pool buffers with the handle addressed raw and an observer in the sidecar, delivers a data entry per read as it completes, and stops without waiting on an empty pool, a full ring or a full client SQ and continues from where it stopped. -> [completed 2026-10-16](COMPLETED-CHECKLIST.md#tw-55)

- [x] **TW-5.6** -- The reading engine is tested over real files that child processes write through inherited handles, through a pool of a few buffers and a client SQ of two: every byte delivered once, chunks the client keeps stopping the reads and dropping them resuming them, and registrations dropped with reads in flight. -> [completed 2026-10-16](COMPLETED-CHECKLIST.md#tw-56)

## M6 -- The reading layer on the thread pool

Coarse on purpose: its scope is set when M5 is done.

- [x] **TW-6.1** -- The reading engine runs on the thread pool with no thread of the caller's: the watcher's doorbell, a completed read, a chunk returned to an empty pool and the client's resume doorbell each drive it through a wait of their own, which stays armed and idle between polls. -> [completed 2026-10-16](COMPLETED-CHECKLIST.md#tw-61)

- [ ] **TW-6.2** -- **Integration tests: polls on the thread pool over files that children write**,
  through a small pool and a client SQ of two slots.

- [ ] **TW-6.3** -- **Decide whether a failed read is an entry of the client's SQ.** Today it is a
  `Diagnostic::ReadFailed` and nothing else, and the bytes it was to read are not announced again
  ([TW-D-15](DESIGN-NOTES.md#tw-d-15), working positions).

## M7 -- Shutdown with reads in flight

Coarse on purpose: its scope is set when M6 is done.

- [ ] **TW-7.1** -- **Shutdown and `Drop` with reads in flight**: the reads cancelled or completed, the
  ring run down, poll-done always delivered, and the bound on `Drop` settled for reads.
  ([TW-D-9](DESIGN-NOTES.md#tw-d-9))

- [ ] **TW-7.2** -- **Integration tests: shutdown from every state of a read.**