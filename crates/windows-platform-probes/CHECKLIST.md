# Checklist: windows-platform-probes

Design decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md). This crate's *creation* is tracked
separately, in the workspace [CHECKLIST-thread-ambient.md](../../CHECKLIST-thread-ambient.md) milestone
M27; that file is feature-scoped and is deleted when its feature completes, so durable follow-up work
for the crate belongs here instead.

## M2 -- Check correspondence between the report's parts, not just each part

A pull-request review found a state where [src/topology_report.rs](src/topology_report.rs) printed
`BUG IN THIS PROBE ... Nothing below about cache partitioning can be trusted` while `cross_check` had
no branch for that state, so the verdict could print `=> agree` two paragraphs below. Twenty-eight
rounds of per-artifact review and a zero-surviving-mutant `cargo-mutants` result had both passed over
it, because every function involved was correct on its own terms and the defect lived in the relation
between two of them.

See [DESIGN-NOTES.md](DESIGN-NOTES.md) -> [The defects that survived were correspondence
failures](DESIGN-NOTES.md#d-correspondence-failures) for why each instrument was structurally
incapable of finding it, and for the matrix-as-exploration / oracle-as-durable split this milestone
implements.

The three correlations below are known to be real because each was violated. They are not a
speculative list to extend by imagination -- a fourth is added when a fourth contradiction is found.

- [x] **M2.1** -- Add a report oracle to this crate. [src/report_oracle.rs](src/report_oracle.rs),
  seeded with the three known invariants and modelled on `ContractChecker`, including its
  as-many-must-accept-as-must-reject discipline. Reasoning in
  [DESIGN-NOTES.md](DESIGN-NOTES.md#d-correspondence-failures).

  It relates two things **already visible in the report** and re-derives nothing, because a second
  implementation of the rendering rules would be a check of the copy rather than of the contract.
  The gating rule is the interesting boundary: `parse_in_doubt` is
  `!disagreements.is_empty() || !parse_incomplete.is_empty()`, and the NDJSON publishes
  `parse_incomplete` as a *count*, so the oracle reads that count and the `disagree` verdict -- the
  two visible shadows of the definition. That coupling is what M2.2's sabotage must confirm.

  **Validated against a real rendered report, not only fixtures.** The prose labels were confirmed
  against a live `probe-topology` run, a test corrupts each double-rendered value in turn so a
  drifted label fails loudly rather than silently reading nothing, and the historical defect injected
  into a real report is reported twice -- once for the prose verdict, once for the NDJSON.

  **The first injection silently did nothing and nearly inverted the conclusion.** Its anchor,
  `cross-check:`, does not occur -- the real text is `cross-check against independently read Win32
  counters:` -- so the "defective" report was identical to the clean one and the oracle correctly
  found nothing, which read as the oracle being blind. A sabotage that fails to apply is
  indistinguishable from an instrument that fails to fire unless the injection asserts it changed
  something.

- [x] **M2.2** -- Route every test that renders a report through the oracle. Bound inside
  `topology_report::report` and `report_unmeasured` under `cfg(test)`, so all **26** existing call
  sites inherit it without being touched and every future one does too -- no author has to remember.

  `cfg(test)` rather than always-on: a real probe run must still print a contradictory report rather
  than panic, because a self-contradicting report is a finding *about this probe* and suppressing it
  would destroy the evidence. The real-host path is M2.3.

  **The sabotage measured both directions, which is what makes it evidence rather than a gesture.**
  Emitting the processor count where the core count belongs -- a pure correspondence defect, both
  renderings individually well-formed -- turns **13 tests red**, all in `tests` and none in
  `report_oracle::tests`. Among them is `every_report_carries_the_banner_and_title`, written for
  something else entirely, which is exactly the point: the cases most likely to catch the next
  contradiction are the ones nobody aimed at it.

  With the same defect in place and the binding removed, **all 173 tests pass**. The existing suite
  cannot see the defect at all, so the detection is the oracle's and the binding is what delivers
  it. Had only `report_oracle::tests` gone red, the binding would have been cosmetic.

- [x] **M2.3** -- Add the missing integration test.
  [tests/a_real_report_agrees_with_itself.rs](tests/a_real_report_agrees_with_itself.rs) composes the
  report exactly as `probe-topology` does -- two fingerprint reads, a real `measure()`, `attribution`
  -- and applies the oracle. Explicitly, because an integration test links the lib without
  `cfg(test)`, so M2.2's binding does not reach it.

  **It asserts nothing about this machine.** A test expecting a processor count or a cache level
  would fail on the next runner shape rather than on a defect, and would be loosened until it
  asserted nothing. It checks only that the report's parts agree *with each other*, which every host
  must satisfy -- including one whose topology cannot be read at all.

  **A second test exists because the first can pass vacuously**, and that is not hypothetical. If the
  renderer drifts from the oracle's prose labels, every lookup returns `None`, every comparison is
  skipped, and the real assertion passes having checked nothing. So each of the four double-rendered
  counts is corrupted in this host's own report and a violation is required. Verified by widening a
  prose label by one space: the guard failed naming `"packages":`, **while the primary test still
  passed** -- exactly the vacuous green it exists to expose.

  The corruption asserts it changed something first, per M2.1's lesson.

- [x] **M2.4** -- Explore with the sparse matrix. Full record, findings and vacuous cells both, in
  [DESIGN-NOTES.md](DESIGN-NOTES.md#d-correspondence-failures).

  **The prediction held on the axis it named and failed on one it did not.** `Coherence` and
  `BracketOutcome` are each rendered exactly once -- `Coherence` never appears in prose or NDJSON at
  all, `BracketOutcome` only in the banner -- so neither can contradict itself and both cells are
  genuinely empty. `Verdict` was already covered.

  The productive axis turned out to be **facts, not state enums**: six more were rendered twice with
  nothing comparing them, and all six are now promoted -- NUMA domains and those without processors,
  cache domains per level, the outermost partitioning level, domains per policy, and the two
  independently-read Win32 counters against the enumeration.

  Those last two are a **different rule shape** and the closest to what this probe is for: the
  counters exist so a mismatch is a finding, so one contradicting the enumeration under an `agree`
  verdict is the original defect in its purest form. `GetNumaHighestNodeNumber` is deliberately
  excluded and the exclusion is pinned by a test -- it is the largest node *number*, not a count, so
  comparing it would manufacture a disagreement on any sparsely-numbered machine.

  Reading `caches` and `policies` forced the field reader to balance brackets rather than stop at the
  first closer; `caches` is an array *of objects*, so the naive read saw only its first entry and
  would have skipped every later cache level in silence.

  All eight promoted cells are proved live against this host's real report by the M2.3 guard.

  > **-> SCOPE FINDING:** `probe-doorbell-cost` and `probe-request-cost` render **every measured
  > figure twice**, prose table and NDJSON, with nothing comparing them -- the same class, in two
  > more probes. Queued as M2.9 rather than taken here, because extending the oracle past one
  > renderer is a design question about where it should live, not a mechanical follow-on.

- [x] **M2.9** -- **Decided: if two renderings must match, they come from a common source.** Not a
  third oracle rule set -- both cost probes now walk the same `Observation::timings` the prose table
  walks, with a `json_key` function deciding only what the machine-readable rendering calls each
  entry. Reasoning in [DESIGN-NOTES.md](DESIGN-NOTES.md#d-correspondence-failures).

  This is strictly stronger than extending the oracle, and cheaper. An oracle rule finds a
  contradiction that already exists; deriving both renderings from one value means there is none to
  find. It also deleted code rather than adding it: ten hand-named NDJSON fields and a `get` closure
  went, because naming each figure separately was what made the two renderings independent.

  The `json_key` gate panics on a label it does not know, so a figure added to `measure` reaches both
  renderings or fails loudly -- it cannot reach one only. Verified by adding an unnamed timing: the
  probe printed its prose row and then died naming the missing key. (The row appearing before the
  panic is M1.2's streaming, which is how a reader sees how far it got.)

  One test remains, and its job is narrow: the derivation is structural in the source, so what is
  left to check is that the structure survives rendering, formatting and the process boundary. It
  reuses the crate's own `json_key` rather than restating the pairing, since a test carrying its own
  copy would be checking the copy.

  **Its emptiness guard fired on the first run**, and that is worth recording: `request_cost`'s table
  has ratio columns after the figure, so a row parser requiring exactly two tokens matched nothing
  and the test would have passed having compared zero rows.

  > **-> REMAINING SCOPE:** this closes the class in the two cost probes. Whether the same
  > common-source rule should be applied to `topology_report`, whose prose and NDJSON are still
  > written separately and are guarded by the M2.1 oracle instead, is a larger change and is not
  > queued yet -- the oracle covers it today, and the eight promoted cells are what make that
  > coverage real.

> **-> ANSWERED (M2.9):** it did generalise, and the answer was not to move the oracle. If two
> renderings must match they come from a common source, so the pairing is structural and there is
> nothing for an oracle to check. That is a repository-shaped answer -- it is the same rule the root
> [DESIGN-NOTES.md](../../DESIGN-NOTES.md) states as preferring a derived fact to a restated one --
> but it needed no shared code to apply, because what generalises is the principle rather than a
> mechanism.

- [x] **M2.5** -- Make the banner describe the read the body describes. A probe run performs
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

- [x] **M2.6** -- Say precisely what `GetFullPathNameW` does, in the crate that owns it, and decide
  whether it is still the call `prepare` wants. Two successive descriptions in the cost probe were
  each wrong in the same direction: *a syscall cost*, which a timing loop cannot establish, and then
  *lexical*, which it also is not. The probe now states the cost and declines the mechanism, which is
  honest but leaves the question open one layer down.

  [../windows-namespace-request-sys/src/full_path.rs](../windows-namespace-request-sys/src/full_path.rs)
  carries the same imprecision, and is the crate that owns the answer: its module doc says "This call
  is **lexical**. It resolves relative components and `.`/`..` against the process current
  directory". Those two sentences disagree -- consulting the current directory is process state, and
  for a drive-relative path (`C:foo`) it also reads the per-drive current directory held in the
  `=C:` environment variables. "Touches no filesystem" is the claim that holds; "lexical" is not.

  **The mono-repo rule says fix the layer, so the correction belongs in
  `windows-namespace-request-sys`, not in the probe that consumes it.** It is queued rather than
  taken because that crate is outside this peel and is release-managed, so a docs change there is its
  own commit with its own scope.

  The decision half is the part worth an engineer's attention rather than a sweep. A genuinely
  lexical canonicalizer exists -- `PathCchCanonicalizeEx`, or `PathAllocCanonicalize` -- and would be
  cheaper, with no process state read at all. **It is very likely the wrong call anyway**, because
  resolving against the current directory *at submission* is the property the namespace design is
  buying: the CWD is shared mutable state, so a relative path means something different depending on
  when it is resolved, and pinning that on the submitting thread is the whole point. Record that
  conclusion explicitly, with the alternative named, so the next reader does not re-derive it -- and
  if it is wrong, the cheaper call is sitting there.

  Also worth settling while the question is open: whether `GetFullPathNameW` can enter the kernel at
  all on any path this crate takes. The probe measured ~212 ns for a build on x86_64 and declines to
  say what that is made of; the owning crate could say, and a reader of either would then stop
  guessing.

- [x] **M2.7** -- Decide whether the other nine probe steps in CI should carry `if: '!cancelled()'`,
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

- [x] **M2.8** -- Carry the OS error in the remaining Win32 assertion messages.

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
