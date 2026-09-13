# Checklist: windows-platform-probes

Design decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md). This crate's *creation* is tracked
separately, in the workspace [CHECKLIST-thread-ambient.md](../../CHECKLIST-thread-ambient.md) milestone
M27; that file is feature-scoped and is deleted when its feature completes, so durable follow-up work
for the crate belongs here instead.

## M4 -- Carried over from M2: the items M3 gated

These were written under M2 and were blocked on M3, which is **now complete and archived** in
[COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md). They are unblocked.

Each targeted the prose-against-row machinery M3 retired or relocated, so none was merely delayed --
each was RE-SCOPED, and the re-scoping is what makes them safe to pick up. Those notes are below.
They were written in M3's preamble and moved here when M3 was archived, because they describe work
that is still open: an instruction for a pending item is not history, and leaving it in the archive
would have left this milestone pointing at a file it may not edit.

- **M2.4** was re-scoped by M3.2. The exploration is still worth doing and its instrument is
  unchanged, but what it hunts for changed: invariants over `Coherence`, `BracketOutcome` and
  `Verdict` as VALUES, and facts the row fails to publish -- not correspondences between two
  renderings. Its closing sentence, "promote only what proves meaningful into the oracle from M2.1",
  now means the invariant set from M3.2. The open question attached to it -- whether this generalises
  past this crate -- survives unchanged and is arguably sharpened, since a data-level invariant is
  easier to share than a text reader.
- **M2.5** was gated by M3.1 and M3.3, both landed. Establishing that the middle of three discoveries
  agreed produces a new FACT, which M3.1 says must reach the row rather than only the banner; and
  M3.3 changed how the banner is built. Written before those, it would have been written into
  machinery about to move.
- **M2.15** keeps its conclusion but loses its evidence. The five failures it cites were all
  `prose: "x86_64"` against `ndjson: "x86"` -- instances of exactly the correspondence M3.4 retired,
  so they can no longer occur and a re-run now looks clean. The underlying point stands without
  them: CI builds `aarch64` and never tests it, and architecture is the one shape dimension a corpus
  cannot vary because it is fixed at compile time. Restate it on that basis when picked up.
- **M2.17** was re-scoped by M3.5: the dimensions worth crossing are the row's, and crossing prose
  shapes that have since stopped being checked would have aimed at the retiring half.

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


- [x] **M2.14** -- Make the two authoring rules this branch earned actually bite. Re-planned
  2026-09-12; see the rationale below before implementing either sub-step. Both sub-steps done.

  **As originally written this item said "write two authoring rules into the repository
  instructions". Measurement says that would have been worse than useless.** The two rules it
  proposed -- *state the invariant, not the census*, and *a new test is not done until it has been
  observed to fail* -- ALREADY EXIST, in
  [.github/copilot-instructions.md](../../.github/copilot-instructions.md) under CONTRACT INTEGRITY
  rule 1 ("Prefer a derived fact to a restated one", and beneath it "verify the binding by
  sabotage: change the definition and confirm the consumer's BEHAVIOR changes"). Writing them again
  would add a second copy of a rule, which is the exact defect that section forbids and the exact
  mechanism -- restatement drift -- it exists to prevent.

  **And statement is demonstrably not the gap.** Both rules were in force on 2026-09-12 and both
  were violated: `09da7e9` claims "Sabotage-verified, EACH against the instrument it was meant to
  strengthen" and then names two sabotages for four fixes. The one that got none is the completeness
  guard, which a review found broken an hour later (M3.8). A rule cited in the commit that breaks it
  will not be repaired by a third copy of itself.

  **This repository has already solved this problem once, and not with a rule.** The
  [ci.yml](../../.github/workflows/ci.yml) `sabotage-harness` job records that the harness "accumulated
  fixes over eleven review rounds and thirteen of the later defects were introduced by earlier fixes,
  because every verification was a one-off command that was then discarded and nothing re-checked an
  earlier guarantee." That is M3.8's story verbatim. The answer then was a CI ratchet.

- [x] **M2.14.1** -- Give `windows-platform-probes` a sabotage manifest, so "observed to fail" is a
  recorded artifact rather than a habit.

  **Done.** [sabotage.json](sabotage.json), 7 entries, swept green: six `caught`, one `survives`,
  all behaving as declared. Not wired into CI, matching the two sibling manifests and the
  `sabotage-harness` job's own note that a sweep "rebuilds a crate per entry and is deliberately an
  occasional instrument".

  **The control is the entry that matters most here.** It rewords a prose line to carry the same
  fact and must SURVIVE, which turns this component's central decision -- the row is the machine
  contract, the prose is for a reader -- from a sentence into a measurement. If it is ever reported
  as caught, a test has started reading the prose again and that test is the defect.

  **The harness found a defect in the manifest that the authoring script missed, which is the
  lesson.** The entry for the escape-aware key reader anchored on `'\\' => escaped = true,`; the
  script checked uniqueness by whole-LINE equality and found one match, while the harness matches by
  SUBSTRING and found two -- the same arm appears in `malformation` at a deeper indent, and the
  shallower line is a substring of the deeper one. The script's check was a second, weaker
  implementation of the harness's rule, which is precisely the defect class this manifest exists to
  catch. The anchor was widened to the function signature; the harness remains the only authority on
  uniqueness.

  `tools/run-sabotage.ps1` exists, has its own tests, and runs in CI;
  [windows-placement-probe](../windows-placement-probe/sabotage.json) (9 entries) and
  [windows-waitable-queues](../windows-waitable-queues/sabotage.json) (39 entries) each carry a
  `sabotage.json`. **This crate carried none**, so every sabotage run while building M3 was ad-hoc
  PowerShell, discarded on the spot -- which is why a `git checkout` destroyed uncommitted work
  twice and a `.Replace` pattern silently matched two sites once. The manifest format's `find` must
  match EXACTLY ONCE, which is precisely the guard that hand-running lacked.

  Eight commits on this branch recorded their sabotages in the message, so the first pass was
  transcription rather than invention: the defect, the file and the test expected to redden were
  already written down.

  Include at least one `expect: "survives"` control. A manifest of nothing but `caught` cannot
  distinguish a suite that is watching from a suite that fails on any edit.

- [x] **M2.14.2** -- Add to CONTRACT INTEGRITY rule 1 the one thing this branch learned that it does
  NOT already say, and a pointer to the mechanism. A pointer, not a restatement.

  **Done.** Two paragraphs added to CONTRACT INTEGRITY rule 1 in
  [.github/copilot-instructions.md](../../.github/copilot-instructions.md). Neither restates the
  existing rule: the first says to sabotage the CLAIM a change makes rather than the symptom it
  cites, and the second says a sabotage is worth nothing once discarded and points at the manifest
  and the harness. The original M2.14 wording is nowhere in the diff, which was the point.

  The genuinely new fact: **when a fix claims to have removed a weakness, sabotage the claim rather
  than the symptom.** Rule 1 tells an author to prefer a derived fact over a restated one; it does
  not warn that an author may believe they derived one when they only MOVED the census. That is
  exactly what happened three times on a single guard -- strings, then a hand-written `ALL`, then
  generation -- each fix relocating the census somewhere harder to see while its commit message
  claimed the class was closed. The wording that would have caught it is about the CLAIM, and rule 1
  currently has no sentence about claims.

  > **-> DEPENDS ON M2.14.1:** the pointer has nothing to point at until the manifest exists.

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
