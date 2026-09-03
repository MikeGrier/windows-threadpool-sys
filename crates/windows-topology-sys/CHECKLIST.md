# Checklist: reshaping the machine memory topology

A fresh plan, deliberately numbered `MMT-*` rather than continuing the release checklist's `SH-*`.
It supersedes the model items filed there during PR #56's tenth review round; those are marked and
point here.

Design decisions live in [DESIGN-NOTES.md](DESIGN-NOTES.md). The session that produced this plan is
[DESIGN-SESSION-2026-09-02-cache-locality-model.md](../../design-sessions/DESIGN-SESSION-2026-09-02-cache-locality-model.md);
the consumer whose requirements shaped it is
[windows-execution-plan](../windows-execution-plan/DESIGN-NOTES.md).

The crate's *original* design session, which produced the model this plan reshapes, is
[DESIGN-SESSION-2026-08-22-topology-schema.md](design-sessions/DESIGN-SESSION-2026-08-22-topology-schema.md).
Completed milestones are archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

## What this is for

The current model describes a machine as a list of domains, and answers questions about it with one
global projection (`outermost_partitioning_cache`) that three consumers have independently
re-derived, two of them differently. It cannot say whether a fact was observed or merely absent, it
cannot answer anything about a *pair* of processors, and it collapses a seven-kind, any-depth
locality graph onto a single cache boundary.

The reshape has one governing idea, settled with the engineer: **model the observed connectivity.**
Presence and observation are facts to represent, not shapes to infer from.

## Where this stands

| Milestone | State | What it is waiting on |
|---|---|---|
| M1 settle what is still open | 1 of 5 done | nothing -- these are decisions, and they gate the rest |
| M2 the granularity model | parked | M1 |
| M3 observation and provenance | parked | M1 |
| M4 the queries | parked | M2, M3 |
| M5 the defects this subsumes | parked | M4 |

**M1 is decision work, not implementation.** Each item is a question the session left open, and each
would change the shape of everything below it. They are cheap to answer and expensive to answer
wrongly after code exists.

## M1: settle what is still open

