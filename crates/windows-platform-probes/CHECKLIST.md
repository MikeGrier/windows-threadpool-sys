# Checklist: windows-platform-probes

Design decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md). This crate's *creation* is tracked
separately, in the workspace [CHECKLIST-thread-ambient.md](../../CHECKLIST-thread-ambient.md) milestone
M27; that file is feature-scoped and is deleted when its feature completes, so durable follow-up work
for the crate belongs here instead.

## M1 -- Stream a probe's report as it is measured

The report sink introduced with [src/report.rs](src/report.rs) has each renderer compose its whole
report into a `String`, which `emit_report` then hands to a [`Report`]. That buys the seam the crate
wanted -- a probe's findings can be asserted rather than eyeballed -- and it gave up a property the
previous line-by-line `println!` had for free: output appearing as it is measured.

`emit_report` recovers it for an **unwinding panic** only, by catching, emitting what was composed, and
resuming. A termination that does not unwind still discards the buffer:

- **Ctrl-C.** The default Windows console handler terminates the process; no unwind runs.
- **Abort from a panic raised while already unwinding.**

The case that costs most is `probe-cancel-io`: four attempts against a five-second watchdog, so about
twenty seconds, and it runs that long *precisely when the wedge it hunts for occurs* -- which is
precisely when a reader gives up and interrupts. The measurement most worth having is the one most
likely to be thrown away.

See [DESIGN-NOTES.md](DESIGN-NOTES.md) -> [The report is buffered, and what that
costs](DESIGN-NOTES.md#d-buffered-report) for why it was built this way and why the fix is a separate
piece of work rather than a correction to that one.

- [ ] **M1.1** -- Decide how a formatted line reaches the sink, because that choice is what makes the
  rest mechanical. Every renderer today writes through `let _ = writeln!(out, ...)` against a
  `String`'s `fmt::Write` -- upwards of 160 sites across the probes -- so the sink must accept
  *formatted* output, not just `&str`, or every site grows a `format!` and an allocation per line.
  The options differ in what they cost callers, and the choice is the engineer's:
  (a) give `Report` a method taking `fmt::Arguments` plus a `report_line!` macro, so a call site stays
  one line and reads almost as it does now;
  (b) implement `fmt::Write` for the sink types, so `writeln!(out, ...)` keeps working verbatim against
  a `&mut dyn Report` -- smallest diff at the call sites, but `fmt::Write` is line-agnostic, so the sink
  must split on newlines internally and `Captured`'s one-line-per-entry guarantee has to be re-established
  rather than assumed;
  (c) leave the renderers writing to a `String` and flush it to the sink at each line boundary, which
  streams without touching the call sites but keeps two buffers.

- [ ] **M1.2** -- Convert every renderer to write into the sink as it measures, and simplify
  `emit_report` accordingly: once lines leave as they are produced, catching the unwind is no longer
  what makes partial output work, and the `catch_unwind`/`resume_unwind` pair should be removed rather
  than left as machinery that no longer earns its place. Keep `Captured` working -- it is what every
  in-process test asserts against.

- [ ] **M1.3** -- Verify by interruption, not by reasoning. Sending Ctrl-C to a probe part-way through
  must leave the already-measured lines on the terminal; today it leaves nothing. Assert the in-process
  half (a renderer that panics mid-report still has its finished lines in a `Captured`) as a unit test,
  and record the Ctrl-C observation in [DESIGN-NOTES.md](DESIGN-NOTES.md) -- an interactive signal is not
  something to assert in CI, but it is the property the milestone exists for, so it must be measured
  once rather than assumed.


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
