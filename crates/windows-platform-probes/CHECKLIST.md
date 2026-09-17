# Checklist: windows-platform-probes

Design decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md). This crate's *creation* is tracked
separately, in the workspace [CHECKLIST-thread-ambient.md](../../CHECKLIST-thread-ambient.md) milestone
M27; that file is feature-scoped and is deleted when its feature completes, so durable follow-up work
for the crate belongs here instead.

## M4 -- Carried over from M2: the items M3 gated

These were written under M2 and were blocked on M3, which is **now complete and archived** in
[COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md). They are unblocked, and each was RE-SCOPED rather
than merely delayed -- so each item below carries its own re-scoping note, in the item, where
somebody executing the list will actually meet it.

**The IDs keep their M2 numbers deliberately.** [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md) is
append-only and its entries are immutable, and two archived entries already cite M2.4 and M2.14 --
so renumbering would leave dangling references in a file that may not be edited to repair them.
Stable IDs cost a mismatch between an item number and its milestone; renumbering would cost
correctness in the archive.

- [ ] **M4.1** -- Model the observation-readable `ParseIncomplete` conditions that
  `blocking_states` currently omits, so a deleted push site in `cross_check` is caught for all of
  them rather than for the subset.

  **Gap:** `blocking_states` justified its absentees as "derived counts whose only source IS the
  cross-check's own arithmetic". That is false for most of them --
  `CacheLevelsWithoutPartitions`, `NumaDomainsOnlyInCpuSets`, `CoresOnlyInCpuSets`,
  `RelationsWithoutProcessors`, `UnreportedRelations`, `DescribedRelations`,
  `OverlappingWalkRelations`, `ProcessorAttributeConflicts`, `NumaDomainsWithConflictingLabels`,
  `NumaDomainsUnreported` and `MeasuredButCountsAbsent` all read fields sitting on `Observation`
  in plain sight. Deleting one of those push sites lets `verdict()` reach `agree` with
  `blocking_states` silent. Reported across three review rounds against two wordings of the claim;
  the claims in [src/topology/invariant.rs](src/topology/invariant.rs) and in
  `every_numa_counter_branch_...` were narrowed in the same review round that reported this,
  to stop overstating the coverage,
  which is why this item is the fix rather than the discovery.

  **Target:** each gains a `BlockingState` variant, a `blocking_states` branch, a `codes_for` arm,
  a perturbation in the invariant tests, and a corpus shape in
  [tests/a_real_report_agrees_with_itself.rs](tests/a_real_report_agrees_with_itself.rs) so
  `the_corpus_reaches_every_blocking_state` still holds. **Do not add a variant without its corpus
  shape** -- that test is what stops the mapping being written and never exercised, which is the
  defect this whole area keeps producing.

  Apply the tautology test to each before adding it: a state whose only source is the cross-check's
  own arithmetic does NOT belong, and the honest outcome for such a one is a line in the module
  header saying so by name rather than a silent absence.

