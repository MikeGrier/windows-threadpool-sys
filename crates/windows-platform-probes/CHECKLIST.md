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

- [ ] **M2.4** -- Explore, with the sparse matrix as the instrument, whether `Coherence`,
  `BracketOutcome` and `Verdict` carry invariants the row does not yet publish -- as VALUES on the
  observation, not as correspondences between two renderings -- and whether the sibling probes'
  renderers have the same gaps. Expect the matrix to be mostly empty; that is the expected shape and
  not a sign the exercise failed. **Record the vacuous results as well as the findings** -- "X and Y
  were examined and need not be related" is what stops the next person re-exploring the same cells,
  and is the half that normally evaporates. Promote only what proves meaningful into the invariant
  set from M3.2.

  Re-scoped by M3.2; the note at the top of this milestone gives the reasoning. **The item text
  above was rewritten when M3 was archived, to match**: it still asked for "the same correspondence
  failures" and for promotion "into the oracle from M2.1", both retired by M3, so a reader working
  the list linearly would have been sent after the half that no longer exists. Found by a review --
  and the lesson generalises, since a re-scoping note 25 lines above an item does not reach someone
  executing the item.

> **-> OPEN QUESTION for the engineer:** M2.4 may show this generalises past this crate, in which case
> the oracle belongs somewhere shared and the question becomes a repository-wide convention rather than
> a probe-crate one. That is a design decision, not a mechanical follow-on, and is deliberately left
> unanswered here.

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

