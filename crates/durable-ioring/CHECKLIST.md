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

## DI-M23 -- No `std`, and every allocation made at construction

The engineer, 2026-10-22, for a no-std user-mode file-system driver stack: [DI-D-97](DESIGN-NOTES.md#di-d-97) to
[DI-D-107](DESIGN-NOTES.md#di-d-107), from
[DESIGN-SESSION-2026-10-22-durable-ioring-no-std-and-preallocation.md](../../design-sessions/DESIGN-SESSION-2026-10-22-durable-ioring-no-std-and-preallocation.md).
Sharp through `DI-23.9`; the items after it are coarse on purpose, since the designs they depend on are open in the session
and `DI-23.10` is where they are settled. **No allocation is analysed or removed before `DI-23.2` is done**: the engineer,
2026-10-23, "I don't want to do further analysis without tagging".

- [x] **DI-23.1** -- Allocations in the core are measured, phase by phase, by a counting global allocator. -> [completed 2026-10-23](COMPLETED-CHECKLIST.md#di-231)

- [x] **DI-23.2** -- Every allocation the core makes is tagged by the call that makes it, and the allocation test counts by tag. -> [completed 2026-10-23](COMPLETED-CHECKLIST.md#di-232)

- [x] **DI-23.3** -- Every std path with a core or alloc equivalent is named by it, here and in io-buffers, and StderrDiagnostics is behind a default std feature. -> [completed 2026-10-23](COMPLETED-CHECKLIST.md#di-233)

- [x] **DI-23.4** -- A fault's report is a static or a shared dyn Error, shared through rc-errors, in three steps. -> [completed 2026-10-23](COMPLETED-CHECKLIST.md#di-234)

- [x] **DI-23.4.1** -- The rc-errors crate is created, registered, and given its decisions and its first milestone. -> [completed 2026-10-23](COMPLETED-CHECKLIST.md#di-2341)

- [x] **DI-23.4.2** -- The core's faults carry the report, and io::Error leaves the portable boundary. -> [completed 2026-10-23](COMPLETED-CHECKLIST.md#di-2342)

- [x] **DI-23.4.3** -- win-durable-ioring reports through the report, landed with DI-23.4.2. -> [completed 2026-10-23](COMPLETED-CHECKLIST.md#di-2343)

- [ ] **DI-23.5** -- **Capacity declared at construction, and construction fallible** ([DI-D-97](DESIGN-NOTES.md#di-d-97),
  [DI-D-98](DESIGN-NOTES.md#di-d-98)). Re-planned 2026-10-23: the tables are fixed-capacity and preallocated
  ([DI-D-106](DESIGN-NOTES.md#di-d-106)), so the crate that provides them comes first.

- [ ] **DI-23.5.1** -- **Create `crates/fixed-hash-table`**, `no_std` and allocation-free by default, with generics for clients
  that want `std` and `alloc`: a workspace member registered for release and publication, with its own `COMPONENT.md`,
  `DESIGN-NOTES.md`, `CHECKLIST.md` and `PLANS.md` row. Its first milestone, which this item writes, holds a fixed-capacity
  open-addressed map that refuses an insert when full, with the hasher a `BuildHasher` type parameter, the usual collection
  affordances, and their tests; its design notes settle the storage generics, the default hasher, and which affordances it
  keeps.

  > **-> CROSS-COMPONENT HANDOFF:** next work is in component `crates/fixed-hash-table` -> its first milestone, written by
  > this item. Control returns here at `DI-23.5.2`.

- [ ] **DI-23.5.2** -- **Declare capacity and refuse past it** ([DI-D-107](DESIGN-NOTES.md#di-d-107)): `operations`,
  `lineages`, `imports` and `targets`, given at construction, which becomes fallible; the queue and observed-failure storage
  derived from them; a push, a mint, an import and an `add_target` refused at capacity; the core's keyed tables moved onto
  `fixed-hash-table`. Breaking for this crate and `win-durable-ioring`.

  > **CROSS-COMPONENT PREREQUISITE:** `crates/fixed-hash-table`' first milestone must land first.

- [ ] **DI-23.6** -- **Charge a seal's work to the (lineage, file, seal span)** ([DI-D-98](DESIGN-NOTES.md#di-d-98)): the
  first write to a file in its lineage's unsealed span pays for the flush, the flush pair and the provider's parts; the
  charge stays with the span's entry while any write to the file remains in it.

- [ ] **DI-23.7** -- **A seal that covers no writes makes no record** ([DI-D-99](DESIGN-NOTES.md#di-d-99)), held in the
  trailing field and the per-lineage slot, releasing heals and the mark in order, with an overwritten one's pair ended as
  `Superseded`.

- [ ] **DI-23.8** -- **A failure holds its suspect writes' capacity until resolved** ([DI-D-100](DESIGN-NOTES.md#di-d-100)), and
  [CONTRACT.md](CONTRACT.md) and the crate's documentation say so with worked examples of a consumer refused at submit and
  resolving its failures to clear it.

- [ ] **DI-23.9** -- **Coalesce `Durable`** ([DI-D-101](DESIGN-NOTES.md#di-d-101)): remove the older un-popped entry and append
  the new one at the tail; the oracle and the tests that expect one `Durable` per seal follow.

- [ ] **DI-23.10** -- **Settle the open designs with the engineer**, as the session lists them: the storage a failure holds and
  how events and the inventory lend out its suspect set and markings; the `Arc`-shared contract types; the ordered tables;
  `Mutex` and `thread::panicking()` without `std`; whether a portable `ErrorKind` is wanted; and the shared report a refusal
  carrying a backend fault allocates ([DI-D-105](DESIGN-NOTES.md#di-d-105)'s "what building it settled").

- [ ] **DI-23.11** -- **The clean paths hand out no allocated collection** ([DI-D-103](DESIGN-NOTES.md#di-d-103)): visitors for
  provider requests, lineages and `close()`, and visitor queries behind `IoTargetBusy` and `LineageBusy`.

- [ ] **DI-23.12** -- **Implement what `DI-23.10` settles**, until `DI-23.1` asserts zero allocations after construction in
  every phase.

- [ ] **DI-23.13** -- **Build without `std` in CI**: this crate with `--no-default-features` for a target that has no `std`.

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
