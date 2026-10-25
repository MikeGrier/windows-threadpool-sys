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

- [x] **DI-23.5** -- Capacity is declared at construction and construction is fallible, over fixed-capacity preallocated tables, in the crate `DI-23.5.1` made and the steps of `DI-23.5.2`. -> [completed 2026-10-24](COMPLETED-CHECKLIST.md#di-235)

- [x] **DI-23.5.1** -- The fixed-hash-table crate is created, registered, and given its decisions and its first milestone. -> [completed 2026-10-23](COMPLETED-CHECKLIST.md#di-2351)

  > **-> CROSS-COMPONENT HANDOFF:** next work is in component `crates/fixed-hash-table` -> `FHT-M1` -> `FHT-1.1` (the map's
  > core). See [CHECKLIST.md](../fixed-hash-table/CHECKLIST.md). Control returns here at `DI-23.6`.

- [x] **DI-23.6** -- A seal's work on a file is charged to a (lineage, file, seal span) entry, made at push and released with its last uncovered write. -> [completed 2026-10-23](COMPLETED-CHECKLIST.md#di-236)

- [x] **DI-23.7** -- A seal that covers no writes makes no record, kept in a slot, evaluated in order, and overwritten by a later one whose pair ends Superseded. -> [completed 2026-10-23](COMPLETED-CHECKLIST.md#di-237)

- [x] **DI-23.8.1** -- An operation holds its place in the window until its last holder lets go: its completion popped, the write covered, and each failure suspecting it resolved. -> [completed 2026-10-23](COMPLETED-CHECKLIST.md#di-2381)

- [x] **DI-23.9** -- A lineage has at most one un-popped Durable: a later one removes the older and goes to the tail. -> [completed 2026-10-24](COMPLETED-CHECKLIST.md#di-239)

- [x] **DI-23.5.2** -- Capacity is declared and refused past, in three steps: a flush's places outlive its lineage, the declaration and its refusals, and the core's tables on `fixed-hash-table`. -> [completed 2026-10-24](COMPLETED-CHECKLIST.md#di-2352)

- [x] **DI-23.5.2.1** -- A flush's places outlive its lineage: ending a lineage keeps the writes named to a seal whose flush is out, with their span entries and places, until it answers. -> [completed 2026-10-24](COMPLETED-CHECKLIST.md#di-23521)

- [x] **DI-23.5.2.2** -- A `Capacity` of five counts, with no library default, is declared at construction, and a push, a mint, an import and an `add_target` are refused past it. -> [completed 2026-10-24](COMPLETED-CHECKLIST.md#di-23522)

- [x] **DI-23.5.2.3** -- The core's keyed tables are `fixed-hash-table` tables made when it is built, each sized from the declared counts, so nothing hashed allocates after construction. -> [completed 2026-10-24](COMPLETED-CHECKLIST.md#di-23523)

- [x] **DI-23.8.2** -- Capacity, its refusals, and the places unresolved failures hold are in the contract and the documentation, with compiled worked examples of a push refused at capacity and cleared by a seal and by resolving a failure. -> [completed 2026-10-24](COMPLETED-CHECKLIST.md#di-2382)

- [x] **DI-23.10** -- The open designs are settled: a failure's storage and the shared handles (DI-D-108), and what arrives unasked bounded by derivation with the rest (DI-D-109). -> [completed 2026-10-24](COMPLETED-CHECKLIST.md#di-2310)

- [x] **DI-23.11** -- The clean paths hand out no allocated collection: provider requests, lineages and `close()` go to visitors, and the busy refusals count what `target_holds` and `lineage_holds` name. -> [completed 2026-10-24](COMPLETED-CHECKLIST.md#di-2311)

- [ ] **DI-23.12** -- **Implement what `DI-23.10` settles** ([DI-D-108](DESIGN-NOTES.md#di-d-108),
  [DI-D-109](DESIGN-NOTES.md#di-d-109)), until `DI-23.1` asserts zero allocations after construction in every phase. Split
  2026-10-24 into the steps below, in dependency order; each is said in CONTRACT.md and the documentation as it lands, and
  each one's phases of the allocation test reach zero with it. Steps after the first are written at the resolution
  available now and are expected to sharpen, or split again, when they start.

- [x] **DI-23.12.1** -- The ordered tables, the seal queue among them, are on fixed storage of the crate's own, and a recorded seal holds a place of the window until the mark passes it (DI-D-110). -> [completed 2026-10-24](COMPLETED-CHECKLIST.md#di-23121)

- [x] **DI-23.12.2** -- The core's per-call scratch and the lists a call hands the driver are made at construction, each for a stated bound, and `CoreActions` carries no lists (DI-D-111). -> [completed 2026-10-24](COMPLETED-CHECKLIST.md#di-23122)

- [ ] **DI-23.12.3** -- **A failure's storage** ([DI-D-108](DESIGN-NOTES.md#di-d-108),
  [DI-D-109](DESIGN-NOTES.md#di-d-109)): the suspect set as the rule that made it -- the cut, the reach, the lineage --
  over write records that keep where they stopped being uncovered and are kept while a failure could name them; a marking
  kept once per write; the failure's records lent by a scoped borrow, events and the inventory carrying its identity,
  token, cause and stamp; a resolved failure's records kept until its `Abandoned` or `Healed` is popped; a `Marked` queued
  once per marked write and expanded at pop; and the failure table, the completion queue, and the list of entries one
  change makes (`Due`'s events, made at construction by `DI-23.12.2` for the clean paths only) sized from the derived
  bounds, the entries list pushed onto within its room as the others are. Proof obligations: unresolved observed failures within `operations` times one plus the served domains, a
  provider's answer for an ended lineage included, and a test filling the table and the queue to their bounds. Breaking:
  `Failed`, `Abandoned`, `Healed`, `Marked`, `FailureInfo` and `SuspectSet` change shape in both crates, and the oracle
  follows. Split 2026-10-24 when it began into the steps below, in dependency order: today a covered write's record is let
  go at the moment its marking is made, which is why each failure keeps its own copies, so the records must outlive
  coverage before a failure can be a rule over them, and the consumer-facing change comes last, over storage already in
  place.

- [x] **DI-23.12.3.1** -- The unresolved failures are kept in a table made at construction for the flush parts plus the imports, with the bound argued and filled by a test. -> [completed 2026-10-24](COMPLETED-CHECKLIST.md#di-231231)

- [ ] **DI-23.12.3.2** -- **Write records kept while a failure could name them, each with its marking once**
  ([DI-D-108](DESIGN-NOTES.md#di-d-108)): a write that stops being uncovered -- covered, nullified, or let go with its
  lineage -- keeps its record, marked with where it stopped and, once, with its marking, for as long as an unresolved
  failure suspects it; the table holds at most a record for each place, which the places already bound. A failure's
  markings are read from its suspects' records in observation order, so `mark` writes one marking and none per failure.
  Internal: the inventory and the resolution entries still copy the markings out until `DI-23.12.3.4`.

- [ ] **DI-23.12.3.3** -- **The suspect set as the rule that made it** ([DI-D-109](DESIGN-NOTES.md#di-d-109)'s second
  item): a failure keeps its cut, its reach, its lineage if one was named, and what else observation filtered on, and a
  write is its suspect when the records say so; the epochs it holds, which `blocking`, gates and abandoning read, and a
  heal's pending lineages, read from the same records. Internal: `SuspectSet` is still built for the entries until
  `DI-23.12.3.4`; the window's census and the oracle check that the rule names exactly the writes the old list did.

- [ ] **DI-23.12.3.4** -- **A failure's records lent by a scoped borrow** ([DI-D-108](DESIGN-NOTES.md#di-d-108)):
  `Failed`, `Abandoned`, `Healed` and `FailureInfo` carry the failure's identity, token, cause and stamp, and its suspect
  writes and markings are lent for the duration of a visitor; a resolved failure's records are kept until its `Abandoned`
  or `Healed` is popped. Breaking for both crates; CONTRACT.md, the READMEs' examples, and the oracle follow.

- [ ] **DI-23.12.3.5** -- **The completion queue and a change's entries within derived bounds**
  ([DI-D-109](DESIGN-NOTES.md#di-d-109)'s third item): a `Marked` queued once per marked write and expanded at pop into one
  entry per failure observed before the marking that suspects the write, in observation order; the queue and `Due`'s
  entries list made for the derived bound and pushed onto within it; and a test filling the queue to the bound.

- [ ] **DI-23.12.4** -- **Handles, tokens, descriptions and domain lists as fixed storage**
  ([DI-D-108](DESIGN-NOTES.md#di-d-108)): lineage handles and failure tokens become an index and a generation into an
  array of atomics the driver makes at construction and reaches through the home it supplies, sized from `lineages` and
  `DI-23.12.3`'s failure bound, keeping DI-D-12's one live token; a lineage's description is the consumer's string, kept
  and lent as `&str`; a target's domains become a range in a fixed table sized from `targets` and `domains`. Breaking:
  `LineageHandle`, `FailureToken` and `LineageInfo` change shape, and the `Home` trait grows.

- [ ] **DI-23.12.5** -- **An abandoned epoch is forgotten once the mark passes it** ([DI-D-109](DESIGN-NOTES.md#di-d-109)):
  above the mark it holds a place of the window until the mark passes it; at or below the mark `epoch_state` reports it as
  the mark means it. A contract change: CONTRACT.md's epoch states, and win-durable-ioring's README example asserting
  `Abandoned` below the mark. Proof obligation: how far the table can be taken past `operations` where another failure
  still holds the abandoned writes, and a test reaching it.

- [ ] **DI-23.12.6** -- **A refusal carrying a backend fault passes the backend's report through**
  ([DI-D-109](DESIGN-NOTES.md#di-d-109), amending [DI-D-54](DESIGN-NOTES.md#di-d-54)): the cause carried beside the report
  in `IoFault`, as `ErrorCode` is, so `Unusable` and `Unknown` allocate nothing; their cause is read from that field rather
  than downcast. Breaking for both crates where they downcast.

- [ ] **DI-23.12.7** -- **The crate builds without `std`** ([DI-D-97](DESIGN-NOTES.md#di-d-97),
  [DI-D-109](DESIGN-NOTES.md#di-d-109)): `no_std` unless the `std` feature is on; the driver's locking (`engine.rs`,
  `lock_watch.rs`) and the `thread::panicking()` guard behind `std`, an inconsistency without it panicking
  unconditionally; and every other use of `std` the crate still has moved to `core` or `alloc`, or behind the feature.
  Built here locally for a target without `std`; CI is `DI-23.13`.

- [ ] **DI-23.12.8** -- **A provider request's contents and a gate cycle's report onto storage made at construction**
  ([DI-D-97](DESIGN-NOTES.md#di-d-97)), **once the engineer decides their shape**: both reach the consumer, a request through
  the provider and a cycle in `PushRefusal::GateCycle` and `SealError::GateCycle`. Each is allocated where it is made: a
  seal's domain parts and the write lists in them, the `ProviderRequest` built from them, and the traced cycle's `Vec` of
  epochs. Found 2026-10-24 implementing `DI-23.12.2`, which moved the lists that carry these but not the contents; no step
  owned them, and the allocation test cannot see them, since it runs no provider and closes no cycle, so it gains a
  provider-routed seal and a refused cycle here. The assistant's recommendation, for the engineer: lend each by a scoped
  borrow of fixed storage, as a failure's records are ([DI-D-108](DESIGN-NOTES.md#di-d-108)) -- a request's parts sized
  from the recorded seals and the served domains, and a cycle, at most the operations plus one steps long, read through the
  refusal for the duration of a visitor. A contract change for both crates.

- [ ] **DI-23.12.9** -- **The allocation test asserts zero after construction in every phase** (`DI-23.1`): the recorded
  tables become an assertion that every phase after construction allocates nothing, construction's allocations staying
  recorded. Closes `DI-23.12`.

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