- [x] **MMT-1.1** -- **Are several observations of one relation held as a set, or reduced on insert
  with the reduction recorded?** No longer speculative: `GetLogicalProcessorInformationEx` and
  `GetSystemCpuSetInformation` both report a processor's core, NUMA node and efficiency class, from
  different kernel paths, and both are read today. A set is honest and pushes adjudication onto every
  caller; reducing on insert is convenient and throws away the disagreement, which is the one thing a
  second observer is for.
  **Done, as [D-15](DESIGN-NOTES.md#d-15): a set -- but the reason is not the one above.** Measured
  rather than argued. The two sources **agree exactly** on the core partition (eight groups each) and
  **label it completely differently** (`[0, 2, 4, ..., 14]` against `[0, 1, ..., 7]`). So the
  disagreement a reduction would resolve is between *dictionaries*, not about the machine.
  That makes **a relation identified by `(kind, membership)`**, with a source's label an attribute of
  the *observation*. Reduce-on-insert is then not merely lossy but **arbitrary** -- it would pick
  between two correct labels by coin toss, while the fact that mattered needed no reduction because
  the sources agreed. And a set costs nothing in the common case: agreement is one relation with two
  observations, not two competing relations.
  **Honest about the evidence:** only the core comparison is strong. NUMA is one group here so it
  matches under almost any bug, and efficiency class is zero everywhere -- which is both trivially
  matchable and the exact value `Processor::capacity`'s sentinel is indistinguishable from, so that
  row confirms nothing. A hybrid, multi-node machine would test all three; none is available.

- [ ] **MMT-1.2** -- **What a query returns when observations differ.** ~~And there are three cases,
  not two~~ -- **the third case dissolved.** [D-14](DESIGN-NOTES.md#d-14) found that CPU Sets reports
  one last-level-cache group where the derivation reports eight L2 partitions, neither wrong because
  they answer **different questions**, and this item was going to have to invent vocabulary for it.
  [D-15](DESIGN-NOTES.md#d-15) removes the need: under `(kind, membership)` identity, different
  memberships at different kinds are simply **different relations**, so they never meet to disagree.
  **What remains is narrower**: two sources claiming the same *kind* over overlapping-but-unequal
  memberships -- a real contradiction about the machine. Decide what a query returns then: a value
  plus a conflict marker, or the conflict itself, forcing the caller to adjudicate.
  Note the detection machinery partly exists. Overlapping-but-unequal sets at one kind is exactly
  what `are_pairwise_disjoint` checks for cache domains today -- though only at *query* time, inside
  `outermost_partitioning_cache`, and `Core` and `Memory` domains are never validated at all.

  ### The specifics, since "observations differ" is too vague to decide on

  **Where it arises:** `discover()`, populating a `MachineMemoryTopology`. It makes **two separate,
  sequential Win32 calls** -- `relation::discover()` then `cpu_set::enumerate()` -- and nothing
  compares their results.

  **Two shapes of conflict, not one.** The item above describes only the first:

  - **A, partition conflict:** same kind, memberships overlap without being equal. GLPIE says a core
    is `{0,1}`, CPU Sets groups `{0,1,2}` under one `CoreIndex`. `(kind, membership)` identity
    from [D-15](DESIGN-NOTES.md#d-15) makes this detectable.
  - **B, attribute conflict:** same processor, same attribute, **different scalar**. GLPIE's
    `Core { efficiency_class }` against CPU Sets' `EfficiencyClass`. This is not a membership
    question and D-15 does not reach it, which the item as first written did not notice.

  **A third case that is not a source conflict at all: the two calls are not atomic.** A processor
  parked, unparked, hot-added or hot-removed between them means the two halves describe **different
  instants**. That is not Windows contradicting itself -- it is us sampling twice -- and **from a
  single observation a torn read is indistinguishable from a genuine inconsistency.** So the topology
  is already a composite of two moments and nothing records that, which is true even when nothing
  conflicts.

  **And a fourth, within a single source:** a processor named by two `Core` domains, from malformed
  firmware or a hand-built description. Unchecked today.

  ### What to decide

  1. Does `discover()` **detect** at populate time, or is a conflict something only a query surfaces?
     Detecting costs a comparison over every overlapping fact; not detecting means a caller who never
     asks the right question never learns.
  2. What happens when it does: **refuse** (an `Err` from `discover`, which would make a machine
     unusable over a discrepancy that may be benign), **record and continue** (consistent with this
     crate's posture of representing the awkward case), or **prefer a source** -- which is
     reduce-on-insert wearing a different hat, and D-15 rejected it.
  3. Whether to record that a topology is a **composite of two instants** regardless of conflict, and
     whether to spend a third call re-reading the first source to tell a torn read from a real one.

- [ ] **MMT-1.3** -- **What a consumer does when a needed fact was not observed**, given the bar that
  the model answers without further measurement. Degrade to a documented weaker policy, refuse, or
  answer with an explicit "chosen without knowing X" marker.
  > **-> CROSS-COMPONENT PREREQUISITE:** this is the same decision as `EP-1.4` in
  > [windows-execution-plan](../windows-execution-plan/CHECKLIST.md), seen from the model's side
  > rather than the consumer's. They were filed independently before anyone noticed. **Take them
  > together** -- answering either alone risks a planner that degrades in a way the model does not
  > support, or a model offering a fallback no consumer wants.

- [ ] **MMT-1.4** -- **Does `distances` survive at all?** The two-component architecture says the
  *synthesizer* measures, with the caller's permission, for its own scenario -- so a measured number
  is its working state and its justification for a choice, not a property of the machine. That
  reverses this session's earlier conclusion that measured facts must live in the model, which
  assumed a single component. If it holds, `distances` is **deleted rather than filled**, which is
  the opposite of what the release checklist proposed. Decide before removing anything.

- [ ] **MMT-1.5** -- **Does the synthesizer live in this crate, and therefore what is this crate
  called?** Recorded as open rather than settled: see
  [windows-execution-plan/COMPONENT.md](../windows-execution-plan/COMPONENT.md). The naming follows
  the merge rather than leading it -- while this crate is only a Win32 wrapper, `-sys` is correct
  for it; if it gains a synthesizer that measures, it stops being one and the name should change
  then.

## M2: the granularity model

Parked on M1. Shape recorded so it is not lost.

- [ ] **M2+.1** -- Model **observed sharing relations**, not a ladder of levels with optional rungs.
  A machine with no L3 has no L3 relation, which is an observation rather than a missing value.

- [ ] **M2+.2** -- Derive the order from **observed set inclusion**, never from firmware level
  numbers. Inclusion is checkable; numbering is asserted, and this crate has been bitten by asserted
  structure before -- the ARM64 host with no L3, and the guard test against a consumer sweeping
  `1..=4`.

- [ ] **M2+.3** -- Give the order an explicit **top** ("the machine"), so a pairwise query is total.
  Two processors always share one address space, one scheduler and one memory system; without a top,
  every caller writes the same empty-case branch for a cross-node pair.

- [ ] **M2+.4** -- Represent **incomparable** granularities. An inclusion order is partial, so two
  granularities may not nest, and the honest answer to "tightest shared" is then a set of minimal
  elements -- almost always one, but not by construction.

- [ ] **M2+.5** -- Make absence first-class per [D-13](DESIGN-NOTES.md#d-13): **not observed**,
  **observed and absent**, and **a negative result** are three different facts that an `Option`
  spells identically.

## M3: observation and provenance

Parked on M1.

- [ ] **M3+.1** -- Provenance is **per relation**, not per source. Per-relation subsumes per-source
  by repetition, and the reverse fails on the case that matters: two sources describing the *same*
  relation.

- [ ] **M3+.2** -- Keep two properties of the old `Provenance` **because they re-derive**, not
  because they were there: the default is the untrusted value (a *stronger* argument per-relation,
  since there are more places to forget), and trust never upgrades (a file still cannot establish it
  describes the machine you are on).

- [ ] **M3+.3** -- Supersede the whole-object `Provenance` **without replacing it with another
  whole-object scalar**. With trust per relation, an object-level scalar can only be the minimum --
  ninety-nine measured relations and one synthetic reading `SYNTHETIC` -- or the maximum, which is
  dishonest. Trust belongs to an *answer*.

- [ ] **M3+.4** -- Carry both observers without merging, per MMT-1.1's decision. `Topology::cpu_sets`
  already lands this way; this item is whether that stays a parallel list or becomes observations
  attached to relations.

## M4: the queries

Parked on M2 and M3. Each is a requirement from
[windows-execution-plan](../windows-execution-plan/DESIGN-NOTES.md), stated there against a real
caller rather than invented here.

- [ ] **M4+.1** -- **Pairwise proximity**, over an **unordered** pair, returning the minimal shared
  granularities, **their membership** (so a caller can size an MPSC fan-in without re-deriving the
  grouping), and whether a finer granularity went **unobserved** so the answer can be an upper bound
  and say so. This is the query with no equivalent today, and its absence is why the partitioning
  rule got re-derived three times.

- [ ] **M4+.2** -- The **shard-set** surface (EP-D-1): identity as `(group, number)`, online, core
  membership and SMT, efficiency class **without a sentinel**, and availability.

- [ ] **M4+.3** -- **Residency** (EP-D-3): processor to memory domain, with the unplaced case
  distinguishable rather than defaulted -- an unknown cache domain costs an optimisation, an unknown
  memory domain has no honest fallback.

- [ ] **M4+.4** -- Reduce `outermost_partitioning_cache` to a **named projection** over the order --
  "the coarsest granularity with more than one group" -- so it is a query rather than a rule, and
  cannot be restated wrongly because there is nothing to restate.

## M5: the defects this subsumes

Parked on M4. Each already exists as a defect; the reshape is what fixes them, so they are listed
here rather than fixed separately and then re-fixed.

- [ ] **M5+.1** -- `Processor::capacity` uses `0` as both a legitimate efficiency class and a "not
  known" sentinel, and the two collide on **every non-hybrid machine**. Worse than an ambiguous
  `Option`: a colliding sentinel cannot be distinguished even by a careful caller.

- [ ] **M5+.2** -- `DomainKind::Memory::memory_bytes` is unambiguous from `discover` but ambiguous
  from a **description**, where "the field was omitted" and "this node's capacity is unknown" are the
  same value. The [D-13](DESIGN-NOTES.md#d-13) audit found this and documentation cannot fix it.

- [ ] **M5+.3** -- The partitioning rule is stated **three times in two crates**, and two of the
  three differ: `windows-platform-probes` omits the pairwise-disjointness check this crate requires.
  M4+.4 removes the reason to restate it.

- [ ] **M5+.4** -- `windows-placement-probe` **refuses a partially-covering cache level** that this
  crate deliberately hands back, failing an entire measurement run over a topology this crate
  considers describable. M2+.5 gives it the vocabulary to accept one.
