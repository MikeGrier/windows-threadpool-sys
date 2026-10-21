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

## DI-M19 -- Paired begin and end events with correlation ids

Queued by the engineer, 2026-10-21 ([DI-D-80](DESIGN-NOTES.md#di-d-80)): durations are computed by the analysis tools from
paired begin and end events, which the sinks can all do given a correlation id and a reliable pairing, and no component
such as the router builds timing of its own. A correlation id is scoped to a process, and is a `u64` minted from an atomic
with relaxed increments. The shipped delay events ([DI-12.1](COMPLETED-CHECKLIST.md#di-121)) carry no id; they report a flush's
issue once for a whole seal and its answer once for each part; and the ETW write takes the current thread's activity id,
which a begin and an end on different threads do not share, since a provider answers from any thread.

Ruled by the engineer, 2026-10-21, and recorded in [DI-D-80](DESIGN-NOTES.md#di-d-80):
- there are pairs for seals and for the weightier actions such as a flush, and a sink may decline any pair; a flush is
  asynchronous in the provider, as with `io_uring`, so its pairing matters;
- an end carries no `elapsed`, and a sink that wants a duration of its own is enhanced in the sink, not in the core or the
  router;
- the pairs are a seal, a flush of a file or a domain, and a lock wait;
- a seal's pair runs from its being asked for to its becoming durable or failing;
- a lock wait's end is written with the lock held, and a sink's synchronization must not interfere with dioring's.

A working position, not decided: the 128-bit activity id is the process id in the upper 64 bits and the correlation id in the
lower, made where the id is written, in the ETW provider, since the portable core has only the `u64`. Open, and met in
`DI-19.4`: whether a process id reused within one trace makes two processes' ids the same, which a counter that starts again
would allow.

- [x] **DI-19.1** -- `CorrelationId`, a process-scoped `u64` from one relaxed atomic counter, minted only by `CorrelationId::for_pair`, which asks the sink first. -> [completed 2026-10-21](COMPLETED-CHECKLIST.md#di-191)

- [ ] **DI-19.2** -- **Begin and end events in the core, and one oracle for their pairing.** Restructured on 2026-10-21 into
  `DI-19.2.1` to `DI-19.2.5`, with the engineer's rulings recorded in [DI-D-87](DESIGN-NOTES.md#di-d-87): the event shapes
  below, the ETW schema's opcodes moved here from `DI-19.4` because changing `Diagnostic` breaks the provider's crate in the
  same commit, and the core's duration code removed. It is done when they all are. As written before the restructure:
  three pairs, each one kind of event, so a sink wants a pair or declines it and cannot want a begin and not its end -- a
  seal, from its being asked for to its becoming durable or failing; each flush of a file or a domain, from its issue to its
  answer; and a lock wait, whose begin is written when the immediate try fails and whose end is written as the lock is
  acquired, with the lock held. An end is reported if and only if its begin was, whatever the sink wants by then, and on
  every path a begin can leave by; any it cannot be reported on is documented. The pairing rule is defined once, as a
  checker in `durable-ioring` that every sink's tests use and do not restate. The text of every event carries its id.

  The shapes (DI-D-87): the kinds are `Seal`, `Flush`, `LockWait` and `Inconsistency`, and `FlushIssued`, `FlushAnswered`
  and `SealWaited` go, a seal's wait being the gap between its begin and its first flush's. `SealBegan { instance, lineage,
  through, id }` and `SealEnded { ..., id, outcome }`, the outcome how the seal was answered -- durable, failed, blocked by
  an earlier failure -- or ended unanswered, by its lineage ending or its instance being dropped. `FlushBegan { instance,
  lineage, through, part, id }` and `FlushEnded { ..., id, outcome }`, succeeded, failed or abandoned. `LockWaitBegan {
  instance, lock, id }` and `LockWaitEnded { instance, lock, id }`. An end repeats its begin's identity fields.

- [x] **DI-19.2.1** -- The seal, flush and lock-wait pair variants and kinds are in `Diagnostic` beside the old ones, `Diagnostic::pairing` says which end of which pair an event is, and `PairingChecker` holds the pairing rule. -> [completed 2026-10-21](COMPLETED-CHECKLIST.md#di-1921)

- [ ] **DI-19.2.2** -- **The core emits the seal and flush pairs.** In place of `SealWaited`, `FlushIssued` and
  `FlushAnswered`, each pair's id minted by `CorrelationId::for_pair` as its begin is built. An end on every path a begin can
  leave by: an answer, a failure, a completion dropped unanswered, a lineage that ends or retires, and the core closed or
  dropped; any that cannot be reached is documented at its definition. Tested over the in-memory backend and its script,
  through the checker.

- [ ] **DI-19.2.3** -- **The lock-wait pair, and the sink's leaf-lock contract.** `LockWatch` writes the begin when the
  immediate try fails and the end on acquisition, with the lock held, and its timer goes. `DiagnosticSink`'s contract says a
  sink is called with dioring's lock held, so its own synchronization is leaf-only: never taken in a way that can wait on
  anything dioring's locks guard, and never held by a thread while it calls into an instance, a consumer holding a sink's
  lock around such a call included. The shipped sinks are checked against it and the result recorded; a test sink that
  records under a mutex is shown to keep to it.

- [ ] **DI-19.2.4** -- **The sinks follow, and the old variants go.** The standard-error and debugger lines carry the id.
  `durable-ioring-events`' table maps the pairs to events with Start and Stop opcodes, the manifest is rendered and held
  equal, and it is verified as `DI-12.1.5` was, compiled, installed and read back by `tracerpt`; the activity id is
  `DI-19.4`'s. The driver's tests bind to the checker. `FlushIssued`, `FlushAnswered` and `SealWaited` are removed, and
  `StampClock::elapsed` with them (DI-D-87). This is the major change to `durable-ioring`.

- [ ] **DI-19.2.5** -- **The sweep, the sabotage and the record.** A sweep for the delay events' old shape, of `DI-D-26`,
  `DI-D-51`, `DI-D-77`, the README and the crates' docs, is recorded with it; sabotage shows the checker is the one place
  the pairing rule lives; and the decisions the steps made are recorded.

- [ ] **DI-19.3** -- **`win-etw-sys` writes an event with an explicit activity id.** `EventWriteTransfer` with an activity
  id, and a related one, taken from the caller and not from the thread, so a begin and an end written on different threads
  carry the same one. The call's page is read first, and its conditions on the arguments are the only ones refused. A session
  test, elevated and `#[ignore]`d as the crate's others are, reads the activity id of a record written from two threads.

- [ ] **DI-19.4** -- **The ETW provider writes the activity id.** Waits on `DI-19.2` and `DI-19.3`. The Start and Stop
  opcodes moved to `DI-19.2.4` (DI-D-87); this step writes each pair's activity id by the mapping the section above holds as
  a working position, and is verified as `DI-12.1.5` was: compiled and installed, then a trace recorded and read by
  `tracerpt`, which this time must show the begin and end paired and their duration computed.

## DI-M20 -- Durability of a directory's entries

Opened 2026-10-21 from [the design session](../../design-sessions/DESIGN-SESSION-2026-10-21-router-domains-and-paired-events.md#directory-entries):
a database that creates a data file by name at run time needs the creation, and so the directory entry, durable before it
relies on the file, and nothing in [CONTRACT.md](CONTRACT.md) or [API.md](API.md) says whether a seal covers it. Linux
requires an `fsync` of the directory; what Windows requires is not known to the assistant. Kept apart from the router by
the engineer's agreement, 2026-10-21. Beyond the first two items this milestone is deliberately coarse: what is built, if
anything, follows from what the pages say.

- [ ] **DI-20.1** -- **Read what Windows documents about a directory entry's durability, and record it.** The pages for
  `CreateFileW`, `FlushFileBuffers`, `MoveFileExW` (with `MOVEFILE_WRITE_THROUGH`), `SetFileInformationByHandle`'s rename
  and disposition classes, and anything they lead to, each read for: whether flushing a new file makes its directory entry
  durable; whether a directory handle may be flushed, and what that commits; and what a rename or a delete needs. Each
  finding is recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md) with its page and the date read, and silence is recorded as
  silence. No `Nt*` page is a source ([the rule](../../.github/copilot-instructions.md)). If the pages leave a question open,
  the experiment that would answer it is described, and is evidence, never a check.

- [ ] **DI-20.2** -- **Bring the findings to the engineer for a design discussion.** Whether dioring makes a directory
  entry durable, whether a consumer must, and whether a directory is a target a seal can cover, are decided there; the work
  that follows is queued from that discussion and not before it.

## DI-M21 -- A sample log on durable-ioring

Queued by the engineer, 2026-10-21 ([DI-D-85](DESIGN-NOTES.md#di-d-85)): survey common practice in database and file-system
logs and provide a log built on the durable ring as a sample; once it proves out it is a candidate to promote, which is parked
as [DI-21+.1](PARKED.md#di-m21----promoting-the-sample-log-parked-behind-di-m21). It does not start from the `epoch_log`
sample, which it may cite only incidentally. The engineer: "I don't see it as being important to me personally in the
immediate future" -- a statement of priority, not a deferral. Beyond the first two items this milestone is deliberately
coarse: what the sample is follows from the survey.

- [ ] **DI-21.1** -- **Survey common practice in database and file-system logs.** Write-ahead logging and its recovery
  (redo and undo, log sequence numbers and a page's own LSN, fuzzy checkpoints, compensation records -- ARIES and what
  followed it); file-system journaling (physical block journals and their ordering modes, metadata-only journals, a log
  with restart areas); and what every log faces: group commit, detecting a torn or partial record, a record's identity
  across a crash, reclaiming log space, and how a log and the data it protects are ordered. Each point is cited to its
  source, published papers and the systems' own documentation, and recorded as a design session in
  [design-sessions/](../../design-sessions/). The survey says what is common to both kinds of log, what differs and why,
  and, for each, which `durable-ioring` facility a log built on it would use -- epochs and seals, lineages, gates across
  lineages, flush domains, failures and their suspects -- or that none does.

- [ ] **DI-21.2** -- **Bring the survey to the engineer, to settle the sample's scope.** Which practices the sample
  shows, which it names and leaves out, and where it lives -- an example of `win-durable-ioring`, or a portable example of
  `durable-ioring` over its in-memory backend that a Windows run also exercises -- are decided there. A gap the survey
  finds in `durable-ioring` is queued as work on the crate, not worked around in the sample.

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
