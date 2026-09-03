# Checklist: reshaping the machine memory topology

A fresh plan, deliberately numbered `MMT-*` rather than continuing the release checklist's `SH-*`.
It supersedes the model items filed there during PR #56's tenth review round; those are marked and
point here.

Design decisions live in [DESIGN-NOTES.md](DESIGN-NOTES.md). The session that produced this plan is
[DESIGN-SESSION-2026-09-02-cache-locality-model.md](../../design-sessions/DESIGN-SESSION-2026-09-02-cache-locality-model.md);
the consumer whose requirements shaped it is
[topology-planner](../topology-planner/DESIGN-NOTES.md).

The crate's *original* design session, which produced the model this plan reshapes, is
[DESIGN-SESSION-2026-08-22-topology-schema.md](design-sessions/DESIGN-SESSION-2026-08-22-topology-schema.md).
Completed milestones are archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

## What this is for

This crate publishes a **refined view of what the platform publishes**
([D-21](DESIGN-NOTES.md#d-21)). The current model does that badly: it describes a machine as a list
of domains and answers questions with one global projection (`outermost_partitioning_cache`) that
three consumers have independently re-derived, two of them differently. It cannot say whether a fact
was observed or merely absent, it cannot answer anything about a *pair* of processors, and it
collapses a seven-kind, any-depth locality graph onto a single cache boundary.

The reshape has one governing idea, settled with the engineer: **model the observed connectivity.**
Presence and observation are facts to represent, not shapes to infer from.

**The scope test is "is this a refinement of what Windows reports?"** -- never "does the planner need
it?". [D-20](DESIGN-NOTES.md#d-20) draws the lower bound (the crate does not go below the Win32
topology APIs); D-21 draws the upper one. A planner requirement with no platform correspondence is
the **adapter's** problem and must not be filed here as a gap.

## Where this stands

| Milestone | State | What it is waiting on |
|---|---|---|
| M1 settle what is still open | **5 of 5 done** | nothing -- complete |
| M2 the granularity model | **6 of 6 done** | nothing -- complete |
| M3 observation and provenance | **ready** | nothing (1 of 4 answered early, by D-19) |
| M4 the queries | **ready** | M2 is done; M3 is decision work that does not block it |
| M5 the defects this subsumes | parked | M4, **except M5+.5 (done)** |

**M1 was decision work, not implementation**, and it is complete. Each item was a question the
session left open, and each would have changed the shape of everything below it.

**M2 onward is implementation, and it is in scope for PR #56.** Per
[D-21](DESIGN-NOTES.md#d-21) the reshape is self-justified as the refined view rather than waiting on
a consumer, so nothing here is gated on the planner. Taking it into the current PR means
`windows-topology-sys` 0.2.0 ships the shape once, instead of publishing a surface already known to
be wrong and breaking again later.

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

- [x] **MMT-1.2** -- **What a query returns when observations differ.** ~~And there are three cases,
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

  **A third case: `discover()`'s two calls are not atomic.** Raised, then twice mis-corrected, then
  wrongly retired, and finally **answered by [D-16](DESIGN-NOTES.md#d-16): collect again.** The whole
  path is kept because the wrong turns are instructive.

  If the incoherence is detectable and harmful, **re-initiate collection**. Both calls are
  whole-machine enumerations and trivially inexpensive, so a retry costs almost nothing, and more
  than a couple of passes failing to find a coherent set is not plausible.

  **Retry is also the discriminator this item twice claimed could not exist.** The assertion was that
  a transient inconsistency and a genuine one are indistinguishable *from a single observation* --
  true, and the conclusion that the model must therefore tolerate the ambiguity does not follow. Stop
  using a single observation: transience resolves on the next pass, and what survives is *proved*
  genuine. So only what has already been classified reaches the representation question below.

  The earlier missteps, kept short:

  - **It is not a torn read.** Nothing tears -- each call returns a self-consistent snapshot and the
    buffers are process-private. The accurate term is a *non-atomic composite*.
  - **Parking cannot cause it**, which was the example first given. Parking changes a CPU-Sets-only
    field; GLPIE does not report parked state, and none of the three overlapping facts move when a
    core parks -- `CoreIndex` and `NumaNodeIndex` are unchanged, `EfficiencyClass` is static.
  - **And then it was retired for proving too much** -- on the grounds that even an atomic
    `discover()` returns a topology stale the instant it returns, so the two-call window is only a
    larger instance of an unavoidable problem. True, and **not a reason to do nothing**: the two are
    not equally addressable. Staleness after the fact is the executor's to validate, and is already
    owned as `M-inf.1` in [topology-planner](../topology-planner/CHECKLIST.md).
    Incoherence *during* collection is ours, detectable, and cheap to fix.

  The framing is what caused the miss. Asking "what do we **store** when sources disagree" admits
  refuse, record, or prefer -- and quietly excludes "ask again", which is the standard shape every
  compare-exchange loop in this workspace already uses.

  ### What is left to decide

  Retry removes the transient cases, so what reaches representation is proved genuine. Remaining:

  1. ~~What is compared, to call a collection coherent.~~ **Not a decision -- an inventory, and it
     falls out of the implementation.** What *can* be compared is fixed by the data: whatever both
     sources report about the same thing, which is the processor sets naming each other, the core and
     NUMA groupings, and per-processor efficiency class. The one trap -- comparing *labels* rather than
     memberships, which would flag `[0, 2, 4, ...]` against `[0, 1, 2, ...]` as a conflict when the
     sources fully agree -- is already closed by [D-15](DESIGN-NOTES.md#d-15).
     And under [D-16](DESIGN-NOTES.md#d-16) a *partial* comparison is actively wrong: an incoherence in
     an uncompared fact survives the retry and is never classified, defeating the mechanism. So retry
     forces comparing everything, which makes the scope determined rather than chosen.
     It was listed as a decision while the question was still "detect or not, and how much", where
     scope would genuinely have been a knob. `D-16` removed the knob.

  2. **The bound**, and what exhausting it *means* -- not a failure to collect, but the **conclusion**
     that the disagreement is genuine, and the point at which the partition and attribute shapes
     apply. The *meaning* is settled by [D-16](DESIGN-NOTES.md#d-16); only the number is open, and it
     is small -- a couple of passes failing to find a coherent set is not plausible.

  3. **How a topology records its coherence.** *Records*, not reports -- an earlier draft of this item
     said "precisely enough to file a bug", which put a downstream concern in the crate that states
     facts, the same layering error as `outermost_partitioning_cache`.
     What is required is what each source said, kept rather than collapsed, so a reader can tell how
     far the parts may be **correlated** -- a different question from whether any one part is accurate.
     Turning that into something actionable, with the identifying provenance an actionable report
     needs, is the probe tools' job and is tracked as **M7** in
     [CHECKLIST-placement-tool.md](../../CHECKLIST-placement-tool.md).
     > **-> CROSS-COMPONENT HANDOFF:** the reporting half is `PT-7.1` and `PT-7.2` in
     > [CHECKLIST-placement-tool.md](../../CHECKLIST-placement-tool.md). That tool already carries the
     > review this needs -- the runner sees real values before sending (`PT-4.5`), the README lists what
     > is collected (`PT-4.3`), and suppression is recorded rather than merely absent.
     Only possible because [D-15](DESIGN-NOTES.md#d-15) keeps both observations: a disagreement cannot
     be reported after it has been collapsed.

  4. **The attribute shape has no representation.** `(kind, membership)` identity does not reach a
     per-processor scalar disagreement, and that gap is untouched by any of the above.

  ### Closed by [D-18](DESIGN-NOTES.md#d-18)

  **The attribute shape (4):** an observation is `(subject, claim, source)`, and a **subject** is either a relation
  identity `(kind, membership)` or a processor attribute `(processor, attribute)`. The mechanism above
  it is unchanged -- observations of one subject are a set, agreement is one subject observed twice,
  disagreement is a set with more than one distinct claim. So the second shape needs no second
  mechanism. D-15 had simply described the subject too narrowly, having been derived from the one case
  that was measurable at the time.

  **Recording coherence (3):** two facts, one derivable and one not. *That* collection concluded
  incoherently is a fact about the process -- the retry ran, the bound was exhausted, the sources still
  disagreed -- and nothing in the data says so, so it is recorded. *Which* subjects disagreed is
  derivable, and is recorded anyway: leaving it to be re-derived is exactly the arrangement `SH-16.9`
  documents going wrong three times in two different ways. A rendered report is **not** recorded; per
  [D-17](DESIGN-NOTES.md#d-17) that belongs to the probe tools.

  **The bound (2):** a small documented constant, cheap even when exhausted, since a persistently
  inconsistent machine pays only a few extra whole-machine enumerations. Its meaning was already
  settled by [D-16](DESIGN-NOTES.md#d-16) -- exhaustion is the **conclusion** that the disagreement is
  genuine, not a failure to collect, and `discover()` still returns a topology.

  **What made this closable** was not one insight but the arsenal accumulating: D-15 gave the identity,
  D-16 removed the transient cases so only proved-genuine ones needed representing, D-17 moved
  reporting out of the crate, and D-18 widened D-15's subject. Three of the item's four questions
  dissolved rather than being answered -- one already covered by a prior decision, one forced by the
  retry mechanism, one a constant with a rationale.

  Refusing outright remains rejected on its own merits: a genuine inconsistency is something a caller
  would rather be told about and route around than be unable to run at all.

  **A finding that came out of checking it:** `online` (GLPIE, from `active_processors`) and `parked`
  (CPU Sets) are **complementary, not overlapping**. Parked is not offline -- a parked processor is
  active and the scheduler is merely avoiding it. So the two sources together give a *fuller*
  availability picture than either alone, which is an argument for consuming both that has nothing to
  do with conflict.

  **And a fourth, within a single source:** a processor named by two `Core` domains, from malformed
  firmware or a hand-built description. Unchecked today.

  Two questions this block used to ask are now answered and are not repeated: whether `discover()`
  detects at populate time (**yes** -- it must, in order to retry), and whether it may prefer a source
  (**no** -- [D-15](DESIGN-NOTES.md#d-15) rejected reduce-on-insert, and preferring is that by another
  name). What remains is listed above.

- [x] **MMT-1.3** -- **What a consumer does when a needed fact was not observed**, given the bar that
  the model answers without further measurement. Degrade to a documented weaker policy, refuse, or
  answer with an explicit "chosen without knowing X" marker.
  **Narrowed by [D-19](DESIGN-NOTES.md#d-19), then resolved as not-this-crate's by
  [D-21](DESIGN-NOTES.md#d-21).** D-19 removed two thirds of it: a contested subject is one the
  unified view does not cover, which is D-13's not-observed, so there is one degradation path rather
  than one per reason a fact is missing.
  D-21 then places the remainder. This crate publishes a **refined view of what the platform
  publishes**; what a consumer *does* with an unobserved fact is not a question about that view. The
  model owes only that the absence be **representable and distinguishable** -- which is
  [D-13](DESIGN-NOTES.md#d-13), implemented by **M2+.5**. The behavioural decision is the consumer's
  and stays with `EP-1.4`.
  **This item was gating M2, and through it M3, M4 and M5** -- a decision that was never the model's
  to make was holding the whole reshape. Recorded as a planning defect rather than quietly fixed: it
  landed in a "decisions that shape everything below" milestone because it *looked* foundational, and
  foundational-looking is not the same as being about this component.
  > **-> CROSS-COMPONENT HANDOFF:** the behavioural half is `EP-1.4` in
  > [topology-planner](../topology-planner/CHECKLIST.md). It no longer has a counterpart here, so it
  > is that component's decision alone rather than a joint one.

- [x] **MMT-1.4** -- **Does `distances` survive at all?** The two-component architecture says the
  *synthesizer* measures, with the caller's permission, for its own scenario -- so a measured number
  is its working state and its justification for a choice, not a property of the machine. That
  reverses this session's earlier conclusion that measured facts must live in the model, which
  assumed a single component. If it holds, `distances` is **deleted rather than filled**, which is
  the opposite of what the release checklist proposed. Decide before removing anything.
  **Decided by the engineer, and the ruling is a scope boundary rather than a judgement about the
  field: this crate does not go below the Win32 topology APIs, so if they do not provide distance
  data, we do not have distance data.** Recorded as [D-20](DESIGN-NOTES.md#d-20). `distances` is
  deleted.
  **What the check found:** zero read sites. `render_node_distances` in `windows-platform-probes`
  reads the *probe's own* measured `Observation`, not this field, so the one thing that looked like a
  consumer is not one. Removal is spawned as **M5+.5** and is not gated on the reshape.

- [x] **MMT-1.5** -- **Does the synthesizer live in this crate, and therefore what is this crate
  called?** Recorded as open rather than settled: see
  [topology-planner/COMPONENT.md](../topology-planner/COMPONENT.md). The naming follows
  the merge rather than leading it -- while this crate is only a Win32 wrapper, `-sys` is correct
  for it; if it gains a synthesizer that measures, it stops being one and the name should change
  then.
  **Answered by the engineer's architectural shift, recorded as
  [EP-D-4](../topology-planner/DESIGN-NOTES.md#ep-d-4): no.** The planner is a separate
  component named **`topology-planner`** -- with no `windows-` prefix, because it plans against an
  abstracted idealized machine and emits a platform-neutral plan. So this crate does not gain the
  synthesizer, remains a pure Win32 wrapper, and **keeps its name**.
  What settles it is not a naming preference but the shift's second half: the planner queries an
  *abstract* model through traits, and **adapters** bridge this crate's objects to those traits. A
  crate that is one side of an adapter boundary is exactly what `-sys` names.
  [D-20](DESIGN-NOTES.md#d-20) reinforces it from the other direction -- a crate whose scope is
  "what the Win32 topology APIs report" is a `-sys` crate by construction.

## M2: the granularity model

**Ready.** M1 is closed, and [D-21](DESIGN-NOTES.md#d-21) makes every item here a refinement of what
Windows reports rather than something a planner asked for.

**Re-planned 2026-09-03, on execution.** Checking these six against the code they reshape found two
that already describe the status quo and three that are one deliverable. Recorded rather than
quietly worked around, per the re-plan rule.

- [x] **M2+.1** -- Model **observed sharing relations**, not a ladder of levels with optional rungs.
  A machine with no L3 has no L3 relation, which is an observation rather than a missing value.
  **Already satisfied; this item described the existing design.** `Domain` *is* a relation over a
  `ProcessorSet`; `DomainKind` is open with seven kinds (D-4); `Cache { level: u8 }` has no fixed
  rungs; and `cache_levels()` is documented as "derived from what the topology actually contains
  rather than from a fixed ceiling", with a regression test guarding the exact hazard this item
  names. A machine with no L3 simply has no `Cache { level: 3 }` domain today.
  Not absorbed silently: separating *relation identity* `(kind, membership)` from the **label**
  `Domain::id`, which [D-15](DESIGN-NOTES.md#d-15) requires, is real remaining work -- but it is
  observation work and belongs to **M3**, not here.

- [x] **M2+.2** -- Derive the order from **observed set inclusion**, never from firmware level
  numbers. Inclusion is checkable; numbering is asserted, and this crate has been bitten by asserted
  structure before -- the ARM64 host with no L3, and the guard test against a consumer sweeping
  `1..=4`.
  **The concrete target:** `cache_levels()` sorts by firmware `level`, and
  `outermost_partitioning_cache()` walks it with `.rev()` -- so today's only ordering *is* firmware
  numbering, which is what this item forbids.

- [x] **M2+.3** -- Give the order an explicit **top** ("the machine"), so a pairwise query is total.
  Two processors always share one address space, one scheduler and one memory system; without a top,
  every caller writes the same empty-case branch for a cross-node pair.

- [x] **M2+.4** -- Represent **incomparable** granularities. An inclusion order is partial, so two
  granularities may not nest, and the honest answer to "tightest shared" is then a set of minimal
  elements -- almost always one, but not by construction.

  > **M2+.2, M2+.3 and M2+.4 are one deliverable and land in one commit citing all three.** They are
  > not independently implementable: an inclusion-derived order cannot be defined without deciding
  > what its top is and what happens when two elements do not nest, and a type that answered only one
  > of the three would not compile into anything coherent. This is the acknowledged-coupling case,
  > named rather than disguised by splitting the commits.
  >
  > **Shape:** the order is over *relations*, compared by processor-set inclusion, with a synthetic
  > `Machine` top that is **not** inserted into `domains` -- putting it there would claim the platform
  > observed it. The operation the order exists to support is "the minimal relations containing this
  > set of processors", which returns a `Vec` precisely because M2+.4 says minimality need not be
  > unique. The *pairwise* query built on it, with membership and the upper-bound flag, is `M4+.1`.
  >
  > **Done.** `src/granularity.rs` -- `Granularity::{Relation, Machine}`,
  > `MachineMemoryTopology::{machine_processors, minimal_shared, is_finer_than}`, on a new
  > `ProcessorSet::is_subset`. 21 tests.
  > **The top is a fallback, not a competitor**: `Machine` is returned exactly when no reported
  > relation covers the query, so it never appears beside an observed relation. The alternative --
  > treating it as an ordinary element -- was rejected on measurement of its consequence: on a machine
  > whose group domain spans every processor, every answer would carry a redundant second element,
  > which is systematic noise rather than M2+.4's "almost always one".
  > **Totality is over processors the topology knows.** A query naming an unknown processor answers
  > *empty*, not `Machine`, because claiming the machine contains a processor it has never heard of
  > would be an invention.
  > **Sabotage-verified, and it found a real gap.** Removing the strictness from minimality failed 10
  > of 20 tests. Breaking `is_subset` failed only **one**, incidentally -- the new primitive the whole
  > order rests on had no direct tests, and every granularity test used group 0 alone, so the
  > multi-group path was untested. Seven `is_subset` tests and a cross-group order test took that from
  > 1 detection to 4, two of which name the defect directly.

- [x] **M2+.5** -- Make absence first-class per [D-13](DESIGN-NOTES.md#d-13): **not observed**,
  **observed and absent**, and **a negative result** are three different facts that an `Option`
  spells identically. Per [D-19](DESIGN-NOTES.md#d-19) this also carries the contested case -- a
  subject the sources genuinely disagreed on is one the unified view does not cover, which is *not
  observed*, so no fourth state is added.
  **Deliverable: the vocabulary type**, which `M5+.2` and `M5+.4` then consume -- M5+.4 already says
  "M2+.5 gives it the vocabulary to accept one". Independent of M2+.2/.3/.4, so it lands separately.
  **Done.** `src/observed.rs` -- `Observed<T>` with `Known`, `Absent`, `NotObserved`, plus `known()`,
  `was_observed()`, `map()`, and a `Default` of `NotObserved` (D-12's reasoning: forgetting a field
  must not assert something about the machine). 9 tests.
  **Two variants, not three, and the omission is deliberate.** The *negative result* is not an
  absence -- it is a computed answer whose value happens to be "no" -- so giving it a variant would
  re-create the conflation the type removes. It stays an ordinary value, or an `Option` documented as
  meaning exactly that.
  **Sabotage-verified:** making `was_observed()` treat `Absent` as a gap -- the precise conflation
  this type exists to prevent -- is caught by the test named for that claim.
  Not yet *applied* to any field: `M5+.2` (`memory_bytes` from a description) and `M5+.4` (the probe
  refusing a partially-covering cache level) are the sites, and both are M5 items.

- [x] **M2+.6** -- **Relations carry attributes, not only memberships.** Required by
  [D-19](DESIGN-NOTES.md#d-19): once the relation set *is* the unified model,
  `DomainKind::Memory { memory_bytes }` and `Core { efficiency_class, simultaneous_multithreading }`
  have nowhere to live unless a relation holds a payload alongside its processor set.
  **Already satisfied, and the premise was wrong.** `DomainKind` has carried per-kind attributes
  since D-4: `Memory { memory_bytes }`, `Core { simultaneous_multithreading, efficiency_class }`,
  `Cache { level, associativity, line_size, size_bytes, cache_type }`, and `Other { name, attributes
  }` for a kind this crate cannot interpret. Nothing had "nowhere to live".
  The item was written from the abstract `(kind, membership)` framing while recording D-19, without
  checking it against the type -- which had solved this two decisions earlier. Kept rather than
  deleted because it is the second time in this milestone that an item asserted a gap the code did
  not have, and once is a slip while twice is a method problem: **check the item against the code
  before planning work from it.**

## M3: observation and provenance

**Ready.** M1 is closed.

**Re-planned 2026-09-03, before implementing.** Checking these against the code found `M3+.3`'s
premise wrong in the same way `M2+.6`'s was, and found work this milestone had been assigned but
never given an item. Recorded rather than absorbed silently -- this is the third item in the reshape
to assert a gap the crate does not have, and the pattern is what the M2 re-plan named: **check the
item against the code before planning work from it.**

- [ ] **M3+.1** -- Provenance is **per relation**, not per source. Per-relation subsumes per-source
  by repetition, and the reverse fails on the case that matters: two sources describing the *same*
  relation.
  **What that means concretely, which the item did not say.** "The case that matters" does not exist
  in the code yet: `domains` is built from `GetLogicalProcessorInformationEx` alone, and `cpu_sets`
  sits beside it as a parallel list, so no relation is currently described by two sources. Satisfying
  this item therefore means **unifying the two sources into one relation set keyed by
  `(kind, membership)`** per [D-15](DESIGN-NOTES.md#d-15), with each relation recording which sources
  observed it. That is the heart of [D-19](DESIGN-NOTES.md#d-19)'s unified view, and it does not
  exist yet.
  **It absorbs the `Domain::id` work rather than leaving it a separate item.** D-15 requires the
  *label* to move from the relation to the observation, and unification forces it: the two sources
  agree on the core partition while labelling it `[0, 2, 4, ..., 14]` against `[0, 1, ..., 7]`, so a
  single unified relation cannot carry one `id`. The two are the same change.
  *(The M2 re-plan said this work "belongs to M3" without filing it anywhere, so until now it was
  owned by no item at all.)*

- [ ] **M3+.2** -- Keep two properties of the old `Provenance` **because they re-derive**, not
  because they were there: the default is the untrusted value (a *stronger* argument per-relation,
  since there are more places to forget), and trust never upgrades (a file still cannot establish it
  describes the machine you are on).

- [ ] **M3+.3** -- ~~Supersede the whole-object `Provenance` **without replacing it with another
  whole-object scalar**. With trust per relation, an object-level scalar can only be the minimum --
  ninety-nine measured relations and one synthetic reading `SYNTHETIC` -- or the maximum, which is
  dishonest. Trust belongs to an *answer*.~~
  **Rewritten. The premise is wrong, recorded as [D-22](DESIGN-NOTES.md#d-22).** `Provenance` is not
  an aggregate of anything: it records **how the object was obtained** -- `discover()` stamps
  `Measured`, deserialization is capped at `Restored`, hand construction defaults to `Synthetic`. That
  is a fact about the construction *act*, and no per-relation value can express it. The mixed
  "ninety-nine measured and one synthetic" case cannot arise from collection at all; it needs someone
  to hand-insert a relation into a discovered topology, which is exactly what per-relation provenance
  makes **visible** rather than a reason to delete the object-level fact.
  It also has a real consumer that wants precisely it: `windows-placement-probe`'s
  `Record::is_trustworthy` gates on `is_measured()` to decide whether a measurement counts, and its
  record schema carries the value at the top level deliberately so a collector need not reach into
  the fingerprint.
  **Revised deliverable:** keep the type, and make its documentation say what it actually means --
  the construction act, orthogonal to per-relation provenance -- so the next reader does not repeat
  this item's mistake.

- [x] **M3+.4** -- Carry both observers without merging, per MMT-1.1's decision. `Topology::cpu_sets`
  already lands this way; this item is whether that stays a parallel list or becomes observations
  attached to relations.
  **Answered by [D-19](DESIGN-NOTES.md#d-19): both.** The question presented the two as alternatives,
  and they are not. Observations attach to relations, which is what makes the *unified* view exist at
  all; the raw per-source list stays for a caller that wants what one source said, verbatim. That is
  what "a unified model in addition to the individual ones" means concretely. No implementation is
  owed here -- M2+.6 and M4 carry the surface -- so this item is closed as a decision, not as code.

## M4: the queries

Parked on M2 and M3.

**Each is a refinement of what Windows reports**, per [D-21](DESIGN-NOTES.md#d-21) -- not a planner
requirement, which is how they were first justified. The change matters because it changes what is
in scope: a query the platform's data supports belongs here whether or not any planner wants it, and
a planner requirement with no platform correspondence belongs to the adapter.

Read on their own terms, most of these were never planner-shaped. `M4+.2` and `M4+.3` are ordinary
facts about processors and memory stated without sentinels; `M4+.4` fixes a rule the **probes** have
restated three times in two crates. Only `M4+.1`'s pairwise helper is consumer-flavoured, and the
ordered collection it derives from is what stops that restatement recurring.

They remain cross-referenced to [topology-planner](../topology-planner/DESIGN-NOTES.md) as
**evidence** the shape is right rather than as its justification -- stating those requirements found
the `Processor::capacity` sentinel collision that reviewing the model alone had not.

- [ ] **M4+.1** -- **The ordered relations are the query surface; pairwise proximity is a method on
  them.** The requirement arrived from [EP-D-2](../topology-planner/DESIGN-NOTES.md#ep-d-2) as a
  *pairwise* query returning the minimal shared granularities, **their membership**, and whether a
  finer granularity went **unobserved** so the answer can be an upper bound and say so. All three
  requirements stand. The **shape** does not, and the requirement says so itself: it asks the answer
  to carry the whole block containing both processors, "or the planner asks O(n^2) times and
  reconstructs the grouping". An answer that must carry the block is not about the pair -- the pair is
  an index into a partition.
  Everything the planner does is an operation on the partitions: choosing domain granularity is
  *selecting one*, sizing an MPSC fan-in is *a block's cardinality*, choosing a channel is *the finest
  block containing both*. Pairwise is three lines over that. The reverse is derivable too, but only by
  union-find over O(n^2) queries -- which is exactly the reconstruction `SH-16.9` records three
  consumers performing, two of them differently. Building pairwise as the primary surface would ship
  the stated requirement and re-create the defect one level up.
  Both are provided; **the collection is primary and the pairwise helper is derived from it**, so
  there is one implementation of the grouping.
  *Terminology:* it is a **poset with a top**, not a lattice -- M2+.4's incomparable granularities
  mean meets need not be unique, which is also why a pairwise function has to return a *set* and is
  an awkward face on an ordered collection.

- [ ] **M4+.2** -- The **shard-set** surface (EP-D-1): identity as `(group, number)`, online, core
  membership and SMT, efficiency class **without a sentinel**, and availability.

- [ ] **M4+.3** -- **Residency** (EP-D-3): processor to memory domain, with the unplaced case
  distinguishable rather than defaulted -- an unknown cache domain costs an optimisation, an unknown
  memory domain has no honest fallback.

- [ ] **M4+.4** -- Reduce `outermost_partitioning_cache` to a **named projection** over the order --
  "the coarsest granularity with more than one group" -- so it is a query rather than a rule, and
  cannot be restated wrongly because there is nothing to restate.

## M5: the defects this subsumes

Parked on M4, **except M5+.5, which is independent and ready now**. Each of the others already
exists as a defect that the reshape is what fixes, so they are listed here rather than fixed
separately and then re-fixed.

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

- [x] **M5+.5** -- **Delete `MachineMemoryTopology::distances` and the `Distances` type**, per
  [D-20](DESIGN-NOTES.md#d-20). **Not gated on M4**: the reshape does not fix this one, deletion
  does, so it does not wait for the rest of M5.
  A **breaking change to a published crate** (0.1.0), so the commit takes the Conventional Commits
  `!` marker. It is not a *parse* break -- the crate does not `deny_unknown_fields`, so a description
  carrying `"distances"` still deserializes and the field is ignored.
  Two things to do rather than skip: keep the Linux-shaped description test, retargeted to assert the
  field is now **ignored** rather than deleting the evidence that such a description parses; and note
  in the doc comment that round-tripping such a description no longer preserves it, since that is a
  real if small behaviour change and a silent drop is exactly what this crate has objected to
  elsewhere.
  **Done.** Field, type, and re-export removed; three call sites in `windows-placement-probe`'s
  fingerprint fixtures updated. The Linux-shaped test survives as
  `a_linux_shaped_description_parses_and_its_distances_are_ignored`, keeping the **populated** matrix
  so what it proves is that an existing description still parses, and gaining an assertion that the
  value does **not** reappear on re-serialize -- the silent drop asserted rather than assumed.
  `distances_is_expected_to_be_square` was deleted with the type it tested (125 tests to 124).
  Two stale statements sweeps found and fixed: the [D-13](DESIGN-NOTES.md#d-13) audit row, and the
  Linux-comparison summary, which had recorded optional distances as a decision that *held up* --
  sound about the schema, and reversed by a ruling about scope.
