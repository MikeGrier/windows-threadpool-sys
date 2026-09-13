# Checklist: windows-platform-probes

Design decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md). This crate's *creation* is tracked
separately, in the workspace [CHECKLIST-thread-ambient.md](../../CHECKLIST-thread-ambient.md) milestone
M27; that file is feature-scoped and is deleted when its feature completes, so durable follow-up work
for the crate belongs here instead.

## M3 -- Make the encoded row the contract, and stop checking the prose against it

Decided in [DESIGN-NOTES.md](DESIGN-NOTES.md) -> [The encoded row is the contract; the prose is
not](DESIGN-NOTES.md#d-encoded-row-is-the-contract), from the session in
[design-sessions/DESIGN-SESSION-2026-09-12-what-the-oracle-should-read.md](design-sessions/DESIGN-SESSION-2026-09-12-what-the-oracle-should-read.md).

The row is a machine contract mined across a fleet; the prose is for a reader. They carry different
obligations -- the row must be **correct**, enforced by machine; the prose must be **accurate and
readable**, enforced by review. Nothing is required to hold *between* them.

Re-checked against the code rather than against M2's account of it: the original defect was fixed,
and what it left behind was larger. The row published `not_compared`, `parse_incomplete` and
`enumeration_anomalies` as **counts**, where the prose printed each entry's text. A survey reading
`"parse_incomplete":1` could not tell *the probe detected a bug in itself* from *a core record
contradicted itself* from *this topology was not measured from a running machine*. **The row was
impoverished relative to the prose** -- the artifact that gets mined carried less than the artifact
that gets read. M3.1 has since closed that particular gap; the rest of the milestone is about which
artifact carries the contract, and stands whole.

**M2 completed with this decision**, and its ten open items were re-sequenced rather than reworked.
The milestone's own work -- the oracle, the binding, the real-host test, the derived fact set, the
partitioning discriminator and the shape corpus -- is done and archived. The leftovers had
accumulated under a heading none of them fit, and they split by whether M3 gates them: four are in
M4 below, six in M5. M2.18 is the exception, dissolved rather than moved.

- **M2.18 (typed banner) is dissolved into M3.3**, not carried over. It was the smallest instance
  of "should a report be a value a writer renders, or a string the renderer concatenates", and
  answering it alone would have typed one parameter while leaving the shape everywhere else.
- **M2.4** is re-scoped by M3.2. The exploration is still worth doing and its instrument is
  unchanged, but what it hunts for changes: invariants over `Coherence`, `BracketOutcome` and
  `Verdict` as VALUES, and facts the row fails to publish -- not correspondences between two
  renderings. Its closing sentence, "promote only what proves meaningful into the oracle from M2.1",
  now means the invariant set from M3.2. The open question attached to it -- whether this generalises
  past this crate -- survives unchanged and is arguably sharpened, since a data-level invariant is
  easier to share than a text reader.
- **M2.5** is gated by M3.1 and M3.3. Establishing that the middle of three discoveries agreed
  produces a new FACT, which M3.1 says must reach the row rather than only the banner; and M3.3
  changes how the banner is built. Written first, it would be written into machinery about to move.
- **M2.15** keeps its conclusion but loses its evidence. The five failures it cites were all
  `prose: "x86_64"` against `ndjson: "x86"` -- instances of exactly the correspondence M3.4 retires,
  so afterwards they would not occur and a re-run would look clean. The underlying point stands
  without them: CI builds `aarch64` and never tests it, and architecture is the one shape dimension
  a corpus cannot vary because it is fixed at compile time. Restate it on that basis when picked up.
- **M2.17** is re-scoped by M3.5: the dimensions worth crossing become the row's, and crossing prose
  shapes that are about to stop being checked would aim at the retiring half.
- **M2.7, M2.8, M2.9, M2.13, M2.14 and M2.16 are gated by nothing** and are in M5. M2.9 (a
  cross-host ratio called "the finding") and M2.14 (two authoring rules) are if anything reinforced:
  under this decision prose accuracy is a review obligation rather than a machine-checked one, which
  puts more weight on both.

- [x] **M3.1** -- Publish each diagnostic as itself, not as a count.

  `not_compared`, `parse_incomplete` and `enumeration_anomalies` reach the row as
  `check.parse_incomplete.len()` and its two siblings, so the fact that a mining pass most needs --
  *which* condition occurred -- exists only in prose. Publish the entries, and give each a stable
  machine-readable discriminant rather than the human sentence, so a survey can group by condition
  without matching on English that is free to be reworded. The sentences stay in the prose, where
  rewording them is harmless.

  **The rule this establishes, which is the durable half:** a renderer may not tell a reader
  something the row cannot tell a survey. A cardinality is not a statement of the fact.

  **Done.** Three enums in `topology::diagnostic` -- 21 + 6 + 3 variants, one per condition -- each
  carrying its data, rendering its sentence through `Display`, and naming itself through `code()`.
  `CrossCheck`'s three `Vec<String>` became `Vec<Diagnostic>`, and the row publishes arrays of codes
  where it published `.len()`. The prose is byte-identical: the loops write `{entry}` and `Display`
  emits the same sentences.

  **The wire format changed**, deliberately and not additively: `"parse_incomplete":1` is now
  `"parse_incomplete":["partitioning_summary_missing"]`. The count is still available as the list's
  length, so nothing is lost, and publishing both would be a restatement that can drift. Same shape
  as the `efficiency_classes` correction that preceded it.

  Sabotage-verified, each mutation injected on its own line and reverted: renaming
  `PartitioningSummaryMissing`'s code reddens only
  `the_row_names_the_probes_own_bug_when_it_detects_one`; making the row keep only the first
  condition reddens the two list tests, through the bound oracle's count rule; mislabelling
  `TrailingBytes` reddens only `an_anomaly_reaches_the_row_as_its_kind`.

  **The substring-to-variant conversion cost two assertions their discrimination, found by review.**
  `c.contains("no online processors and processor groups")` became
  `matches!(c, MeasuredButCountsAbsent { .. })`, which holds when the entry names only ONE of the
  two -- exactly what the test forbids -- and the loop's labels stopped being asserted at all.
  Measured: with `absent` truncated to its first entry the whole suite stayed green at 249 passed.
  Both now assert the variant's `absent` payload, and both were observed to fail -- the truncation
  reddens the both-absent test, and swapping the two names reddens both. The general lesson is that
  converting an assertion from a substring to a variant DROPS whatever the substring discriminated
  inside the payload; the variant is the weaker claim unless the payload comes with it.

  **Which conditions are listed is deliberately not compared against the prose.** The code and the
  sentence come from one variant, so there is no second implementation to disagree through -- the
  correspondence holds by construction, which is stronger than a check. What remains checkable, and
  is checked, is that both renderings list the same NUMBER. Found while converting the accounting
  instrument: a mutation that swapped one code for another went unnoticed on the
  `verdict incomplete` shape, because the oracle reads the length. `corruptions` now APPENDS a code
  rather than substituting one, so the length always differs.

- [ ] **M3.2** -- Assert the surviving correspondences as invariants on the observation, before
  rendering.

  Alarm-against-verdict, diagnostics-against-verdict and counters-against-verdict are the three
  oracle rules that survive the decision. They stop being comparisons of two rendered texts and
  become predicates over `Observation` and `CrossCheck` -- `SummaryMissing` implies the verdict is not
  `agree`, a non-empty `parse_incomplete` implies the verdict is not `agree`, and so on. No parser is
  involved, and the check runs whether or not anything was rendered.

  Each one must be sabotage-verified on arrival: delete the invariant, confirm the suite reddens,
  restore it. A predicate that cannot fail is the failure mode this crate keeps meeting.

- [ ] **M3.3** -- Emit the row from a typed value through one writer.

  The row is built today by interpolating every value positionally into a `concat!` template.
  Two defect classes follow from that construction and both are closed by replacing it, not by
  checking it:

  **Injection.** Measured on PR #88: an `io::Error` containing `{` was selected as the report's
  machine-readable row, so the oracle checked the caller's text instead of the probe's. Caller text
  reaching the mined artifact is contamination of the contract.

  **Field order and labelling.** A field's name and its value are related only by counting
  positions, so a reordered argument or a miscounted placeholder yields mislabelled data that
  still parses, which nothing downstream can detect. Stated as the coupling rather than as a
  count of placeholders: that count was written twice and wrong twice within an hour.

  A typed row struct plus a single writer that escapes strings makes both unrepresentable. Write the
  writer here rather than adding a serialization dependency -- this crate has none and the row is
  one flat object.

  **Carry each diagnostic's DATA, which M3.1 left behind.** M3.1 publishes a condition's code but
  not the values its variant holds -- a survey learns `contradictory_cores` without learning that
  three cores contradicted themselves. The variants already carry those values, for `Display`; what
  stopped M3.1 publishing them is that the row is still a positional `concat!` template, where a
  nested per-entry object has to be hand-assembled. Once the row is typed this is a field like any
  other. Not deferred for want of a consumer -- the shape of the row is the blocker, and it is this
  item. This subsumes M2.18: the banner becomes a typed field like any other, and the
  question of who may construct one is answered by the row's constructor rather than separately.

- [ ] **M3.4** -- Retire the prose-against-row correspondences and the parsers that serve only them.

  Of 38 top-level functions in [src/report_oracle.rs](src/report_oracle.rs), ten are correspondence
  rules, four are comparison helpers, and **twenty-three exist only to extract values back out of
  rendered text**. With M3.2 and M3.3 landed, that extraction layer has no remaining consumer.

  What stays is a thin check that the row is **well-formed** -- it parses, it carries the expected
  key set, and it is the only such line in the report. That is not a correspondence; it is the
  writer's own output being checked, and the writer is the one place structure cannot check itself.

  Retire, do not merely stop calling. Dead extraction helpers left in place are a second grammar for
  a format that no longer has two readers.

- [ ] **M3.5** -- Re-aim the shape corpus and the fact accounting at the row.

  **The instrument enumerates in one direction only, and the other direction is where M3.1's rule
  lives.** `ndjson_keys` reads the ROW's keys and requires each to be classified, so it asks "does
  anything read this key?" -- never "does the prose state a fact the row omits?". A fact with no key
  is outside the set of things it can have an opinion about.

  Measured, and this is how it was found rather than reasoned: `CrossCheck::disagreements` reached
  the prose as a listed entry per disagreement and reached the row as nothing at all. `cross_check`
  said `disagree` without saying WHICH counter did, which is the same shape as the defect the
  milestone came from. It survived 41 review rounds, a zero-survivor mutation sweep and the fact
  accounting, because every one of those instruments starts from what the row publishes. A review
  found it by reading the enum and asking who called `code()` -- the answer was nobody.

  So the accounting needs a second enumeration, from the PROSE's facts to the row's keys, or the
  rule "a renderer may not tell a reader something the row cannot tell a survey" has no instrument
  behind it and holds only as long as someone remembers it.


  [tests/a_real_report_agrees_with_itself.rs](tests/a_real_report_agrees_with_itself.rs) enumerates
  the facts a report publishes and measures, by mutation, which are read. The instrument is sound and
  the target changes: enumerate the row's fields, and require each to be read by an invariant or
  explicitly classified as unread. Its corpus of shapes keeps its purpose -- it exists to defeat the
  imagination-driven fixture, which the decision does not change.

  The prose half becomes a rendering test: the renderer emits what it is supposed to emit, judged on
  its own terms rather than against the row.

- [ ] **M3.6** -- Split [DESIGN-NOTES.md](DESIGN-NOTES.md) into Tier 1 and Tier 2.

  Measured: 88 KiB, which is **XL** on the repository's byte scale, and the default posture at XL is
  to split unless the module is indivisible. It is not -- it carries current decisions and a large
  volume of how-we-got-here reasoning, which is exactly the Tier 1 / Tier 2 fracture the repository
  instructions describe.

  Move the rationale to `DESIGN-RATIONALE.md`, cross-referenced by decision anchor, leaving Tier 1
  stating what was decided and what forced it. The decision added by this milestone is written to be
  split that way already, so it is the worked example rather than the hard case.

## M4 -- Carried over from M2: the items M3 gates

These were written under M2 and are blocked on M3 above: each one targets the prose-against-row
machinery that M3 retires or relocates, so doing them first means doing them twice. M3's preamble
says which M3 item gates each of them.

**The IDs keep their M2 numbers deliberately.** [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md) is
append-only and its entries are immutable, and two archived entries already cite M2.4 and M2.14 --
so renumbering would leave dangling references in a file that may not be edited to repair them.
Stable IDs cost a mismatch between an item number and its milestone; renumbering would cost
correctness in the archive.

- [ ] **M2.4** -- Explore, with the sparse matrix as the instrument, whether the same correspondence
  failures exist for `Coherence`, `BracketOutcome` and `Verdict`, and in the sibling probes' renderers.
  Expect the matrix to be mostly empty; that is the expected shape and not a sign the exercise failed.
  **Record the vacuous results as well as the findings** -- "X and Y were examined and need not
  correspond" is what stops the next person re-exploring the same cells, and is the half that normally
  evaporates. Promote only what proves meaningful into the oracle from M2.1.

> **-> OPEN QUESTION for the engineer:** M2.4 may show this generalises past this crate, in which case
> the oracle belongs somewhere shared and the question becomes a repository-wide convention rather than
> a probe-crate one. That is a design decision, not a mechanical follow-on, and is deliberately left
> unanswered here.

- [ ] **M2.5** -- Make the banner describe the read the body describes. A probe run performs
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

  **This belongs to M2 rather than beside it:** "the banner describes the measured read" is a
  correspondence invariant, so it should be expressed in the M2.1 oracle and checked on every rendered
  report, not asserted once in a single test.

  Reviewer disagreement is recorded deliberately, because it is evidence about the instrument rather
  than noise: across two rounds one reader raised this twice while two others cleared it, one of them
  explicitly after being pointed at the question. Nothing in the suite decides it either way, which is
  itself the argument for the oracle.

- [ ] **M2.15** -- Run the probe suite on a second architecture in CI.

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
the report pipeline, so any of them may be pulled forward ahead of M3 or M4 at any time. They were
discovered during M2 and parked there under a heading none of them fit.

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


- [ ] **M2.14** -- Write two authoring rules into the repository instructions, both earned on this
  branch.

  **State the invariant, not the census.** "14 keys read, 3 unread" added nothing that "every key is
  classified" does not, and it was wrong -- written by eyeballing a list rather than counting it, in
  the commit documenting a fix for exactly that defect class. Where a number is genuinely load
  bearing, it must come from a command run in the same action that writes it.

  **A new test is not done until it has been observed to fail.** Every vacuous test on this branch was
  written green and stayed green until a reviewer thought to break something: a guard that matched a
  violation's VARIANT where only its FACT established the point, and a fixture whose `.replace()` of
  `[1]` matched nothing because the report rendered `[0]`. Sabotage belongs at authoring time, not at
  review time.

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
  and cannot rot. This is the same defect class as M2.14's first authoring rule.
