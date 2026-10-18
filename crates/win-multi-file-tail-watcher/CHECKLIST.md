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

- [x] **TW-6.2** -- The layer is tested on the thread pool over files that child processes write, through a pool of four small buffers and a client SQ of two: polls repeated while the children write, every byte delivered once, a stall on kept chunks that dropping them continues, and a drop that releases every file. -> [completed 2026-10-16](COMPLETED-CHECKLIST.md#tw-62)

- [x] **TW-6.3** -- A failed read is classified as a failed probe is: a terminal one is delivered on the client's SQ as a terminal entry and reaches the diagnostic sink, a transient one reaches the sink alone. -> [completed 2026-10-16](COMPLETED-CHECKLIST.md#tw-63)

## M7 -- Shutdown with reads in flight

Coarse on purpose: its scope is set when M6 is done.

- [x] **TW-7.1** -- Shutdown and `Drop` with reads in flight: shutdown is a non-blocking, repeatable request that stops reads being planned or submitted, reads in flight complete and are delivered, every poll-done is still delivered, and `Drop` waits only for the reads already in flight. -> [completed 2026-10-16](COMPLETED-CHECKLIST.md#tw-71)

- [x] **TW-7.2** -- Integration tests over children's files: shutdown with reads in flight, while the children still write, while stalled on kept chunks, and of an idle layer, each returning and releasing every file. -> [completed 2026-10-16](COMPLETED-CHECKLIST.md#tw-72)

## M8 -- Findings of the pre-merge review

A review of the branch against origin/main, 2026-10-16, found three defects in M5 to M7.

- [x] **TW-8.1** -- A stop request that lands while a plan's reads are being pushed ends the pushing at the next read, and the reads left are abandoned. -> [completed 2026-10-16](COMPLETED-CHECKLIST.md#tw-81)

- [x] **TW-8.2** -- A poller that cannot be made leaves the watcher usable: dropping the callbacks' view of the shared state no longer stops it, so a poll after the failure is retried rather than refused with `Shutdown`. -> [completed 2026-10-16](COMPLETED-CHECKLIST.md#tw-82)

- [x] **TW-8.3** -- A terminal read failure makes its registration terminal once: a later terminal failure of it, one after a probe reported it, and one of a registration since dropped reach the diagnostic sink alone. -> [completed 2026-10-16](COMPLETED-CHECKLIST.md#tw-83)

- [x] **TW-8.4** -- A `T` whose `Drop` uses the watcher is dropped outside the roster lock, tested, and the sabotage that dropped it under the lock is declared caught rather than a control. -> [completed 2026-10-16](COMPLETED-CHECKLIST.md#tw-84)

- [x] **TW-8.5** -- The chunk directory is bound as the chunks are: no allocation of a `Chunked` is larger than 64 KiB, at the price of a ceiling of 2,730 x 2,730 chunks. -> [completed 2026-10-16](COMPLETED-CHECKLIST.md#tw-85)

- [x] **TW-8.6** -- The root `PLANS.md` row for this crate no longer carries a milestone status that the crate's checklist owns. -> [completed 2026-10-16](COMPLETED-CHECKLIST.md#tw-86)
- [x] **TW-8.7** -- CI sweeps every sabotage manifest a crate carries, not only `sabotage.json`, so this crate's integration and doctest manifests are run. -> [completed 2026-10-16](COMPLETED-CHECKLIST.md#tw-87)

- [x] **TW-8.8** -- The crate is seeded at `0.0.1` in the release-please manifest, with `Release-As: 0.1.0` on the commit that records it, so its first release is `0.1.0`. -> [completed 2026-10-16](COMPLETED-CHECKLIST.md#tw-88)

- [x] **TW-8.9** -- Error edges that had no test or no answer: a push the ring refuses is a failed read and abandons the rest of its plan, a poll whose poller cannot be made releases the gate, the layer keeps the first error a run returns, and a failed construction drains the waits it made. -> [completed 2026-10-16](COMPLETED-CHECKLIST.md#tw-89)

- [x] **TW-8.10** -- Documentation and housekeeping from the third review: `PollError::Io` and the poll docs, the callback hazards (a panic aborts, a `Drop` must not release the last owner of the watcher), `FAN` stated as derived, a control's `why`, stale lint reasons and comments, and the plans and README claims. -> [completed 2026-10-16](COMPLETED-CHECKLIST.md#tw-810)

## M9 -- The reading layer is public, and the client chooses its sizes

Deliverable: a client outside the crate, using only public items, creates the reading layer with a
buffer size, a buffer count, a client SQ capacity and the ring's two queue sizes of its own choosing,
registers files that child processes write, polls, and receives every announced byte exactly once as
chunks; and a size that cannot work is refused at construction, naming the field and its bound
([TW-D-16](DESIGN-NOTES.md#tw-d-16)). The layer is a module of this crate, not a new one.

The module is `reader`, and its types are `TailReader<T>` (the layer), `TailReaderOptions` (the five
sizes), `ReadEntry<T>` (the SQ entry) and `Chunk` (the bytes): the engineer, 2026-10-16, "the names are
fine".

- [x] **TW-9.1** -- The ring's two queue sizes are measured and asserted: how it rounds and pairs them, its largest sizes and the conditions it refuses with, and how its completion queue bounds the reads in flight against the buffer count. -> [completed 2026-10-17](COMPLETED-CHECKLIST.md#tw-91)

- [x] **TW-9.2** -- An options type holds the five sizes, checks them before anything is allocated, and refuses each size that cannot work with an error that names the option, the value and the cause, including every ring size the ring would round, replace or raise. -> [completed 2026-10-17](COMPLETED-CHECKLIST.md#tw-92)

- [x] **TW-9.3** -- The reading layer's entries are public in the `reader` module: the entry type and its data, shrink, terminal and poll-done kinds, the chunk, the drain and the SQ with its two doorbells, each with rustdoc stating what it promises and what holding it too long does. -> [completed 2026-10-17](COMPLETED-CHECKLIST.md#tw-93)

- [x] **TW-9.4** -- The reading layer itself is public as `reader::TailReader`: made from the options and the watcher's own arguments, it registers a file with an offset and a `T`, polls, exposes its SQ and doorbells, shuts down without blocking and reports the first error a run returned, with nothing it needs from the watcher made public. -> [completed 2026-10-17](COMPLETED-CHECKLIST.md#tw-94)

- [x] **TW-9.5** -- The layer is exercised from `tests/` through the public API only, over files that child processes write, under the default options, a set well above them and two small sets, with the near-term and worst-case loads, every invalid size refused, and a held chunk and an undrained client SQ each stopping the reads; the README names what each size limits and the three stops. -> [completed 2026-10-17](COMPLETED-CHECKLIST.md#tw-95)

- [x] **TW-9.6** -- A queue capacity that is valid but more than memory can hold is an error and not an abort: the queue crate's allocation refusal is mapped to a new `CreateError::Allocation` for the client queue and the watcher's own queue, and tested through the public constructor. -> [completed 2026-10-17](COMPLETED-CHECKLIST.md#tw-96)

## M10 -- Findings of the review of M9

A review of the branch against origin/main, 2026-10-17, found four defects in M9. The engineer: "polls
initiated before the previous one completes should fail. fix all".

- [x] **TW-10.1** -- A sabotage entry made stale by TW-9.6 is repaired, and every sabotage manifest of the crate is swept in full, so that no other entry is stale. -> [completed 2026-10-18](COMPLETED-CHECKLIST.md#tw-101)

- [x] **TW-10.2** -- A failure to read the ring's limits is an I/O ring failure and not a refused size: `TailReaderError::Ring`, not `Options`, with the documentation of both variants saying so. -> [completed 2026-10-18](COMPLETED-CHECKLIST.md#tw-102)

- [x] **TW-10.3** -- `TailReader::poll` is refused while the previous poll is not complete, and a poll is complete when its poll-done has been queued for the client, after every read of it is delivered, and not when the watcher has finished announcing; a client that has read poll-done may poll again at once. -> [completed 2026-10-18](COMPLETED-CHECKLIST.md#tw-103)

- [x] **TW-10.4** -- The integration tests do not use a fixed sleep as proof that asynchronous work has finished: the undrained-client-queue test asserts what the public API can observe, that the poll cannot complete and nothing is lost, and leaves the exact stop at capacity to the synchronous engine test that already asserts it, and the quiet check after a poll tolerates the one empty ring that may land late instead of sleeping past it. -> [completed 2026-10-18](COMPLETED-CHECKLIST.md#tw-104)

- [x] **TW-10.5** -- A poll refused as in flight wakes the reading engine, so that a poll held up by an error a run returned, which leaves nothing in flight and rings no doorbell, is not refused for ever now that the reader's gate is in front of the watcher's. -> [completed 2026-10-18](COMPLETED-CHECKLIST.md#tw-105)

- [x] **TW-10.6** -- The tests of the reading layer that count how many times the engine ran no longer take their baseline while the ring's setup signal is still driving it, and wait for those drives instead of sleeping past them. -> [completed 2026-10-18](COMPLETED-CHECKLIST.md#tw-106)

- [x] **TW-10.7** -- Two public documentation claims are corrected to what the code does: `Terminal` says its `last_known` and `win32_code` mean something different for a failed read than for a failed probe, and `TailReader` no longer promises that no wake follows a poll-done. -> [completed 2026-10-18](COMPLETED-CHECKLIST.md#tw-107)

- [x] **TW-10.8** -- `ReadSq::capacity` no longer takes the lock a `ReadDrain` holds, so asking for the capacity while draining does not wait for the drain, on the thread that holds it or on another. -> [completed 2026-10-18](COMPLETED-CHECKLIST.md#tw-108)

- [x] **TW-10.9** -- A design-notes bullet that an earlier commit joined onto the end of the one before it is on its own line again. -> [completed 2026-10-18](COMPLETED-CHECKLIST.md#tw-109)

- [x] **TW-10.10** -- The public `TailReader` example rings `ring_resume` after it drains, as a reader stopped on a full queue needs, instead of teaching a loop that stalls on a poll longer than the queue. -> [completed 2026-10-18](COMPLETED-CHECKLIST.md#tw-1010)

- [x] **TW-10.11** -- `CreateError`, which is now the error of the watcher's queue and of the reading layer's client queue, words its messages for a submission queue and not for the watcher. -> [completed 2026-10-18](COMPLETED-CHECKLIST.md#tw-1011)

- [x] **TW-10.12** -- The README says a data entry carries the value, range and bytes, and what the other entries carry, instead of saying every entry does. -> [completed 2026-10-18](COMPLETED-CHECKLIST.md#tw-1012)
