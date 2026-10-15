# Checklist: win-multi-file-tail-watcher

Reports length changes across a large population of files, on a beat the caller owns. See
[DESIGN-NOTES.md](DESIGN-NOTES.md) for the decisions every item cites, and
[COMPONENT.md](COMPONENT.md) for the crate's state before `TW-1.1`.

Each milestone ends in a deliverable exercised by integration tests: every one of them crosses a
real file system, process, or thread-pool boundary. Unit tests accompany each item as usual.

## M1 -- Register one file and read its length through a held handle

Deliverable: a file, written to by a child process, is opened by path and its growing length is
read through the held handle.

- [ ] **TW-1.1** -- **Create the crate.** `Cargo.toml` inheriting the workspace package fields,
  `src/lib.rs` with the copyright line and crate docs, membership in the workspace, and an entry in
  `release-please-config.json` and `.release-please-manifest.json` matching the other `win-` crates.
  Dependencies: `windows-sys`, [wtf-string](../wtf-string/),
  [windows-overlapped-io-sys](../windows-overlapped-io-sys/) with its `fs` feature. Replace
  [COMPONENT.md](COMPONENT.md) with a `README.md`.

- [ ] **TW-1.2** -- **Open a file by `Wtf16String` path as an endpoint, and take an observer.**
  Read-only, sharing read, write and delete, with `FILE_FLAG_OVERLAPPED`; the endpoint is kept for
  the reads iteration. A failed open is an error carrying the Win32 error code.
  ([TW-D-3](DESIGN-NOTES.md#tw-d-3), [TW-D-10](DESIGN-NOTES.md#tw-d-10))

  > **CROSS-COMPONENT PREREQUISITE:** `crates/windows-overlapped-io-sys` -> `M15` -> `M15.5`
  > (endpoints share their handle with observers, with the observer's length query and tests) must
  > land first. Landed; see that crate's
  > [COMPLETED-CHECKLIST.md](../windows-overlapped-io-sys/COMPLETED-CHECKLIST.md#m155).

- [ ] **TW-1.3** -- **Read the current length through an observer.** ([TW-D-2](DESIGN-NOTES.md#tw-d-2))

- [ ] **TW-1.4** -- **Validate the starting offset at open.** An offset beyond the length read at
  open is a registration error with its own `Err` variant, distinct from an open failure.
  ([TW-D-3](DESIGN-NOTES.md#tw-d-3))

- [ ] **TW-1.5** -- **Integration tests: one file, real writers.** A child process whose stdout is
  redirected to the file grows it, and the held-handle length tracks each write; a shrink by
  another handle is reported as the smaller length; a missing path yields its Win32 error code; an
  offset past the length yields the distinct error; another process can still write to and delete
  the file while the handle is held.

## M2 -- A poll over many files reports attentions through the SQ

Deliverable: a watcher over hundreds of files, polled once, puts exactly the changed files into the
SQ with correct last known and current lengths, then a poll-done entry, and rings the doorbell.

- [ ] **TW-2.1** -- **The watcher and its roster.** `Watcher<T: Clone>`; registration moves a `T` in
  and returns an RAII value whose `Drop` unregisters; no uniqueness and no lookup by name.
  ([TW-D-3](DESIGN-NOTES.md#tw-d-3), [TW-D-8](DESIGN-NOTES.md#tw-d-8))

- [ ] **TW-2.2** -- **The SQ.** Created by the crate with a capacity set by the caller; two entry
  kinds, an attention (`T` clone, last known length, current length) and a poll-done entry; a
  drain operation for the caller. ([TW-D-4](DESIGN-NOTES.md#tw-d-4), [TW-D-6](DESIGN-NOTES.md#tw-d-6))

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
from any state, and the subsequent `Drop` returns without waiting.

- [ ] **TW-4.1** -- **The shutdown request**: non-blocking, idempotent, and `poll` refused after it.
  ([TW-D-9](DESIGN-NOTES.md#tw-d-9))

- [ ] **TW-4.2** -- **Stopping the poll**: set the stop flag, then stop and drain the resume wait; a
  queued resume callback returns at once on seeing the flag, and a running poll stops at its next
  roster entry. The poll-done entry is written as normal when there is space.
  ([TW-D-9](DESIGN-NOTES.md#tw-d-9))

- [ ] **TW-4.3** -- **`Drop` of the watcher**: nothing to wait for after shutdown; without a prior
  shutdown, blocks until the crate's own callbacks have finished; never depends on the poll-done
  entry having been written or read. ([TW-D-9](DESIGN-NOTES.md#tw-d-9))

- [ ] **TW-4.4** -- **Integration tests: shutdown from every state.** From the caller's doorbell
  callback on a thread-pool wait; while parked on a full SQ; during a running poll over a large
  roster; repeated requests; `Drop` with and without a prior shutdown; no crate callback runs after
  `Drop` returns.

## M4+ -- Reads on the thread pool (parked behind M4)

Coarse on purpose: its scope is set when M4 is done.

- [ ] **TW-4+.1** -- **Bind each kept endpoint to the thread pool** (`ThreadpoolIo`), with probes
  continuing through observers. ([TW-D-10](DESIGN-NOTES.md#tw-d-10))

- [ ] **TW-4+.2** -- **Asynchronous reads of announced bytes, completed on the thread pool.**
  ([TW-D-11](DESIGN-NOTES.md#tw-d-11))
