# Checklist: win-multi-file-tail-watcher

Reports length changes across a large population of files, on a beat the caller owns. See
[DESIGN-NOTES.md](DESIGN-NOTES.md) for the decisions every item cites, and
[README.md](README.md) for what the crate is.

Each milestone ends in a deliverable exercised by integration tests: every one of them crosses a
real file system, process, or thread-pool boundary. Unit tests accompany each item as usual.

## M2 -- A poll over many files reports attentions through the SQ

Deliverable: a watcher over hundreds of files, polled once, puts exactly the changed files into the
SQ with correct last known and current lengths, then a poll-done entry, and rings the doorbell.

- [x] **TW-2.1** -- `Watcher<T: Clone>` registers files into a roster and returns an RAII `Registration` whose `Drop` unregisters. -> [completed 2026-10-15](COMPLETED-CHECKLIST.md#tw-21)

- [x] **TW-2.2** -- The watcher owns a bounded submission queue of attention and poll-done entries, set at `Watcher::new` and drained by the caller. -> [completed 2026-10-15](COMPLETED-CHECKLIST.md#tw-22)

- [ ] **TW-2.3** -- **The two doorbells**: crate-created manual-reset events, crate to caller and
  caller to crate, each exposed as a waitable handle. ([TW-D-6](DESIGN-NOTES.md#tw-d-6))

- [ ] **TW-2.4** -- **`poll` initiates a poll on the thread pool and returns.** The poll walks the
  roster, compares each length with the last reported one, writes an attention for each change
  and then the poll-done entry, and rings the crate-to-caller doorbell. A second `poll` while one is
  in flight is refused at run time. In this milestone the SQ is sized so it cannot fill.
  ([TW-D-4](DESIGN-NOTES.md#tw-d-4), [TW-D-5](DESIGN-NOTES.md#tw-d-5))

- [ ] **TW-2.5** -- **Integration tests: a population of real files.** Hundreds of files, some
  written by child processes between polls: only changed files are reported, with correct pairs;
  growth and shrink are both reported; a second poll with no writes reports only poll-done; a
  concurrent `poll` is refused; after poll-done no crate callback runs.

## M3 -- Full SQ, resume, and a roster that changes during a poll

Deliverable: a population much larger than the SQ is fully reported across repeated stop/resume
cycles, while registrations are added and dropped mid-poll, with no hang and no lock held across
I/O.

- [ ] **TW-3.1** -- **Stop on a full SQ.** Record the roster position, ring the doorbell, stop
  without waiting; no lock held across a probe, an SQ write, or a doorbell ring. Settle the
  working position on roster changes between stop and resume and record it in
  [DESIGN-NOTES.md](DESIGN-NOTES.md). ([TW-D-7](DESIGN-NOTES.md#tw-d-7))

- [ ] **TW-3.2** -- **Resume on the caller's doorbell** through a thread-pool wait the crate
  registers, continuing from the recorded position. Confirm or replace that working position, and
  record the outcome in [DESIGN-NOTES.md](DESIGN-NOTES.md). ([TW-D-5](DESIGN-NOTES.md#tw-d-5),
  [TW-D-7](DESIGN-NOTES.md#tw-d-7))

- [ ] **TW-3.3** -- **Registration and unregistration during a poll.** Dropping a registration
  mid-poll neither waits nor invalidates attentions already written; a registration made mid-poll
  may or may not be seen. ([TW-D-8](DESIGN-NOTES.md#tw-d-8))

- [ ] **TW-3.4** -- **A benchmark of one full poll against population size**, up to the expected
  top end, so the crate can observe its own cost. It informs the thread count, which
  [TW-D-5](DESIGN-NOTES.md#tw-d-5) leaves open; sharding is deliberately not scheduled until a
  measurement calls for it. Comparison against per-file timers is out of scope
  ([TW-D-1](DESIGN-NOTES.md#tw-d-1)).

- [ ] **TW-3.5** -- **Integration tests: stop, resume, churn.** An SQ of one or two slots against
  hundreds of changing files: every change is reported across resumes; registrations dropped and
  added mid-poll cause no failure and no hang; attentions for dropped registrations carry a usable
  `T`; between polls no crate callback runs.

## M4 -- Shutdown

Deliverable: shutdown requested from inside the caller's own doorbell callback stops the watcher
from any state without blocking, and the subsequent `Drop` waits at most for a running poll to reach
its next roster entry.

- [ ] **TW-4.1** -- **The shutdown request**: non-blocking, idempotent, and `poll` refused after it.
  ([TW-D-9](DESIGN-NOTES.md#tw-d-9))

- [ ] **TW-4.2** -- **Stopping the poll**: the request sets the stop flag and returns without
  draining; a queued resume callback returns at once on seeing the flag, and a running poll stops at
  its next roster entry. The poll-done entry is written as normal when there is space.
  ([TW-D-9](DESIGN-NOTES.md#tw-d-9))

- [ ] **TW-4.3** -- **`Drop` of the watcher**: sets the stop flag if no request was made, then stops
  and drains the resume wait and waits for a running poll to return; never depends on the poll-done
  entry having been written or read. ([TW-D-9](DESIGN-NOTES.md#tw-d-9))

- [ ] **TW-4.4** -- **Integration tests: shutdown from every state.** From the caller's doorbell
  callback on a thread-pool wait, asserting the request returns while a poll is inside a probe;
  while parked on a full SQ; during a running poll over a large roster; repeated requests; `Drop`
  with and without a prior shutdown, each bounded by one probe; no crate callback runs after `Drop`
  returns.

## M4+ -- Reads on the thread pool (parked behind M4)

Coarse on purpose: its scope is set when M4 is done.

- [ ] **TW-4+.1** -- **Bind each kept endpoint to the thread pool** (`ThreadpoolIo`), with probes
  continuing through observers. ([TW-D-10](DESIGN-NOTES.md#tw-d-10))

- [ ] **TW-4+.2** -- **Asynchronous reads of announced bytes, completed on the thread pool.**
  ([TW-D-11](DESIGN-NOTES.md#tw-d-11))