- [ ] **M4.2** -- Give the measurement probes the controls needed to act on a dispersion finding,
  so "gather more data along this axis" does not require editing a `const` and rebuilding.

  **This item is deliberately small in software and large in guidance.** The diagnostic method
  belongs in [DESIGN-NOTES.md](DESIGN-NOTES.md) -- see
  [What to try first, and how to tell when you have reached the
  floor](DESIGN-NOTES.md#d-variance-is-a-finding) -- and this item exists only to make that method
  executable. The judgement stays with the person; the probe stops being the obstacle.

  **Gap:** [src/queue_contention.rs](src/queue_contention.rs) fixes every sampling parameter as a
  private constant -- `PUSHES_PER_PRODUCER` (50,000) and `REPETITIONS` (5) -- and `measure()` takes
  no arguments. The first move the design note prescribes on seeing a wide control is to lengthen
  the span and raise the repetition count on the unchanged configuration, which is currently a
  source edit and a rebuild. A control that cannot be turned is not a control, and the cheapest
  diagnostic step is the one being blocked.

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

- [ ] **M2.5** -- Make the banner describe the read the body describes.

  Gated by M3.1 and M3.3, both landed: establishing that the middle of three discoveries agreed
  produces a new FACT, which M3.1 says must reach the row rather than only the banner, and M3.3
  changed how the banner is built. Written before those, it would have been written into machinery
  about to move.

  A probe run performs
  **three** independent `MachineMemoryTopology::discover()` calls: `Fingerprint::discover()` for the
  banner, `measure()`'s own discovery for the body, and `Fingerprint::discover()` again. `attribution`
  compares only the two endpoints, so equal endpoints print an unqualified banner without establishing
  that the middle read agreed with them.

  **The uncovered window is narrow, and worth stating precisely so it is not over- or under-sold.**
  `measure()` brackets its counters around its own discovery, so a processor, group or NUMA change
  during the middle read is already caught as `BracketOutcome::Changed`. What no counter reaches is
  cache and efficiency-class structure. So the reachable case is a run where the cache structure
  differs between the endpoint reads and the middle read while the processor, group and NUMA counts
  stay identical -- near-impossible on real hardware, since caches do not change without processors
  changing, but reachable on a hypervisor returning inconsistent `GetLogicalProcessorInformationEx`
  results, which is exactly the population this probe exists to survey.

  Prefer **construction over comparison**: return the measured topology from `measure()` (as a sibling
  function, so the six existing `measure()` callers are untouched) and build the banner with the
  already-public `Fingerprint::from_topology`. The banner then describes the body's read *by
  construction* and the contradiction becomes unrepresentable, rather than detected by a third
  comparison that is itself new prose able to drift. The endpoint reads still earn their place: they
  catch structural change across the wider window that the counter bracket cannot see.

  **Express it where M3 put the invariants, not in the M2.1 oracle.** "The banner describes the
  measured read" is an invariant over the OBSERVATION, so it belongs in the invariant set from M3.2
  and, if the fact reaches the artifact, in the row schema -- checked on every rendered report through
  the renderer binding rather than asserted once in a single test. **These two paragraphs were
  rewritten when M3 landed**: they asked for the relation to be expressed in the M2.1 prose oracle,
  which M3.4 deleted, so an executor would have gone looking for machinery that no longer exists.
  Found by a review, and the same defect M2.4 carried.

  Reviewer disagreement is recorded deliberately, because it is evidence about the instrument rather
  than noise: across two rounds one reader raised this twice while two others cleared it, one of them
  explicitly after being pointed at the question. Nothing in the suite decides it either way, which is
  itself the argument for making it an invariant rather than a test.

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


## M5 -- Carried over from M2: unblocked hygiene

**Nothing gates these.** They are grouped last by priority, not by dependency -- none of them touches
the report pipeline, so any of them may be pulled forward ahead of M4 at any time. They were
discovered during M2 and parked there under a heading none of them fit. M3, which gated M4 but never
gated these, is complete and archived.

IDs keep their M2 numbers, for the reason given under M4.

- [ ] **M2.7** -- Decide whether the other nine probe steps in CI should carry `if: '!cancelled()'`,
  and apply or record the decision.

  **Measured 2026-09-09:** twelve probe steps in [ci.yml](../../.github/workflows/ci.yml), of which
  three are guarded -- topology, and the doorbell/request pair added with this note. The other nine
  (`error mode`, `handle state`, `worker context`, `pool growth`, `device map`, `IoRing`,
  `completion port`, and both halves of the long-path pair) are skipped whenever an earlier step in
  the job fails, because Actions defaults to `if: success()`.

  The argument for guarding is already written at the topology step and is not specific to it: a
  probe step exists to emit diagnostics, so skipping it on failure suppresses it in exactly the run
  that wanted it. **The long-path pair is the sharpest case** -- its own comment says either half
  alone "says nothing", since the finding is the difference between two executables, so a partial
  run of that pair is worse than useless.

  **It is queued rather than done because there is a real tradeoff, and it is an operational call.**
  `!cancelled()` also runs the step when the *build* failed, where `cargo run` cannot compile and
  the step turns from skipped (grey) into failed (red). That trades quieter broken-build output for
  better broken-test output. The topology step already took that trade; whether all twelve should is
  a judgement about how the CI log is read, not something to settle by consistency alone.

- [ ] **M2.8** -- Carry the OS error in the remaining Win32 assertion messages.

  `last_os_error()` (or a raw `GetLastError`) is in the messages in `doorbell_cost`, `request_cost`
  and `handle_state`, and missing from four sites in probes this peel did not touch:
  `completion_port.rs:224` and `:234` ("create a completion port"), `ioring.rs:320` ("create the
  probe pipe"), and `pool_growth.rs:62` ("create the gate event"). Each says what was being attempted
  and not why it failed, which is the whole of what a CI log can offer someone who cannot rerun under
  a debugger.

  Two rules worth carrying over, both learned the expensive way in this peel. Read the error
  **immediately after the single call whose failure is reported** -- a code attached to a condition
  spanning two calls belongs to whichever ran last, not whichever failed, and can print "The
  operation completed successfully" under a message saying something failed. And attach it only to a
  condition that is genuinely an OS failure: a call that returned a size rather than an error should
  not carry one, since `GetLastError` says nothing about it.

- [ ] **M2.9** -- Stop `request_cost` calling a cross-host ratio "the finding".

  [src/request_cost.rs](src/request_cost.rs) ends its module doc with "Absolute values are
  host-specific; the **ratios against the doorbell and the atomic** are the finding." The ratios that
  [src/bin/request_cost.rs](src/bin/request_cost.rs) actually prints divide THIS host's measurement by
  `DOORBELL_NS_REFERENCE` / `ATOMIC_NS_REFERENCE`, which are constants measured on the Snapdragon X2
  development machine. A ratio with this host's numerator and another host's denominator is neither a
  same-host ratio nor a portable finding, and the emitted report says as much two lines later:
  "re-read that probe on this host before trusting them". So the module doc promotes to "the finding"
  exactly the number its own output tells the reader not to trust.

  **Pre-existing, and deliberately not fixed in the `GetFullPathNameW` peel (PR #86) that found it.**
  It arrived in `ae1e39f`, is already on `main`, and is outside that branch's diff; folding it in
  would have put an unrelated behavioural change into a documentation peel that had already run to
  nineteen review rounds.

  The fix is a decision, not a sweep, which is why this is queued rather than taken: either compute
  both figures on the same host and run (the probe would have to measure the doorbell itself, or read
  a companion artifact), or keep the fixed references and demote them in the prose from "the finding"
  to a labelled cross-host comparison. The first is more useful and more work; the second is honest
  and cheap. Same defect class as PR #86's subject -- a claim stated more strongly than the evidence
  supports -- so whichever is chosen, the wording has to end up matching what the numbers can carry.

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

- [ ] **M2.16** -- Repair the garbled `Report` doc comment, and drop the two counts that have already
  rotted beside it.

  [src/report.rs](src/report.rs) opens its `Report` sink doc with a dangling fragment -- "A [`Report`]
  a renderer can `writeln!` into directly." followed by a blank line and then "is arithmetic. Every
  renderer writes through ..." -- so a sentence was lost in an edit, and "moves only 18 renderer
  signatures." is followed by a bare repeat of the word "signatures." Introduced 2026-09-09 by
  `b5594860` and `3827dc32`, both already on main; found while sweeping a count defect on the report
  -oracle branch, where the file was out of scope to touch.

  Both surviving numbers in that passage are censuses that have since drifted. It claims **332
  `writeln!` sites**; measured now, 354. [DESIGN-NOTES.md](DESIGN-NOTES.md) restates the same 332,
  so the two must be fixed together or they drift apart again. Replace them with the invariant the
  passage is actually arguing -- that `String` already implements `fmt::Write`, so every existing
  write site stands untouched and only the renderer signatures move -- which is what makes the point
  and cannot rot. This is the same defect class as CONTRACT INTEGRITY rule 1 in
  [.github/copilot-instructions.md](../../.github/copilot-instructions.md), which M2.14 exists to
  make bite.
