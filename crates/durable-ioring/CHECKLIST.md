# Checklist: durable-ioring

What this component is: [COMPONENT.md](COMPONENT.md). Decisions: [DESIGN-NOTES.md](DESIGN-NOTES.md).
The session behind both: [DESIGN-SESSION-2026-10-05-epoch-ring.md](../../design-sessions/DESIGN-SESSION-2026-10-05-epoch-ring.md).

DI-M1, the contract, and DI-M2, the API shape, are complete -- see [CONTRACT.md](CONTRACT.md),
[API.md](API.md) and the archive in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md). Code began at
DI-M3, which closed on 2026-10-11 with its open work restructured into DI-M7 to DI-M12, flatter,
because its items had nested six levels deep; the table from its old IDs to the new ones is in [the
archive](COMPLETED-CHECKLIST.md#di-m3-restructured), and older records still cite the old IDs. The
new milestones are numbered after the parked DI-M5+ and DI-M6+, which are cited too widely to
renumber. The flush-failure spike (DI-M4) runs alongside and gates nothing. This file holds only work in
scope now; parked work, gated (`M{n}+`), on the horizon or deferred, is in [PARKED.md](PARKED.md).

## DI-M21 -- A sample log on durable-ioring

Queued by the engineer, 2026-10-21 ([DI-D-85](DESIGN-NOTES.md#di-d-85)): survey common practice in database and file-system
logs and provide a log built on the durable ring as a sample; once it proves out it is a candidate to promote, which is parked
as [DI-21+.1](PARKED.md#di-m21----promoting-the-sample-log-parked-behind-di-m21). It does not start from the `epoch_log`
sample, which it may cite only incidentally. The engineer: "I don't see it as being important to me personally in the
immediate future" -- a statement of priority, not a deferral. Beyond the first two items this milestone is deliberately
coarse: what the sample is follows from the survey.

- [x] **DI-21.1** -- The survey of common practice in database and file-system logs is recorded as a design session, extending the 2026-10-24 prior-art session, with each practice mapped to the durable-ioring facility a log would use or to none. -> [completed 2026-10-29](COMPLETED-CHECKLIST.md#di-211)

- [ ] **DI-21.2** -- **Bring the survey to the engineer, to settle the sample's scope.** The survey is
  [DESIGN-SESSION-2026-10-29-database-and-file-system-log-practice.md](../../design-sessions/DESIGN-SESSION-2026-10-29-database-and-file-system-log-practice.md);
  its section E maps each practice to a facility or to none, and its section F lists what bears on this item. Which practices the sample
  shows, which it names and leaves out, and where it lives -- an example of `win-durable-ioring`, or a portable example of
  `durable-ioring` over its in-memory backend that a Windows run also exercises -- are decided there. A gap the survey
  finds in `durable-ioring` is queued as work on the crate, not worked around in the sample.

  **Questions the survey brings with it, each open and each answerable with "too early to say"** (the resolution
  gradient: none is a decision until the engineer makes it):
  - **Where it lives, and how it stages a crash.** The in-memory backend keeps a target's `contents` apart from its
    `durable_contents`, so a portable example can stage a crash deterministically; on real Windows files a crash is hard
    to stage. That backend, and the oracle, are behind this crate's `std` feature.
  - **Whether the sample shows the allocate-at-construction discipline** of [DI-D-97](DESIGN-NOTES.md#di-d-97), which a
    `no_std` log would. This is a question of scope; nothing is queued for it.
  - **Who retains data for a rewrite.** A heal means the consumer rewrites what a failure put at risk, and retention is
    outside this crate ([COMPONENT.md](COMPONENT.md)). A log naturally keeps its own tail for it, so the sample forces the
    question of whether that retention is the log's own or belongs to the parked retention layer
    ([DI-5+.1](PARKED.md#di-m5----the-retention-layer-parked-behind-di-m12)), which also bears on what promoting the log
    ([DI-21+.1](PARKED.md#di-m21----promoting-the-sample-log-parked-behind-di-m21)) would mean.

- [ ] **DI-21.3** -- **Build the sample log** to the scope `DI-21.2` sets, with its own contract written first, tests,
  and a run that shows recovery after a crash it stages.

  > **-> UNBLOCKS PARKED WORK:** completing this unblocks `DI-M21+` in
  > [PARKED.md](PARKED.md#di-m21----promoting-the-sample-log-parked-behind-di-m21).

## DI-M4 -- Evidence: Windows' flush-failure behaviour (ungated; gates nothing)

- [ ] **DI-4.1** -- **The flush-failure spike** ([D-44](../windows-ioring-sys/DESIGN-NOTES.md#d-44)).
  Make a flush fail on a real Windows stack and observe whether a later flush covers, or hides, the
  earlier loss -- separately for buffered and `NO_BUFFERING` handles -- whether a rewrite plus a
  successful flush re-establishes durability, and whether a write-back failure is reported to every
  handle open on the file or only to one (which decides how often the external-failure import of
  DI-1.2 is needed), and whether another handle's failed flush on the same device observably loses
  this handle's unflushed `NO_BUFFERING` writes. Windows documents none of this; the contract is
  justified on the permitted response space and does not wait for the answer. Producing the failure
  on demand is the hard part (a detachable VHD or an error-injecting filter are the candidates).
  What the published record does say -- Windows' own "the data has been lost" write-behind codes,
  Microsoft assigning the handling to the application, and nothing on retention after a failed
  flush -- was gathered on 2026-10-09 and is in
  [the design session](../../design-sessions/DESIGN-SESSION-2026-10-05-epoch-ring.md#what-the-published-record-says-research-2026-10-09).
