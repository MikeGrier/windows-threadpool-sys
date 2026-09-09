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

- [ ] **M2.1** -- Add a report oracle to this crate: one shared executable definition of the
  correlations that must hold between the parts of a rendered report, checked against the rendered
  artifact rather than against internal state. Seed it with the three known invariants: an alarm in
  the prose implies the verdict is not `agree`; a fact rendered in both prose and NDJSON agrees across
  the two; an uncaveated hardware claim implies `!parse_in_doubt`. Model it on
  [../windows-file-watcher/src/contract.rs](../windows-file-watcher/src/contract.rs)'s
  `ContractChecker`, which is this repository's worked example and which existed unused while this
  probe was being written.

- [ ] **M2.2** -- Route every test that renders a report through the oracle, so the roughly
  twenty-five existing `report()` call sites inherit the checks and every future one does too. This is
  the step that makes it an oracle rather than three more tests: a test added beside the others checks
  one case, whereas binding the call sites checks every case anyone writes later. Verify the binding by
  sabotage -- change an invariant and confirm existing tests go red -- because a binding that only moves
  when its own test moves is cosmetic.

- [ ] **M2.3** -- Add the missing integration test: run `measure()` against the real host, render the
  report, and apply the oracle. At the time of M2 the crate had one integration test, asserting only
  that a probe writes to stdout, and none of the twenty-five `report()` calls rendered from a real
  measurement -- every one used a hand-built `Observation`, which can only contain states its author
  already imagined. On CI this runs across the whole hosted-runner fleet, which is where states no
  fixture anticipates will actually appear.

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

- [ ] **M2.6** -- Say precisely what `GetFullPathNameW` does, in the crate that owns it, and decide
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