- [ ] **M4.4** -- Interleave each candidate with a nearby control instead of measuring the control
  four runs away from it, and re-measure everything that changes.

  **Gap:** `measure()` runs, per producer count, `baseline_fetch_add`, `slotwise_mpsc`,
  `reserving_mpsc`, `permit_mpsc`, then the three drained shapes, then the layout rows starting with
  `reserving(32/32)`. The same-code control is the `reserving_mpsc` row against the
  `reserving(32/32)` row -- **four measurements apart**, each five repetitions of 50,000 pushes per
  producer. Frequency, thermal and scheduler drift across that interval is folded into the control,
  and into every candidate the control is used to judge. At sixteen and thirty-two producers, where
  the machine is oversubscribed and the layout differences are smallest, that is exactly where it
  matters most.

  This is the first *specific* mechanism proposed for the 7-61% same-configuration spread recorded in
  [DESIGN-NOTES.md](DESIGN-NOTES.md#d-variance-is-a-finding); the other candidates there are general.
  Reported by review.

  **Target:** measure each candidate adjacent to a control run of the same code, or randomise and
  balance the order across repetitions so drift cannot align with position in the sequence. Whichever
  is chosen, the control must end up measuring the same interval the candidate did.

  **BLOCKER, same as M4.3:** interleaving changes the measurement, so every figure published in
  [DESIGN-NOTES.md](DESIGN-NOTES.md) becomes a measurement of a different procedure. The item is
  "change it *and* re-run the sweep *and* rewrite the sections", not a reordering. Doing it
  mid-branch would invalidate figures that ten review rounds have been read against. Raised rather
  than silently deferred, per the PRIME DIRECTIVE.

  **It is placed ahead of M4.2 deliberately**, since a control that is not paired cannot answer
  whether lengthening the run narrows the spread -- that answer would be confounded by the same
  drift this item removes. Taking M4.2 first would produce a diagnosis nobody could trust.

- [ ] **M4.2** -- Give the measurement probes the controls needed to act on a dispersion finding,
  so "gather more data along this axis" does not require editing a `const` and rebuilding.

  **Ordered after M4.4, which is why it appears second despite the lower number.** The first
  diagnostic step this item unblocks is "lengthen the run and see whether the control narrows", and
  that cannot be read while the control is measured four runs away from its candidate -- drift would
  confound it either way.

  **This item is deliberately small in software and large in guidance.** The diagnostic method
  belongs in [DESIGN-NOTES.md](DESIGN-NOTES.md) -- see
  [What to try first, and how to tell when you have reached the
  floor](DESIGN-NOTES.md#d-variance-is-a-finding) -- and this item exists only to make that method
  executable. The judgement stays with the person; the probe stops being the obstacle.

  **Gap:** [src/queue_contention.rs](src/queue_contention.rs) fixes every sampling parameter as a
  compile-time constant -- `PUSHES_PER_PRODUCER` (50,000) and `REPETITIONS` (5) -- and `measure()`
  takes no arguments. They were made `pub` and are now printed in the report, so a captured run at
  least says what produced it; but reading a constant is not setting one. The first move the design
  note prescribes on seeing a wide control is to lengthen the span and raise the repetition count on
  the unchanged configuration, which is still a source edit and a rebuild. A control that cannot be
  turned is not a control, and the cheapest diagnostic step is the one being blocked.

  **Target:** `measure()` takes a settings value carrying at least the pushes-per-producer count,
  the repetition count, and the producer counts to sweep (`PRODUCER_COUNTS` is already public and
  is the model for the others). Existing defaults stay exactly as they are, so a default run remains
  the run the notes describe and every published figure stays reproducible. The binary exposes the
  same knobs so a human or an agent can act without a rebuild.

  Apply the same treatment to the sibling cost probes where the sampling parameters are equally
  fixed; the axes we anticipate varying are **duration, repetitions, and concurrency**, so those are
  the ones that need to be reachable. Do not add knobs beyond what a stated diagnostic step needs --
  an unused parameter is a configuration surface to maintain and a way for two runs to differ
  without anyone noticing.

  **Report what was used.** Whatever settings a run was given must appear in its output beside the
  host banner, for the reason
  [D-observations-not-verdicts](DESIGN-NOTES.md#d-observations-not-verdicts) already gives: a figure
  is only interpretable with its capture parameters, and these are now among them. Making the
  sampling adjustable without recording it would turn one reproducibility problem into a worse one.

  **Not in scope:** deciding why the control is wide. That is the judgement this tooling supports,
  and per the design note a negative result -- "lengthening and repeating do not narrow it, so the
  floor is here" -- is a real answer that gets recorded beside the figures.

  **Also not in scope, because it is done:** emitting the dispersion. See M4.5 below.

- [x] **M4.5** -- Emit the dispersion, not just the median. -> [completed 2026-09-15 UTC-07:00](COMPLETED-CHECKLIST.md#m45)

- [x] **M4.3** -- Close the undrained window at the start of the drained regime with a readiness handshake, and re-measure everything that changes. -> [completed 2026-09-16 UTC-04:00](COMPLETED-CHECKLIST.md#m43)

- [x] **M4.6** -- Make the start gate releasable, so a failed thread spawn cannot deadlock the probe. -> [completed 2026-09-16 UTC-04:00](COMPLETED-CHECKLIST.md#m46)

- [x] **M4.7** -- Make the queue-contention report renderer testable, by taking the observation as an argument instead of measuring inside it. -> [completed 2026-09-16 UTC-04:00](COMPLETED-CHECKLIST.md#m47)

- [ ] **M2.15** -- Run the probe suite on a second architecture in CI.

  **Keeps its conclusion but loses its evidence.** The five failures cited below were all
  `prose: "x86_64"` against `ndjson: "x86"` -- instances of exactly the correspondence M3.4
  retired, so they can no longer occur and a re-run now looks clean. The point stands without them:
  CI builds `aarch64` and never tests it, and architecture is the one shape dimension a corpus
  cannot vary because it is fixed at compile time. Restate it on that basis when picked up.

  A reviewer asked whether the suite was portable and it was not: three renderer fixtures and the
  shape corpus' banner builder each hard-coded `x86_64` while the row they are compared against
  publishes `std::env::consts::ARCH`. Measured on `i686-pc-windows-msvc`: five failures, every one
  `prose: "x86_64"` against `ndjson: "x86"`. CI BUILDS `aarch64` and never TESTS it, so a
  build-and-clippy matrix cannot see this class at all.

  Architecture is the one shape dimension the M2.12 corpus cannot vary, because it is fixed at
  compile time rather than chosen per report -- so the corpus that exists precisely to defeat shape
  blindness is blind here by construction, and only a second test target can close it. Add one
  (`i686-pc-windows-msvc` runs natively on the existing runners; `aarch64` would need its own).

- [ ] **M2.17** -- Cross the corpus dimensions instead of varying one at a time.

  Re-scoped by M3.5: the dimensions worth crossing are the ROW's. Crossing prose shapes that have
  since stopped being checked would have aimed at the retiring half.

  [tests/a_real_report_agrees_with_itself.rs](tests/a_real_report_agrees_with_itself.rs)'s `shapes()`
  builds each shape by taking `base()` and changing ONE thing. That makes every shape easy to read and
  is why the corpus found what it found -- but it means any renderer branch selected by TWO
  dimensions at once is unreachable by construction, and the corpus cannot report the gap because it
  does not know the branch exists.

  Measured: `CrossCheck` tags a `parse_incomplete` entry `- {caveat}` under `INCOMPLETE` but
  `(parse incomplete) {caveat}` under `DISAGREE`. Anomalies appeared only in an agreeing-counter
  shape and disagreements only in a zero-anomaly shape, so the second spelling was never rendered,
  and the anomaly-count rule was silently unread on every disagreeing report -- while the comment
  above it said it was read for every verdict. One hand-written crossed shape closed it, and the
  accounting test went red under sabotage only once that shape existed.

  Enumerate the dimensions the renderer actually branches on (verdict, bracket, coherence, the
  partitioning arm, presence of each diagnostic list) and generate the cross product, or a pairwise
  covering set if the full product is too slow. The corpus already asserts self-consistency and runs
  the fact accounting per shape, so nothing new has to be written to check them -- only to produce
  them. Until this lands, a shape that needs two dimensions must be added by hand, which is exactly
  the imagination-driven process M2.12 exists to replace.

- [ ] **M4.8** -- Have the queue-contention report carry the build identity that produced it, and
  have the capture scripts require it to agree.

  **Gap:** a run's report states its `host:`, `profile:` and `sampling:`, and the capture scripts
  now refuse a set whose runs disagree on any of those. None of it identifies the *instrument*. Two
  runs from different probe commits, on one machine, under one profile, pass that check -- and this
  is the capture where that matters most, because `M4.3` changed the drained procedure, so a
  pre-handshake and a post-handshake run would have their medians combined as though one procedure
  produced both. The instrument commit is recorded in the capture README, which is an assertion by
  whoever took the capture rather than something anything verifies.

  **Target:** `windows-placement-probe`'s `build_identity` module is the worked example -- a build
  script stamps the commit, the dirty flag and the build source into env vars that the binary reads
  at run time, and `BuildIdentity::current()` renders them. `windows-platform-probes` has no build
  script today, so this adds one. The report prints the identity beside the existing attribution
  lines, `requireOneConfiguration` in both capture scripts includes it, and the sabotage is two runs
  of different commits being refused.

  **Blocker recorded when queued:** none. The dependency exists next door and is already proven by
  that crate's own tests.

- [x] **M4.9** -- Route a placement probe's refusal-to-measure through the report sink instead of a
  panic, so a host that cannot be pinned is a reported observation rather than a crash.

  **Gap:** `windows-placement-probe`'s pinning helper asserted on `SetThreadGroupAffinity`, and
  `core_affinity::measure()` reaches it through `time_model_on` / `time_model_placed`. **The
  decision to stop was correct and did not change** -- the assert's own message makes the argument,
  that an unpinned thread "would produce a plausible number that answers a different question, and
  nothing in the output would say so." What was wrong is the *mechanism*: a panic bypasses
  `emit_report`, so the refusal landed on stderr while stdout carried a truncated report with no
  row saying why it stopped.

  **Measured, before and after.** With every pin forced to fail, the old binary wrote three lines
  to stdout -- banner, heading, blank -- and exited 101, with the explanation only on stderr. It
  now writes the refusal itself to stdout and exits 0, matching how the same binary already reports
  a topology-discovery failure.

  **Done:** `pin_current_thread` returns `io::Result`; `time_model`, `time_model_on`,
  `time_model_placed`, `median` and `peer_index_cache::measure` propagate it; both probe binaries
  handle it. The assert's message text is kept -- it was the right message, in the wrong channel.
  `PinSignal` still publishes `PIN_FAILED`, now on the early-return path as well as on an unwind,
  so the consumer never waits on a producer that has left.

  **Two things a later review corrected in the first attempt at this item, both the same mistake --
  a claim made about the code rather than from it.**

  `check_group_support` (then `assert_group_support`) tests the *same* predicate as the
  `usize::BITS` branch, over every discovered processor, and runs before any pin. So converting
  only the branch left this one deciding the outcome: the probe still died with banner, heading and
  nothing else. Fixing one site of a rule and leaving its twin is the blast-radius miss this
  repository has a rule against, and the before/after measurement recorded here did not catch it
  because it forced the pin argument rather than the discovered set. Both sites now refuse.

  `peer_index_cache::measure` **cannot** refuse -- it starts only unpinned runs -- so its `# Errors`
  section documented a failure mode no host can produce, and the entry here said "both probe
  binaries render it" as though both could decline. Only `probe-core-affinity` pins. The signature
  stays `Result` because its helpers are fallible in general; the docs now say that instead of the
  opposite.

  **Also corrected:** the refusal text was shared by both causes while being written for one. For
  the mask-width cause it claimed the processor "was reported by this machine's own topology, so
  this is unexpected rather than a limit of the tool" and invited a bug report -- the exact reverse
  of that cause, which *is* a limit of the tool. The explanation is now a parameter, verified in
  both directions.

  **One correction to this item as written.** The blocker recorded when queuing was **wrong** --
  `windows-placement-probe` is `publish = false` and absent from
  [.release-please-manifest.json](../../.release-please-manifest.json), and
  [check-commit-scope.ps1](../../tools/check-commit-scope.ps1) says in terms that such a crate
  "cannot be poisoned, because it is never released -- so it is not a finding." The `x-probe-*` row
  it called for is deferred to `M4.10`, which turned out to be a larger question than these two
  probes.

- [ ] **M4.10** -- Decide whether a probe's report owes a machine-readable row, and make the answer
  uniform.

  **Gap:** three of this crate's sixteen probe binaries emit an `x-probe-*` NDJSON line beside
  their prose -- `doorbell_cost`, `request_cost`, and `topology` through `topology_report`. The
  other thirteen emit prose only, `probe-core-affinity` and `probe-peer-index-cache` among them,
  and so does `queue_contention` despite being the probe whose captures are analysed by committed
  scripts. So a survey can mine three probes and must scrape or skip the rest.

  **This item was first written the other way round**, asserting that the two placement probes were
  the exception and that `queue_contention` emitted a row. Both halves were false: prose-only is
  the majority, by four to one. The correction changes what is being proposed -- not "bring two
  stragglers up to the norm" but "the crate has two conventions and no stated rule for which
  applies."

  **Target:** state the rule first, in [DESIGN-NOTES.md](DESIGN-NOTES.md), since it decides twelve
  more binaries than it decides these two: which probes owe a row, and what a refusal looks like in
  one. `report_unmeasured` already draws the "measured and declined" against "never ran"
  distinction for the topology probe, and the cost probes' `EVERY_LABEL` arrangement is the worked
  example for keeping a row and its prose from drifting. Then apply it.

  **Blocker recorded when queued:** none, but note this is a design decision before it is a code
  change, so it wants the engineer's call on scope rather than a unilateral sweep.


## M5 -- Carried over from M2: unblocked hygiene

**Nothing gates these.** They are grouped last by priority, not by dependency -- none of them touches
the report pipeline, so any of them may be pulled forward ahead of M4 at any time. They were
discovered during M2 and parked there under a heading none of them fit. M3, which gated M4 but never
gated these, is complete and archived.

IDs keep their M2 numbers, for the reason given under M4.

- [ ] **M2.13** -- Lint the completed-checklist archive mechanically in CI.

  Three bookkeeping defects reached review on this branch, and all three are decidable by a script: a
  `###` heading concatenated onto the previous line so it did not parse as a heading at all, three
  more headings with no blank line above them, an archived item body left `- [ ]` after being checked
  off, and a pre-existing entry's heading rewritten -- which the file forbids in its own second line.

  Assert, for every `COMPLETED-CHECKLIST.md`: every `###` heading is preceded by a blank line; no
  `- [ ]` remains; and the file has ZERO deleted lines against the merge base. The last one is the
  append-only invariant, and it is the one a human reviewer is least likely to notice.


- [x] **M2.14** -- Make the two authoring rules this branch earned actually bite. -> [completed 2026-09-13](COMPLETED-CHECKLIST.md#m214)

- [x] **M2.14.1** -- Give this crate a sabotage manifest, so "observed to fail" is a recorded artifact rather than a habit. -> [completed 2026-09-13](COMPLETED-CHECKLIST.md#m2141)

- [x] **M2.14.2** -- Add to CONTRACT INTEGRITY rule 1 the one thing this branch learned that it does NOT already say. -> [completed 2026-09-13](COMPLETED-CHECKLIST.md#m2142)

- [x] **M2.16** -- Repair the garbled `Report` doc comment, and drop the two counts that had rotted beside it. -> [completed 2026-09-16 UTC-04:00](COMPLETED-CHECKLIST.md#m216)
