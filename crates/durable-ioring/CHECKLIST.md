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

## DI-M20 -- Durability of a directory's entries

Opened 2026-10-21 from [the design session](../../design-sessions/DESIGN-SESSION-2026-10-21-router-domains-and-paired-events.md#directory-entries):
a database that creates a data file by name at run time needs the creation, and so the directory entry, durable before it
relies on the file, and nothing in [CONTRACT.md](CONTRACT.md) or [API.md](API.md) says whether a seal covers it. Linux
requires an `fsync` of the directory; what Windows requires is not known to the assistant. Kept apart from the router by
the engineer's agreement, 2026-10-21. Beyond the first two items this milestone is deliberately coarse: what is built, if
anything, follows from what the pages say.

- [x] **DI-20.1** -- What Windows documents about a directory entry's durability -- the pages for creating, flushing, renaming and deleting, and what they lead to -- is read and recorded with each page's date, silence as silence, and the experiment the open questions call for is described. -> [completed 2026-10-21](COMPLETED-CHECKLIST.md#di-201)

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
