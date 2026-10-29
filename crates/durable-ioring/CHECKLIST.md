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

## DI-M24 -- A dioring over the in-memory backend, and what a crash leaves

Queued by the engineer, 2026-10-29, for the sample log ([DI-D-116](DESIGN-NOTES.md#di-d-116)): "I would like to make this a
reality over the memory base dioring, so let's schedule whatever is needed for that so we can do a great deal of fault
injection testing/modeling". The only implementations of `DurableRing` today are the Windows driver and a test fake, so a
consumer cannot yet run portable code against a ring over the in-memory backend, and that backend has no model of what a
crash leaves on a target: a commit copies everything, and nothing is lost but what was never committed. `DI-21.5` onward
depends on this milestone; `DI-21.3` and `DI-21.4` do not.

- [ ] **DI-24.1** -- **A dioring over the in-memory backend.** A `DurableRing`, and a `RegisteredBufferRing` over the
  backend's registered buffers, in this crate behind its `std` feature, built from the driver pieces already here (`Engine`,
  `PumpedDelivery`, `Home` and `LineageSlots`) with pumped delivery, lineage handles, the provider path and the failure
  observer; its readiness is the in-memory notification and its clock is the consumer's `StampClock`. It is built by
  duplicating what `win-durable-ioring`'s `Dioring` does above its backend, under the workspace's duplicate-then-decide
  rule; the merge-or-delete decision is parked as
  [DI-21+.2](PARKED.md#di-m21----merge-or-delete-the-duplicated-driver-parked-behind-di-m21). Held to the conformance
  oracle as the Windows driver is, and its own contract statements (what it does that a Windows instance does not, such as
  completing on the doorbell) are in its rustdoc. Its name, and whether it offers a delivery other than pumped, are settled
  as it is built.

- [ ] **DI-24.2** -- **Fault injection through it.** An instance is built over a backend given a `FaultScript`, and the
  test reaches what the script does from outside: the completions it holds, releasing them, and what fired. Nothing new is
  injected here; this item makes what the in-memory backend already offers reachable through a ring, and its tests show
  each kind of fault arriving as the contract says (a failed commit as a failure with its suspects, a refused push as a
  refusal).

- [ ] **DI-24.3** -- **What a crash leaves.** The in-memory backend records, per target, the writes completed or in flight
  since the target's last successful commit, and gives the contents a crash could leave: each such write lost, applied, or
  torn, a torn write leaving each of its sectors old or new at a sector size the test names (SQLite's "linear" model, C2 of
  [the survey](../../design-sessions/DESIGN-SESSION-2026-10-29-database-and-file-system-log-practice.md)). Images are
  enumerated deterministically, within a bound the test sets; drawing them at random is not done without the engineer's
  approval, per the workspace's rule on reproducible unit tests. A test shows a durable write is never lost or torn in any
  image, and that every image is reachable.

- [ ] **DI-24.4** -- **Restart from a crash image.** A target can be made with given contents, and an instance can be built
  over targets made from a crash image, so a test runs a workload, crashes it at a point it names, and opens the result as a
  fresh instance. A test does that across a seal and checks, with the conformance oracle's record of what was reported
  durable, that every such write is present after restart.

- [ ] **DI-24.5** -- **Documented, with a compiled example**: [COMPONENT.md](COMPONENT.md), the crate's rustdoc, and a
  doctest of a workload crashed and restarted. CI's `portable` job runs these tests on Linux and macOS already, so nothing
  is added there.

## DI-M21 -- A sample log on durable-ioring

Queued by the engineer, 2026-10-21 ([DI-D-85](DESIGN-NOTES.md#di-d-85)): survey common practice in database and file-system
logs and provide a log built on the durable ring as a sample; once it proves out it is a candidate to promote, which is parked
as [DI-21+.1](PARKED.md#di-m21----promoting-the-sample-log-parked-behind-di-m21). It does not start from the `epoch_log`
sample, which it may cite only incidentally. The engineer: "I don't see it as being important to me personally in the
immediate future" -- a statement of priority, not a deferral. The sample's shape was settled on 2026-10-29
([DI-D-116](DESIGN-NOTES.md#di-d-116)): a ring of fixed-size segments, conservative by construction, running on every
dioring. The items after `DI-21.4` are written at the resolution available now and are expected to sharpen when they start.

- [x] **DI-21.1** -- The survey of common practice in database and file-system logs is recorded as a design session, extending the 2026-10-24 prior-art session, with each practice mapped to the durable-ioring facility a log would use or to none. -> [completed 2026-10-29](COMPLETED-CHECKLIST.md#di-211)

- [x] **DI-21.2** -- The sample's scope is settled: a ring of fixed-size segments with checked variable-length records, padded commits, zero-filled segments, fail-stop, and a gate step, running on every dioring (DI-D-116). -> [completed 2026-10-29](COMPLETED-CHECKLIST.md#di-212)

- [ ] **DI-21.3** -- **The log's contract, written first.** Settled with the engineer before it is written: the blocking API
  ([DI-D-116](DESIGN-NOTES.md#di-d-116)'s "Open"). The contract states what the log guarantees -- the records it returns on
  open are a prefix of those appended, holding every record reported durable -- what it requires of its caller, the
  conservative rules and why each is there, how segments are reused and when an append is refused, reservations and
  what they do and do not promise, reading by LSN, several clients' streams sharing one log and how a client is named and
  joins, how threads share it, the tail-pinning hazard it leaves to the caller, the seam a platform supplies, and **how to
  recover after a fail-stop** ("we do need to explain how to recover"): what reopening returns, what
  the caller does with records past what it was told was durable, and what to do when reopening itself fails. Its code
  examples compile, per the workspace's rule that prose containing code must.

- [ ] **DI-21.4** -- **The record format and the recovery scan, as code over bytes.** Encoding a record (with its
  client's stream and its LSN as a byte position, [DI-D-116](DESIGN-NOTES.md#di-d-116)), a segment header and a padded
  commit; finding a record from its LSN; and scanning segments to the end of the log, written with no ring beneath them, so they are tested in
  memory against hand-made damage: a torn final record, a stale record from a segment's previous generation, a bad CRC, a
  record shorter than its header says, a torn segment header, and segments found out of order.

- [ ] **DI-21.5** -- **The log over any dioring.** Settled with the engineer first: where the sample's code lives
  ([DI-D-116](DESIGN-NOTES.md#di-d-116)'s "Open"). Create, which zero-fills each segment and makes it durable through the
  ring; open, which recovers; append, commit, waiting for durability, `release_before`, and fail-stop, against
  `DurableRing` and the per-platform seam only. Run and tested on the in-memory dioring (`DI-M24`).

- [ ] **DI-21.5.1** -- **Reservations.** A client reserves room for records it must be able to write, an append drawn from
  a reservation is never refused for space, and an unreserved append is refused before it eats reserved room, with the
  cost set by the conservative rules ([DI-D-116](DESIGN-NOTES.md#di-d-116)). Tested at the boundary: the last reservable
  record, a record that would cross a segment, and a full log that refuses new work while reserved work finishes.

- [ ] **DI-21.5.2** -- **Reading by LSN** while the log runs: a committed record read back through the ring, a record in
  the current commit group, and a released one reported as released rather than read as another generation's bytes. A
  test walks one transaction's chain backwards through records of several interleaved transactions.

- [ ] **DI-21.5.3** -- **Several clients' streams, shared between threads.** Records tagged by stream, recovery handing
  each client its own, a release point and reservations per client with the log reclaiming only to the lowest, and the log
  shared behind a mutex. Threads waiting on different LSNs share the ring's work without holding the mutex while they
  wait, as the contract (`DI-21.3`) sets out.

- [ ] **DI-21.6** -- **The write-ahead rule through a gate.** Data writes in a second lineage, gated on the log's epoch, so
  the ring holds each until the log records it depends on are durable; shown with the ring holding a data write while the
  log's commit is outstanding, and releasing it after.

- [ ] **DI-21.7** -- **Fault injection and crash testing on the in-memory dioring.** A failed commit stops the log, and
  reopening recovers as the contract says, handing each client its own records, after which clients re-reserve; a crash at each commit point within a bound, with every image `DI-24.3` gives,
  reopens to a prefix of the records appended that holds every record reported durable; and the gate step never lets a
  data write reach the media ahead of the log records it was gated on.

- [ ] **DI-21.8** -- **The same log on Windows.** Over `win-durable-ioring`'s `Dioring` on real files: create, append,
  commit, reopen and recover, with the Windows side of the per-platform seam. Damage is not staged on real files; that is
  what `DI-21.4` and `DI-21.7` are for.

  > **-> UNBLOCKS PARKED WORK:** completing this unblocks `DI-M21+` in
  > [PARKED.md](PARKED.md#di-m21----promoting-the-sample-log-parked-behind-di-m21), and the merge-or-delete decision in
  > [PARKED.md](PARKED.md#di-m21----merge-or-delete-the-duplicated-driver-parked-behind-di-m21).

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
