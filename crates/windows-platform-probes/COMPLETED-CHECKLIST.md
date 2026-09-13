# Completed checklists: windows-platform-probes

Append-only. Newest groups at the bottom.

## Moved 2026-09-09 19:00:17 -04:00 -- M1: a probe's report streams as it is measured

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

- [x] **M1.1** -- Decide how a formatted line reaches the sink, and build it. **Option (b): `LineSink`,
  an adapter implementing `std::fmt::Write` over a `&mut dyn Report`.** Decision and reasoning in
  [DESIGN-NOTES.md](DESIGN-NOTES.md#d-streaming-report).

  **The estimate in this item was wrong, and re-measuring it decided the question.** It said "upwards
  of 160" `writeln!` sites; there are **332** across this crate's production renderers, written into
  the `&mut String` of 18 functions. Option (a) -- a `Report` method taking `fmt::Arguments` plus a
  macro -- is the most explicit and would have rewritten all 332; that is affordable at 160 and is not
  at 332. Option (b) moves the 18 signatures and leaves the 332 untouched, because `String`
  implements `fmt::Write` too and a call site cannot tell the difference. Option (c) was declined as a
  half-measure that keeps two buffers.

  The cost this item predicted for (b) is real and is now paid: `fmt::Write` is line-agnostic, so
  `LineSink` holds a partial line and emits completed ones, and `Captured`'s one-line-per-entry
  guarantee is re-established by test rather than assumed. Seven tests pin it, including the two
  properties that are easy to get wrong -- a final `write!` with no trailing newline still emits its
  line, and `split('\n')` rather than `lines()` because only the former distinguishes a finished line
  from a partial one. Both were verified by sabotage (failing three tests and two respectively), not
  by reading.

- [x] **M1.2** -- Convert every renderer to write into the sink as it measures, and simplify
  `emit_report` accordingly. All thirteen probes now take `out: &mut dyn std::fmt::Write`; the
  `catch_unwind`/`resume_unwind` pair is deleted, because with lines leaving as they are produced
  there is no buffer to rescue and keeping it would imply partial output still depends on the panic
  unwinding. `Captured` is unchanged and its tests pass untouched.

  **Every probe in this crate needed only the signature change**, because each already went through
  `emit_report` rather than composing a `String` and calling `emit` itself. That is what the
  one-sink refactor bought, and it is why converting thirteen probes is one function plus one line
  per renderer. Three further probes under development on a branch do not hold that property and
  are converted where they land, since they are not in this crate yet.

  **Verified with a control, because several of these probes are not deterministic.** A direct
  before/after comparison flagged four of the thirteen reports, which is not evidence -- they print
  measured nanoseconds and branch their verdicts on them. Running the *same* build twice differs in
  **five**, by the same amount or more in every case: `probe-doorbell-cost` 34 lines against 30,
  `probe-request-cost` 32 against 32, `probe-pool-growth` 14 against 14, `probe-device-map` 4
  against 4, and `probe-cancel-io` 2 against **0** -- that last one being the sharpest, since a
  probe whose output varies run to run happened to match across the change and would have counted
  as evidence of no change had the control not existed. The eight reports the control showed to be
  genuinely deterministic were byte-identical. A before/after diff on a probe means nothing without
  that control.

- [x] **M1.3** -- Verify by interruption, not by reasoning. Both halves done, and the in-process half
  needed a test this item did not describe.

  **The unit test as specified would not have caught a regression.** "A renderer that panics
  mid-report still has its finished lines in a `Captured`" passes under a *buffered* report too --
  emit the buffer after catching the unwind and it holds, which is exactly what the pre-M1.2 code
  did. What distinguishes streaming is not what a reader has at the end but **when** the sink
  receives it, so `a_line_reaches_the_sink_before_the_renderer_returns` observes the sink from
  *inside* the renderer through a shared `Rc<RefCell<..>>`. Restoring the old buffered
  `emit_report_to` fails it with its own message; the panic test alone would have stayed green on
  the mechanism and gone red only on the missing catch.

  **The interruption half is measured, with a control**, and recorded in
  [DESIGN-NOTES.md](DESIGN-NOTES.md). `probe-doorbell-cost` (~0.8 s, the longest-running probe
  here), stdout redirected, killed at 300 ms, six runs of each build with every run confirmed still
  alive at the kill: the streaming build captured **129 characters** (banner and heading) on all
  six, the pre-conversion build **0** on all six. The control is what makes it evidence rather than
  an observation.

  `TerminateProcess` was used rather than Ctrl-C deliberately: it runs no handler at all, where
  Ctrl-C still lets the runtime unwind its exit path, so surviving it subsumes the interactive case.
  The reason any of it works is that Rust's `Stdout` wraps a `LineWriter` and flushes at each
  newline even when redirected -- had stdout been block-buffered this milestone would have needed a
  per-line flush too.

## Moved 2026-09-09 -- M2.6: what `GetFullPathNameW` does, and whether it stays

### <a id="m26"></a>M2.6 -- Say what `GetFullPathNameW` does, in the crate that owns it, and whether it stays. *(completed 2026-09-09 22:54:01 UTC-04:00)*

**Resolved.** The correction and the decision both landed in the owning crate as `D-18` in
[../windows-namespace-request-sys/DESIGN-NOTES.md](../windows-namespace-request-sys/DESIGN-NOTES.md):
`GetFullPathNameW` collapses `.`/`..` lexically but roots most paths that are not fully qualified against
process state, so it is not a lexical call as a whole; `PathCchCanonicalizeEx` does not root, and is
the wrong call for that reason, because rooting at submission is the property being bought. No cost
comparison is claimed -- the item below asked whether the alternative "would be cheaper", and the
answer recorded in D-18 is that nothing measures it, so the decision rests on semantics alone.
The mechanism question this item raised is answered rather than left open: resolving a
drive-relative path for another drive checks that drive's recorded entry against the filesystem
and writes the entry back, so the call does touch the filesystem on that form. The item's body below is the
request as it was written, and quotes the module doc as it read before the correction.

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

  *(Later correction: the second half stood, the first did not. "Touches no filesystem" was measured
  false while carrying out this item -- resolving `X:foo` for a non-current drive distinguishes an
  existing directory from an existing file from a missing one, and rewrites the `=X:` entry when that
  check REJECTS it -- an accepted entry is left alone, so the write is conditional rather than part of
  every such resolution. What
  Microsoft documents is only that the call does not VERIFY its result. Two smaller things in the
  paragraph above also turned out to be stated too broadly: the per-drive entry is consulted for a
  drive OTHER than the current one, and on the current drive it makes no difference to the result --
  and "reads" is a mechanism word that observation cannot reach either way. See
  [../windows-namespace-request-sys/DESIGN-NOTES.md](../windows-namespace-request-sys/DESIGN-NOTES.md)
  -> `D-18`.)*

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

## Moved 2026-09-10 -- M2.1: the report oracle

### <a id="m21"></a>M2.1 -- Add a report oracle to this crate. *(completed 2026-09-10 19:45:50 UTC-04:00)*

- [x] **M2.1** -- Add a report oracle to this crate: one shared executable definition of the
  correlations that must hold between the parts of a rendered report, checked against the rendered
  artifact rather than against internal state. Seed it with the three known invariants: an alarm in
  the prose implies the verdict is not `agree`; a fact rendered in both prose and NDJSON agrees across
  the two; an uncaveated hardware claim implies `!parse_in_doubt`. Model it on
  [../windows-file-watcher/src/contract.rs](../windows-file-watcher/src/contract.rs)'s
  `ContractChecker`, which is this repository's worked example and which existed unused while this
  probe was being written.

  Built as [src/report_oracle.rs](src/report_oracle.rs) with the three seeded
  correlations, plus [tests/a_real_report_agrees_with_itself.rs](tests/a_real_report_agrees_with_itself.rs),
  which runs it over a report rendered from this host and then corrupts eight
  double-rendered facts to prove the oracle is reading them rather than going
  quietly blind. The reasoning is in
  [DESIGN-NOTES.md](DESIGN-NOTES.md) -> "The oracle exists, and what it
  deliberately refuses to know".

  **M2.2 is the next step and is deliberately not part of this**: the oracle
  exists and is applied explicitly, but the crate's existing `report()` call
  sites are not yet bound to it, so it checks the cases these tests name rather
  than every case anyone writes later.

### <a id="m23"></a>M2.3 -- Run `measure()` against the real host, render the report, and apply the oracle. *(completed 2026-09-10 19:45:50 UTC-04:00)*

- [x] **M2.3** -- Add the missing integration test: run `measure()` against the real host, render the
  report, and apply the oracle. At the time of M2 the crate had one integration test, asserting only
  that a probe writes to stdout, and none of the twenty-five `report()` calls rendered from a real
  measurement -- every one used a hand-built `Observation`, which can only contain states its author
  already imagined. On CI this runs across the whole hosted-runner fleet, which is where states no
  fixture anticipates will actually appear.

  Landed as [tests/a_real_report_agrees_with_itself.rs](tests/a_real_report_agrees_with_itself.rs),
  composing the report exactly as `src/bin/topology.rs` does so the artifact
  checked is the one that ships.

  *(A "correction" to the item above was made on the way in and then withdrawn.
  It is recorded because it was this workspace's standing defect committed while
  claiming to fix an instance of it. The item says no `report()` call renders
  from a real measurement and every one uses a hand-built `Observation`. That
  was called false on the grounds that unit tests in `src/tests.rs` call
  `crate::topology::measure()`. They do -- six of them, not the three first
  counted, because the count came from a truncated search -- but calling
  `measure()` is not rendering a report from it, and none of the in-crate
  `report()` call sites is fed by one. The premise is true as written. A
  reviewer counted both numbers and caught it.)*

### <a id="m22"></a>M2.2 -- Route every test that renders a report through the oracle. *(completed 2026-09-10 21:18:25 UTC-04:00)*

- [x] **M2.2** -- Route every test that renders a report through the oracle, so the roughly
  twenty-five existing `report()` call sites inherit the checks and every future one does too. This is
  the step that makes it an oracle rather than three more tests: a test added beside the others checks
  one case, whereas binding the call sites checks every case anyone writes later. Verify the binding by
  sabotage -- change an invariant and confirm existing tests go red -- because a binding that only moves
  when its own test moves is cosmetic.

  Two lines, one in each renderer in [src/topology_report.rs](src/topology_report.rs), under
  `#[cfg(test)]`. That placement is the item's own point: a test added beside the others checks one
  case, where binding the renderer checks every case anyone writes later.

  **The sabotage the item asks for, run in both directions.** Re-introducing a cross-part
  contradiction -- the NDJSON processor count one higher than the prose -- turns **13 existing tests
  red** through the binding. None of them was written about processor counts: they are about cache
  notes, efficiency classes and caveats, and they inherit the check purely by rendering a report.
  With the same contradiction in place and the binding removed, **the whole suite passes**. The
  existing tests cannot see the defect at all, which is the measurement that makes this binding
  load-bearing rather than cosmetic. *(This read "all 190 pass" when the suite was 190 tests. The
  count rotted; the invariant did not.)*

  `cfg(test)` and not otherwise: a real probe run must still PRINT a self-contradicting report
  rather than panic, because the contradiction is a finding about the probe and panicking would
  destroy the evidence. The real-host path is covered by
  [tests/a_real_report_agrees_with_itself.rs](tests/a_real_report_agrees_with_itself.rs), which
  applies the oracle explicitly.

  **What this does NOT reach**, so the boundary is stated rather than left to be discovered: only the
  two topology renderers are bound. The crate's other reports -- `long_path_report`,
  `request_cost`, the doorbell and queue probes -- render through their own functions and are
  unchecked. Whether the oracle's correlations generalise to them is M2.4's question, and the answer
  is not assumed here.
  *(Correction, 2026-09-10 22:20:11 UTC-04:00, appended rather than rewritten because this file is
  history. **Two claims above were wrong when written.** First: "under `#[cfg(test)]`" together with
  "checks every case anyone writes later" cannot both hold, because cargo compiles this library as an
  ordinary dependency -- WITHOUT `cfg(test)` -- for anything under `tests/`. The binding therefore
  reached unit tests only, and an integration test could render a report without inheriting it. Found
  by a review and measured: an integration test rendered a report whose banner architecture
  contradicted its NDJSON `arch` and did not panic. Second, and following from it: the sentence
  "`cfg(test)` and not otherwise" described a gate that no longer exists. The gate is now
  `cfg(any(test, feature = "oracle-in-renderer"))`, with the feature switched on by this crate's
  dev-dependency on itself, and the integration-test hole is closed -- re-running the same
  construction now panics. A DEFAULT-FEATURE build is unchanged and still prints rather than panics,
  verified by inspecting the built artifacts; binaries built under `cargo test` do now assert, which
  is stated in [src/topology_report.rs](src/topology_report.rs) rather than left to be discovered.)*

  *(Further correction, 2026-09-10 23:38:02 UTC-04:00. The commit that made the change above said the
  feature is "off for every non-test build". **That was wrong**: `--all-features` enables it like any
  other feature, so `cargo build --all-features` yields probe binaries that assert -- measured, and it
  matters here because this workspace uses `--all-features` for cargo-mutants runs. Cargo has no
  stable way to declare a feature that `--all-features` skips, so the boundary is now stated in
  [Cargo.toml](Cargo.toml) and [src/topology_report.rs](src/topology_report.rs) rather than claimed
  away. The guarantee that holds is about the DEFAULT build.)*

### <a id="m211"></a>M2.11 -- Compare the `outermost_partitioning_cache` discriminator against the prose conclusion. *(completed 2026-09-10 22:58:44 UTC-04:00)*

- [x] **M2.11** -- Compare the `outermost_partitioning_cache` discriminator against the prose
  conclusion. Gap 2 above, kept as its own item because it is the concrete next step and is worth
  doing whether or not M2.10's derivation lands first. Map every arm to its rendered prose before
  writing the rule, then corrupt each arm in turn against a real report -- a rule written against a
  guessed subset is how this branch shipped a banner check that asserted a correspondence
  `attribution()` explicitly declines to claim.

  **Found by a review corrupting a real report and watching the oracle accept it.** Captured from
  `probe-topology.exe` on an x86_64 host, changing `"outermost_partitioning_cache":"level"` to
  another value leaves the oracle silent while the prose still reads `outermost cache that
  partitions the processors it covers: L2 (8 domains)`. The two are opposite answers to this probe's
  central question -- "a level partitions" against "none does" -- and nothing compares them.

  The oracle reads `outermost_partitioning_cache_level` (the number) and the prose level, so the
  *level* is checked. What is unchecked is the **discriminator**, which is the field that says
  whether a level was selected at all.

  **Not taken in the peel that found it**, and the reason is specific rather than scheduling: the
  discriminator has several arms -- at least `level`, `summary_missing`, and the `NoLevelPartitions`
  case -- each with its own prose rendering, and a rule written against a guessed subset would
  produce false violations on the arms it guessed wrong. That failure mode is not hypothetical here:
  the same peel shipped a banner rule that asserted a correspondence `attribution()` explicitly
  declines to claim, and it had to be narrowed after a review reproduced the false positive. Map
  every arm to its rendered prose first, then write the rule, then corrupt each arm in turn against a
  real report.

  The architecture correspondence found in the same review WAS taken, because `arch` has one
  rendering on each side and no arms to map.

  **Done as the item specified, in the order it specified.** All five arms were mapped from
  [src/topology_report.rs](src/topology_report.rs) before any rule was written -- `level`,
  `none` (NoLevelPartitions), `no_levels_reported`, `not_unique` (NoUniqueOutermost) and
  `summary_missing` -- each keyed to the opening sentence that cannot be confused with another
  arm's. The rule reads which answer the prose announces and compares it to the published
  discriminator.

  **Then each arm was corrupted in turn**, which the item asked for because a rule against a guessed
  subset fires falsely on the arms it guessed wrong. Two table-driven tests walk all five: one pairs
  every arm with its own prose and requires acceptance, the other pairs every arm with a DIFFERENT
  published value and requires a violation, so no arm rests on another's coverage.

  **And against a real report, not only fixtures.** Changing the `Level` arm to publish `"none"`
  in the renderer turns both real-host tests red through `assert_corresponds`; before the rule the
  oracle accepted exactly that report. Restored afterwards.

  One deliberate non-suppression, recorded because it looks like a false positive and is not: the
  `summary_missing` prose opens `BUG IN THIS PROBE`, which the alarm rule reads correctly as an
  alarm beside an agreeing verdict. That is a true correspondence about a different fact, so the
  acceptance test filters to the partitioning fact rather than demanding the report be silent.

### <a id="m210"></a>M2.10 -- Derive the oracle's set of checked facts from the renderer instead of extending it by hand. *(completed 2026-09-11 00:04:57 UTC-04:00)*

- [x] **M2.10** -- Derive the oracle's set of checked facts from the renderer instead of extending it
  by hand.

  **ALL SIX GAPS BELOW ARE NOW CLOSED, AND THE ITEM IS STILL OPEN.** That is the point of it. Every
  one was closed by hand, by adding another rule per fact -- including 4 and 5, which this item
  explicitly said to do as part of the derivation rather than as two more hand-written rules. They
  were done by hand anyway, on a deliberate decision to close the known holes inside the pull request
  that opened them rather than carry them out of it. So the symptoms are gone and the cause is
  untouched: a rule is still added per fact, the SET of facts still drifts, and nothing derives that
  set from [src/topology_report.rs](src/topology_report.rs).

  **Six unread double-renderings were found by six different reviewers, and none by the oracle's
  own coverage.** That ratio is the evidence for the derivation, and closing them by hand does not
  weaken it -- gap 6 was created by the hand-written rule that closed gap 2, and found by the next
  reviewer rather than by the suite.

  The six, in the order they were reported, all now fixed:

  1. `arch` -- banner against NDJSON. **Fixed**, since it has one rendering on each side.
  2. The `outermost_partitioning_cache` DISCRIMINATOR. **Fixed** by M2.11, with all five arms mapped
     from the renderer and each corrupted in turn, against fixtures and against a real report.
  3. `numa_domains_only_in_cpu_sets`. **Fixed**, with an acceptance test for its conditional prose.
  4. **Collection identity**, not just collection values. **Fixed**: the policy names and cache levels
     are compared as sets before their values, so a renamed, missing or extra entry is reported
     instead of silently failing the lookup.
  5. The three diagnostic counts -- `not_compared`, `parse_incomplete`, `enumeration_anomalies`.
     **Fixed**, in both halves: the counts against the prose listings, and the correctness question --
     an `agree` verdict beside a nonzero count contradicts the rule the renderer publishes, which is
     the same shape as the alarm-versus-verdict defect the oracle was built for.

  6. The **level named by the `summary_missing` arm**. **Fixed.** That arm is the only non-`Level`
     arm whose NDJSON level is a number rather than `null`, so it is the only one that can disagree
     with the prose -- and the oracle's other level comparison is keyed to the `Level` arm's prose
     label, so it never fired here. Two numbers rendered side by side, never related.

  **The prediction this item made was borne out within the hour, which is the strongest evidence for
  it yet.** An earlier revision of this paragraph said: "until that exists, gap 6 is found by the next
  reviewer rather than by the suite." The next reviewer then found gap 6 -- in the arm added by
  M2.11, the commit that closed gap 2. Closing a gap by hand added a fact, and the new fact was
  unread by exactly the mechanism this item names.

  So what remains is only the derivation itself, and the test that would make it real: a check that
  every double-rendered fact the renderer emits appears in the oracle's set, failing when a new one is
  added. Six gaps have now been found by six reviewers and none by the suite. Gap 7 will be found the
  same way.

  **How the derivation was actually built.** The item asked for a check that every double-rendered
  fact the renderer emits appears in the oracle's set. It is
  [tests/a_real_report_agrees_with_itself.rs](tests/a_real_report_agrees_with_itself.rs) ->
  `every_fact_the_renderer_publishes_is_accounted_for`, and it takes both halves from places that
  cannot drift:

  - **the SET of facts** is enumerated from the NDJSON line of a report this host really rendered,
    not from a list kept in the test. A field added to the renderer appears here by itself.
  - **read or unread** is MEASURED -- each value is corrupted in turn and the oracle is asked -- not
    restated as a second list of what the rules supposedly look at. A rule that stops reading a fact
    is caught even though no list changed.

  Only the CLASSIFICATION of each key is declared, because that is a judgement a person has to make:
  `COMPARED` (must be read), `CONDITIONAL` (read only when its prose condition holds, with the
  value that means silence), `NEVER_COMPARED` (no second rendering, with the reason). **A key in no
  list fails**, which is what forces the judgement to be made deliberately rather than discovered by
  the seventh reviewer.

  **Measured on this host before the lists were written**, rather than predicted: 15 keys read, 3
  unread -- `reason` (a routing tag the prose never states), `not_compared` (listed only under
  the DISAGREE and INCOMPLETE verdicts) and `numa_domains_only_in_cpu_sets` (prose emitted only
  when the count is above zero). Each exemption's reason was checked against the renderer
  individually.

  **All three failure modes verified by sabotage**, at three distinct assertions:

  | sabotage | result |
  |---|---|
  | add `"smt_groups":7` to the renderer's NDJSON | FAILS -- key in no list |
  | blind `architecture_in_banner` so nothing reads `arch` | FAILS -- listed as compared, not read |
  | publish `numa_domains_only_in_cpu_sets` as 5 while the prose stays silent | FAILS -- conditional key at a non-silent value |

  The middle one is the half that keeps the list honest: `COMPARED` is a claim, and a claim nothing
  measures is exactly what this branch spent ten review rounds finding.

  **What it does not reach**, stated rather than left to be found: the prose side is not enumerated.
  A fact rendered ONLY in prose, with no NDJSON key, is invisible to this test -- the enumeration is
  keyed to the machine-readable line because that is the side with an enumerable structure. A prose
  sentence added with no field beside it remains a thing only a reader would notice.

### <a id="m212"></a>M2.12 -- Validate the oracle and its instruments against a corpus of report SHAPES generated from the renderer. *(completed 2026-09-11 17:32:41 UTC-07:00)*

- [x] **M2.12** -- Validate the oracle and its instruments against a corpus of report SHAPES
  generated from the renderer, not against this developer machine's single shape.

  **This is the dominant cause of defects on this branch, measured rather than asserted.** Classifying
  every finding from eleven review rounds by what would have caught it earlier: seven of the roughly
  eight in the largest group were invisible because the validating host produces exactly one shape --
  measured, `agree`, the `Level` partitioning arm, non-empty caches, classes `[0]`, zero anomalies,
  zero `not_compared`, x86_64. Every one of them hid in the COMPLEMENT of that shape, and each was
  found only because a reviewer imagined a shape by hand:

  - the architecture went uncompared on `report_unmeasured`'s object;
  - `efficiency_classes` compared its contents, so a scalar `1` and a list `[1]` were
    indistinguishable on a host whose single class is `1`;
  - `enumeration_anomalies` is unread under the DISAGREE and INCOMPLETE verdicts;
  - the expected fact name differs on the `SummaryMissing` arm, so the guard failed a legitimate host;
  - an empty container could not be corrupted, so a no-cache host skipped a key silently;
  - the fact-set derivation skipped the unmeasured shape entirely;
  - the `HOST NOT ESTABLISHED` exemption was covered by nothing.

  **Do it as M2.10 did, one level up.** M2.10 derived the set of FACTS from the artifact; this derives
  the set of SHAPES from the renderer's own branches -- the 5 `PartitioningCache` variants and the 3 `Verdict`
  arms, crossed with presence and absence of caches, NUMA domains and efficiency classes. Generate
  each report by calling `report()` with a constructed `Observation`, never by writing report text by
  hand, so the corpus cannot drift from the renderer.

  Run over every shape: `check()`, and BOTH instruments -- the fact-name guard and
  `every_fact_the_renderer_publishes_is_accounted_for`. The instruments are where these defects
  actually lived.

  Feasible with what exists: [src/tests.rs](src/tests.rs) already has `clean_observation()` and
  `agreeing_observation()`, and 21 tests already construct variants, so no new constructor is needed.

  **The limit, so it is not mistaken for total coverage:** a corpus covers shapes the renderer can
  branch into, not the ones real hardware invents. The CI fleet stays the backstop.

  **Built as specified: the shapes come from the renderer's branches.** Fourteen observations, each
  named for the branch it reaches -- the five `PartitioningCache` arms, the three verdicts where
  each is legal, more than one efficiency class, more than one processor group, NUMA domains with and
  without processors, a counter that could not be read, several cache levels, and a topology not
  measured from a running machine. Each is rendered through the real `report()`, so rendering is
  itself the oracle check: `report` asserts on its own output under this build, and a shape that
  contradicts itself panics without needing an assertion of its own. The accounting then runs over
  every shape.

  **Which combinations are LEGAL is derived, not assumed.** `cross_check` pushes a
  `parse_incomplete` entry for an empty cache survey, for a named level with no summary, for a
  cpu-sets-only NUMA domain and for an unmeasured topology -- and a non-empty `parse_incomplete`
  forces the verdict away from `agree`. So `NoLevelsReported` and `SummaryMissing` cannot be
  agreeing shapes, and constructing them as such would build a report the crate cannot produce. The
  corpus reads that from the source rather than guessing it.

  **It found a defect on its first run -- in the corpus.** A fixed banner of `4p` beside a
  128-processor multi-group shape was reported as `BannerDisagreesWithBody`. The oracle was right
  and the fixture was wrong: a real run reads banner and body from the same machine, so a shape that
  varies the processor count has to vary its banner with it. The banner is now derived from the
  observation.

  **And it catches the regression that prompted it.** `enumeration_anomalies` was declared silent
  at zero, and a restructure two commits later silently dropped that back to "always read"; nothing
  caught it because this host reports `agree`, where the key is read anyway. Re-introducing that
  exact regression now fails `every_fact_is_accounted_for_on_every_shape`, on the DISAGREE and
  INCOMPLETE shapes this machine cannot produce. That is the measurement that makes this item worth
  its weight: the defect that survived a restructure, that no test on this host could see, and that
  took a reviewer to find, is now caught mechanically.

  **What it does not reach**, stated rather than implied: the corpus covers shapes the renderer can
  BRANCH into. It does not cover what real hardware invents -- an aarch64 host, a machine with no L3,
  a genuinely flaky enumeration -- and the CI fleet remains the backstop for those. It also does not
  enumerate the prose side, which remains M2.10's stated limit.
  *(Correction, 2026-09-11 17:58:14 UTC-07:00, appended because this file is history. **The corpus
  as first committed did not reach every branch it claimed.** Every shape left `bracket`,
  `coherence` and `enumeration_anomalies` at their agreeing defaults, so three `cross_check`
  branches went unrendered -- including the diagnostic sentence the oracle parses the anomaly COUNT
  out of, which meant no corpus report could exercise that rule at all. Four shapes were added:
  `BracketOutcome::Changed`, `BracketOutcome::NotEstablished`, a `Coherence::Disagreed`, and records
  that failed to decode. Verified load-bearing by blinding `anomaly_count_in_prose`, which now fails
  `every_fact_is_accounted_for_on_every_shape` and did not before. The arm and verdict counts stated
  above were also wrong -- 7 and 6 were match-arm occurrences across several `match` statements, not
  the 5 and 3 variants the enums declare. Found by a review.)*
  *(Correction, 2026-09-12 14:21:50 UTC-04:00, appended because this file is history. **"Fourteen
  observations" describes what M2.12 delivered, and is not the size of the corpus now.** `shapes()`
  pushes nineteen: the fourteen above, the four named in the correction before this one, and a
  fifteenth kind added later -- one shape crossing anomalies with a disagreeing counter, written by
  hand because the corpus varies one dimension at a time and so cannot reach a branch that two
  dimensions select. Counted, not estimated. A reader wanting today's number should count `push(`
  in `shapes()` rather than trust any figure written here; this file records what an item did when
  it was done, which is why the census was the wrong thing to state in the first place -- see
  M2.14. Found by a review.
  Two test names above have since changed for the same reason the count did:
  `every_shape_the_renderer_can_produce_agrees_with_itself` is now
  `every_representative_shape_agrees_with_itself`, and `every_fact_is_accounted_for_on_every_shape`
  is now `every_fact_is_accounted_for_on_every_representative_shape`. Both said "every" while
  `shapes()` is a sample, so a green run read as proof of coverage it does not have. The old names
  are left standing above because this file is history; this line is how a reader following them
  finds where they went.)*

## Moved 2026-09-12 -- M2 completes: the report oracle, its fact set and its shape corpus

M2's own work is done. Its completed items -- M2.1 (the oracle), M2.2 (the renderer binding),
M2.3 (the real-host test), M2.6 (`GetFullPathNameW`), M2.10 (the derived fact set), M2.11 (the
partitioning discriminator) and M2.12 (the shape corpus) -- were each archived above as they landed,
so their stubs in [CHECKLIST.md](CHECKLIST.md) carried nothing this file does not already hold and
were deleted with the milestone.

**The ten open items were re-sequenced, not reworked.** They had accumulated under a heading none of
them fit -- a CI `if:` condition and a doc-comment repair are not correspondence work -- and they
split by whether [DESIGN-NOTES.md](DESIGN-NOTES.md) ->
[#d-encoded-row-is-the-contract](DESIGN-NOTES.md#d-encoded-row-is-the-contract) gates them:

- **M4** (gated on M3): M2.4, M2.5, M2.15, M2.17.
- **M5** (gated on nothing): M2.7, M2.8, M2.9, M2.13, M2.14, M2.16.
- **M2.18 is dissolved** into M3.3 rather than moved. It asked whether a banner should be a type;
  M3.3 answers the general form of that question, and answering the banner alone would have typed
  one parameter while leaving the shape everywhere else.

**Their IDs deliberately keep the `M2.` prefix.** This file is append-only and its entries are
immutable, and two entries above already cite M2.4 and M2.14 -- so renumbering would leave dangling
references here that may not be edited to repair them. A stable ID costs a mismatch between an item
number and its milestone heading; renumbering would cost correctness in the archive.

*(Recorded 2026-09-12 18:45:27 -04:00. This entry closes a milestone rather than completing an item,
so it carries no `###` item heading and nothing links to it by anchor.)*

## Moved 2026-09-12 -- M3: the encoded row became the contract, and the prose stopped being checked


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
- **The per-item re-scoping notes for M2.4, M2.5, M2.15 and M2.17 now live in M4's preamble**, not
  here. They were written during this milestone but they instruct work that is still open, and a
  pending instruction does not belong in an append-only archive nobody may edit to correct it.
- **M2.7, M2.8, M2.9, M2.13, M2.14 and M2.16 are gated by nothing** and are in M5. M2.9 (a
  cross-host ratio called "the finding") and M2.14 (making two authoring rules bite) are if anything
  reinforced:
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

- [x] **M3.2** -- Assert the surviving correspondences as invariants on the observation, before
  rendering.

  Alarm-against-verdict, diagnostics-against-verdict and counters-against-verdict are the three
  oracle rules that survive the decision. They stop being comparisons of two rendered texts and
  become predicates over `Observation` and `CrossCheck` -- `SummaryMissing` implies the verdict is not
  `agree`, a non-empty `parse_incomplete` implies the verdict is not `agree`, and so on. No parser is
  involved, and the check runs whether or not anything was rendered.

  Each one must be sabotage-verified on arrival: delete the invariant, confirm the suite reddens,
  restore it. A predicate that cannot fail is the failure mode this crate keeps meeting.

  **Done, and the item's own framing was wrong in a way worth recording.** It named
  "diagnostics-against-verdict" and "counters-against-verdict" as rules to move. Two of those read
  `CrossCheck`'s lists -- and `verdict` is a pure function of those lists, so such a rule restates
  the definition, cannot fail for any input, and CANNOT CATCH A DELETED PUSH SITE: the deletion
  empties the list, the rule sees nothing, and the verdict is `agree` legitimately. Written that
  way first, with three tests that asserted acceptance under violation-sounding names.

  Every rule now reads the OBSERVATION. `blocking_states` names ten states that forbid an agreeing
  verdict, each with a push site in `cross_check` that it does not consult, plus the two counter
  rules. `check` takes the verdict rather than deriving it, so a test can supply the answer a
  broken `cross_check` would give -- otherwise every branch is reachable only by editing the source
  and a green run says nothing.

  Bound at `observe` (every observation MEASURED, rendered or not -- what this item asked for) and
  at `report` (every observation RENDERED, which on the test side is most of them, since the suite
  builds observations by hand). Not in `cross_check`, which would recurse.

  Sabotage: deleting the `PartitioningSummaryMissing` push reddens four tests, two of them new --
  the invariant's own accounting test, and a render test through `assert_holds` at the renderer
  binding. The invariant is not the sole detector for that push site; its value is the nine others,
  several of which have no dedicated test.

- [x] **M3.3** -- Emit the row from a typed value through one writer.

  > **-> PREREQUISITE: M3.4 lands first.** The reason is on M3.4: this item's nested per-entry data
  > makes `ndjson_list_len` silently miscount, so the parsers it would break should be gone before
  > the row changes shape rather than taught a shape they are about to lose.

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

  **Done.** `crate::row` holds a `Value` and a `Row` whose members are name-and-value pairs, with
  one writer that escapes strings. Both defect classes are now unrepresentable rather than
  detected: a name and its value move together or not at all, and a `Value::Text` cannot end the
  string it is in.

  Each diagnostic publishes its DATA through `published()`, so a survey learns
  `{"code":"contradictory_cores","count":3}` rather than the code alone -- what M3.1 had to leave
  behind because the row was a positional template. Anomalies carry `source` and `offset` too:
  the same kind at the same offset across a fleet is a different finding from the same kind
  scattered, and neither is visible from a count.

  `report_unmeasured` goes through the same writer, and that is the shape that most needed it --
  it is the only renderer that interpolates caller text, a failed discovery's `io::Error`. The
  error now reaches the row as a `discovery_error` field, so a survey can group failures by cause
  instead of parsing the prose sentence.

  The key-set check M3.4 deferred here now exists -- but NOT in the form M3.4 predicted, and the
  first attempt at it was vacuous. See the correction recorded under M3.7.

  **Two silent behaviour changes were caught by checking the old code rather than trusting the
  rewrite.** `PartitioningCache` has FIVE variants, not the four a rewrite naturally reaches for;
  and `SummaryMissing` publishes its level rather than `null` -- which matters precisely because
  that arm is the report telling a reader the probe has a bug, and WHICH level went unchecked is
  what they need.

  Sabotage-verified: removing the quote escape reddens three row tests, including the
  brace-injection one. The clean row is byte-identical to what the template produced, confirmed
  against a real `probe-topology` run.

- [x] **M3.4** -- Retire the prose-against-row correspondences and the parsers that serve only them.

  > **-> DO THIS BEFORE M3.3, and leave both IDs where they are.** M3.3 carries each diagnostic's
  > data, which turns the flat code arrays into arrays of OBJECTS -- and `ndjson_list_len` splits on
  > `,`, documented as safe for flat code arrays and nothing else. Pointed at
  > `[{"code":"contradictory_cores","cores":3}]` it counts members rather than entries and returns 2
  > for one entry. It does not fail; it silently answers wrong, and every prose-comparison rule then
  > compares that against the prose. Running M3.3 first therefore means teaching parsers a nested
  > shape and deleting them one item later, with a silent-wrong-answer window in between. The IDs
  > stay put because renumbering costs more than the mismatch, the same trade as M4/M5.
  >
  > Intended order for the rest of M3: **M3.2 -> M3.4 -> M3.3 -> M3.5**.

  > **-> CODE REVIEW RESUMES HERE.** Reviews are paused by the engineer's decision of 2026-09-12
  > until this crate no longer depends on prose as the oracle's subject, and this is the item that
  > ends that dependence. The reasoning: a large share of PR #88's fifteen fix commits were defects
  > in the prose-reading machinery -- the multibyte panic in `processors_in_banner`, `trim_matches`
  > collapsing `[[0]]` and `[0]`, `prose_field` selecting the wrong line -- and every one of them is
  > code this item deletes. Reviewing it closely is polishing something already scheduled for
  > demolition.
  >
  > Recorded with the honest counterweight, so the decision can be re-judged on evidence rather than
  > re-argued: of the six findings across the two reviews run on 2026-09-12, none was a defect in
  > the prose oracle. Two were documentation drift, one was a test that had lost its
  > discrimination, and the most valuable -- `disagreements` reaching the prose and not the row at
  > all -- was about the ROW being incomplete and survives this item untouched.

  Of 38 top-level functions in [src/report_oracle.rs](src/report_oracle.rs), ten are correspondence
  rules, four are comparison helpers, and **twenty-three exist only to extract values back out of
  rendered text**. With M3.2 and M3.3 landed, that extraction layer has no remaining consumer.

  What stays is a thin check that the row is **well-formed** -- it parses, it carries the expected
  key set, and it is the only such line in the report. That is not a correspondence; it is the
  writer's own output being checked, and the writer is the one place structure cannot check itself.

  Retire, do not merely stop calling. Dead extraction helpers left in place are a second grammar for
  a format that no longer has two readers.

  **Done, together with M3.5, because they cannot be separated.** The fact-accounting instrument is
  built entirely on `report_oracle::check` and the `Correspondence` variants, so deleting the
  correspondences leaves it measuring nothing and the suite red between the two items. Committed as
  one commit citing both IDs, per the checklist rule for coupled items, rather than split into a
  commit that does not pass.

  Measured: `report_oracle.rs` 79,394 -> 8,874 bytes, its tests 105,299 -> 5,598, the integration
  instrument 71,007 -> 25,689. All eight prose correspondences and all twenty-three extraction
  helpers are gone.

  What survives is the row's well-formedness: exactly one machine-readable line, brackets balanced
  (string-aware, because a failed discovery's `io::Error` is interpolated into a string value and an
  OS message is free to contain a bracket), and no repeated top-level key. That last one is the
  malformation that survives a consumer's parse and changes what it reads, since most JSON readers
  take the last.

  (The bracket check was weaker than this sentence implies -- it counted depth, so a trailing or
  misplaced separator passed. Strengthened in M3.7.)

  **The key-set check is deliberately NOT here.** Asserting it needs a list of expected keys, and a
  list written here is a census -- this component re-corrected the same census three times in one
  day. M3.3 makes the row a typed value, at which point the key set is derivable from the type
  rather than declared beside it. Moved there rather than approximated here.

  (**The second sentence is wrong, and M3.7 corrects it.** A key set is NOT derivable from a typed
  row: the type says "a row is a map of names to values", which is satisfied by every key set,
  including the one missing a field. The census this note was right to fear is a count; a schema is
  not one, and refusing to write it down bought nothing.)

- [x] **M3.5** -- Re-aim the shape corpus and the fact accounting at the row.

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

  **Done, with M3.4, and the second enumeration exists.** The instrument no longer asks "which prose
  facts does the oracle read" -- there are none. It asks, for every state `topology::invariant` knows
  forbids agreement, whether the row publishes a condition for it; and it holds the row's published
  conditions against what the cross-check found, across the corpus.

  (As first written this said the second rule held the row against a count of PROSE lines, which it
  did at the time. The follow-up commit that removed the last prose parsing replaced that with the
  comparison against the cross-check -- recorded further down this same item, so the item disagreed
  with itself. Found by a review.)

  Sabotage-verified against the defect that motivated it: dropping `disagreements` from the row --
  the omission that survived 41 review rounds, a zero-survivor mutation sweep and the old accounting
  -- now reddens the rule that holds the row against the cross-check. That rule was named
  `the_row_lists_a_condition_for_every_diagnostic_the_prose_lists` when this evidence was recorded
  and is `the_row_lists_exactly_the_conditions_the_cross_check_found` now; the sabotage was re-run
  against the current name. Recorded evidence that cannot be re-run as written is evidence nobody
  will re-run.

  **One asymmetry, found by the instrument rather than reasoned.** Counting all four lists against
  prose lines failed: the prose folds every anomaly into ONE
  `windows-topology-sys recorded N enumeration anomal...` sentence while the row lists one code per
  anomaly, so three anomalies read as two dropped entries. `enumeration_anomalies` counts on its own
  axis and is checked against the OBSERVATION -- one published code per anomaly recorded -- which is
  the artifact the row owes fidelity to. Checking it against the number inside that sentence would
  be the prose-reading this milestone retired.

  Both publication rules carry a corpus guard, because both skip a shape in no blocking state and a
  drifted all-healthy corpus would leave them green while checking nothing.

  **The last prose parsing in the matrix is gone.** M3.5 left one site: a rule that filtered
  rendered lines by prefix, counted them, and compared that number against the row -- the only place
  left where the test matrix obtained structured data by reading sentences. It had a unit-test twin
  in `src/tests.rs` that the first sweep missed and a second, wider sweep found.

  Both are replaced by the same claim against `cross_check`: the row's codes must EQUAL the
  cross-check's, in order. Strictly stronger -- a count catches only a dropped entry, this catches a
  drop, a reorder and a substitution -- and it never reads a sentence. It also covers all three
  lists, which the prose count could not: under INCOMPLETE the renderer gives `not_compared` and
  `parse_incomplete` the same bare `- ` prefix, so only their total was recoverable from prose.

  **The ordering half was vacuous, and the guard is what found it.** Reversing the row's
  `parse_incomplete` order reddened nothing: every corpus shape varied one dimension, so each landed
  at most one entry per list, and a one-element list has no order to get wrong. A first version of
  the guard summed the three lists and passed while the sabotage still did nothing -- one entry in
  each of two lists is two conditions and no order. Corrected to measure the largest SINGLE list,
  and a `several conditions at once, in one list` shape added. The reorder now reddens.

  What remains that touches rendered text at all: selecting the row line, and asserting positional
  containment -- the banner is the first line, the banner is one line, there is exactly one row.
  None reads prose for its content.


  [tests/a_real_report_agrees_with_itself.rs](tests/a_real_report_agrees_with_itself.rs) enumerates
  the facts a report publishes and measures, by mutation, which are read. The instrument is sound and
  the target changes: enumerate the row's fields, and require each to be read by an invariant or
  explicitly classified as unread. Its corpus of shapes keeps its purpose -- it exists to defeat the
  imagination-driven fixture, which the decision does not change.

  The prose half becomes a rendering test: the renderer emits what it is supposed to emit, judged on
  its own terms rather than against the row.

- [x] **M3.6** -- Split [DESIGN-NOTES.md](DESIGN-NOTES.md) into Tier 1 and Tier 2.

  Measured: 88 KiB, which is **XL** on the repository's byte scale, and the default posture at XL is
  to split unless the module is indivisible. It is not -- it carries current decisions and a large
  volume of how-we-got-here reasoning, which is exactly the Tier 1 / Tier 2 fracture the repository
  instructions describe.

  Move the rationale to `DESIGN-RATIONALE.md`, cross-referenced by decision anchor, leaving Tier 1
  stating what was decided and what forced it. The decision added by this milestone is written to be
  split that way already, so it is the worked example rather than the hard case.

  **Done, and the result is still XL -- say so rather than imply otherwise.** 25,942 bytes moved;
  DESIGN-NOTES.md went 92,467 -> 67,807, which is over the 64 KiB threshold still. The split was
  made at the one unambiguous Tier 2 fracture rather than trimmed to hit a number.

  The fracture: the correspondence-oracle investigation. It is Tier 2 on both tests -- a record of
  how a decision was reached rather than a statement of one, AND a decision since superseded by
  [#d-encoded-row-is-the-contract](DESIGN-NOTES.md#d-encoded-row-is-the-contract). Moving it also
  resolved latent drift: it cites `Correspondence`, the fact-accounting instrument and the prose
  rules, none of which survived M3.4 and M3.5. As history those sentences are accurate; as Tier 1
  they described deleted code.

  Pure relocation, verified byte-for-byte against the pre-split file (439 lines, identical). The
  two moved anchors are kept in Tier 1 beside a pointer, so existing links land somewhere that says
  where the content went, and every in-repo reference was repointed at the content.

> **-> OPEN QUESTION for the engineer:** the remaining bulk of DESIGN-NOTES.md is neither current
> decisions nor rationale -- it is FINDINGS, measurements about Windows that are this crate's actual
> product (the completion-port fork, the thread-agnosticism probe, the x64 comparison, the long-path
> pair, the topology cross-check). They do not belong in a rationale file, and filing them as
> decisions is what keeps Tier 1 XL. Whether they want a tier of their own is a structural choice
> about this component's documentation scheme, so it is raised rather than taken.

- [x] **M3.7** -- Make four instruments as strong as their names claim.

  A review of the completed M3 found no wrong behaviour and four weak instruments -- tests and
  guards whose names assert a property they could not actually fail to satisfy. That is the
  recurring defect class of this whole branch, so the four are recorded with what each one was
  measured to miss.

  **1. The row's key set was unenforced.** M3.3's note above claimed the check "is derived:
  `Row::keys` reads the value, and a test asserts the reader and the writer agree". Both halves
  read the same `Row`, so the test says only that the writer is self-consistent. Measured: deleting
  `.with("packages", ...)` from the renderer left the ENTIRE suite green -- a field silently
  vanishes from every downstream survey and nothing objects. Fixed by declaring
  `MEASURED_ROW_KEYS` / `UNMEASURED_ROW_KEYS` as the contract the renderer is held to. This is not
  the census M3.4 feared: a count is derivable from the thing it counts, so restating it is drift
  waiting to happen; a schema is NOT derivable from the row, which is exactly why writing it down
  buys something.

  **2. The well-formedness oracle accepted invalid JSON.** `balanced()` counted bracket depth, so
  `{"a":1,}` (trailing separator) and `{"a":1]` (mismatched closer) both passed -- and a consumer
  would reject both. Replaced by `malformation()`, a typed delimiter stack that also checks
  separator placement. Sabotage-verified: making the writer emit a leading separator produces a
  balanced but invalid row, which the old check passed and the new one reddens.

  (**That replacement was itself replaced, by M3.9.** The typed delimiter stack was a second
  hand-written opinion about what JSON is, and a generated test found 159 more rows it accepted
  and a real parser rejected.)

  **3. A sabotage asserted only that it had sabotaged.** The publication-accounting sabotage stripped
  a condition from the report and then asserted the condition was absent -- which is a fact about
  the string edit, not about the rule. It would have passed with the rule deleted. The rule is now
  `publication_holds(observation, text)`, and the sabotage asserts it REJECTS the stripped report
  and ACCEPTS the original.

  **4. A completeness guard compared a table against itself.** `blocking_states` returned strings,
  and the guard that checked every blocking state was described derived both sides from that one
  table. Introduced a `BlockingState` enum with `ALL`, so the guard holds the table against the
  type's variants and a new state that nobody describes fails to build past it.

  (**The last clause was false, and M3.8 corrects it.** `ALL` was a hand-written array; nothing
  tied it to the enum.)

- [x] **M3.8** -- Make `BlockingState::ALL` exhaustive by construction rather than by assertion.

  The same defect as M3.7's fourth finding, one level up, and introduced by the fix for it. The
  doc on `ALL` claimed "an exhaustive list the compiler checks: adding a variant without adding it
  there fails to build". That is not what the compiler checks. The `match` in `described()` is
  exhaustive-checked, which is what made the claim look right -- but it forces a new variant to
  acquire an ARM, never an ENTRY in a separate array.

  Measured, not read: a new variant plus the `described()` arm the match demands compiled cleanly
  and left all ten invariant tests green, reached by none of them. `ALL` is the list the
  completeness guard iterates, so a variant missing from it is a blocking state nothing tests --
  which is the exact failure the guard was added to prevent, reintroduced by the shape of its fix.

  The reverse loop in the guard is not a substitute. It catches a state `blocking_states` produces
  and `ALL` omits, but only once some perturbation reaches it -- and a state with no perturbation
  entry is precisely what the test exists to catch, so it is circular in the case that matters.

  Fixed by declaring the enum, `ALL` and `described()` from one list through a macro, so a variant
  that is not in the list does not exist. The claim is now true rather than deleted.

  Sabotage-verified in both directions: the original sabotage is now inexpressible (there is no
  second place to omit the variant from), and its reachable equivalent -- a new state in the list
  with no perturbation entry -- reddens `every_blocking_state_has_a_perturbation`, where before
  the whole suite stayed green.

  **Three rounds on one guard: strings, then a hand-written `ALL`, then generation.** Each fix
  moved the census somewhere harder to see rather than removing it. Worth stating because the
  reviewer's finding was not a new defect -- it was the same defect wearing the previous fix.

- [x] **M3.9** -- Decide the row's well-formedness by a real parse, and delete the hand-written one.

  **The question the oracle asks is "could a consumer read this row", and a consumer uses a JSON
  parser.** Anything hand-written here is a second opinion about what JSON is, and a second opinion
  is a thing that can disagree -- so `malformation` now calls `serde_json` and the scanner is gone.

  **Measured, and the measurement is why this happened at all.** The hand-written check had already
  been through a review, which strengthened it after finding it accepted `{"a":1,}`. A generated
  test -- 1807 single-character corruptions of a real row, judged against `serde_json` -- then found
  **159 more disagreements, every single one a FALSE ACCEPT**: 129 stray backslashes forming invalid
  escapes, 10 missing `:`, 13 `,` where a `:` belonged, 3 the reverse, 3 missing values, 1 string
  following a number. The review had found one instance of a class with 160 members.

  Closing the last ~26 required tracking whether an object expects a name or a value next, which is
  a JSON parser. So the choice was to write one or to depend on one.

  **The agreement test was deleted in the same commit, deliberately.** With the parse delegated it
  would compare `serde_json` against `serde_json` -- green by construction, and exactly the
  tautology this milestone keeps deleting. What replaced it asks a question that is still open: not
  "is the verdict right" but "does the verdict REACH the caller", which is a property of `check` and
  not guaranteed by any parser. It found a real boundary while being written: 8 corruptions destroy
  the leading brace, and those are `Missing` rather than `Malformed` -- not a row at all, which for
  a survey asking "did this host report a row" is the right answer and a different one. Both
  branches are asserted.

  **Three tests stopped asserting the defect's wording.** The message is `serde_json`'s now, so
  this crate does not own it; pinning it would let a dependency's patch release redden tests about
  unclosed delimiters, a false finding about this crate. They assert rejection and the carried row.

  **A parse does NOT subsume `RepeatedKey`,** which is why that check stays hand-written:
  `serde_json` accepts a duplicated key and silently keeps the last, which is precisely the
  malformation that survives a consumer's parse and changes what it reads.

  `report_oracle` is now gated `cfg(any(test, feature = "oracle-in-renderer"))` -- every caller
  already was -- which is what keeps the parser out of a shipping probe. Verified by inspecting the
  binaries, per the precedent in that feature's own comment: the default `probe-topology.exe`
  contains no `serde_json`, no oracle panic string, and no parser message; the `--all-features` one
  contains all three.

- [x] **M3.10** -- Delete the last hand-written string scanners in the oracle.

  M3.9 removed one of six; this removes the rest. **The module hand-writes no string walking at
  all now** -- every byte-level decision about quotes, escapes and delimiters comes from
  `serde_json`.

  **Two of the four were provably unsafe, on an argument enforced by nothing.** `list_codes` found
  `"code":"` and took the next `"` as the end, and `list_span_end` counted brackets with no notion
  of being inside a string. Both were safe only because every code is a `&'static str` from an enum
  and no caller text reaches a diagnostic list -- true, load-bearing, and guarded by no test.
  `keys` had already proved the class reachable: it made `assert_corresponds` panic from inside
  `report_unmeasured` on a quoted `discovery_error`. Parsing makes the argument unnecessary rather
  than merely correct, which is the difference between a property and a hope.

  **`keys` needed a visitor rather than a parsed map, and the reason is a contract.** It must return
  the row's names in ORDER and WITH DUPLICATES. `serde_json::Map` sorts, and silently keeps the last
  of a repeated key -- which would delete the evidence for `RowDefect::RepeatedKey`, the one
  malformation that survives a consumer's parse. A `MapAccess` visitor reads each name as the parser
  reads it, so both properties survive while every scanning decision stays `serde_json`'s. That
  reasoning is now a sabotage entry rather than a comment: replacing the visitor with the obvious
  `Map` one-liner is `caught`.

  **`list_span_end` was deleted, not moved.** Its only caller was a sabotage doing text surgery on a
  list. That sabotage now parses, empties the lists and re-renders -- its third implementation, after
  one that split on commas (which sliced entries in half once they became objects) and one that used
  this helper. A sabotage that hand-parses is a sabotage that can quietly stop sabotaging, and it
  leaves the rule it guards unguarded while still passing.

  **The stale sabotage entry is itself the evidence.** After the change the sweep reported
  `MANIFEST STALE: pattern found 0 times` for the escape-awareness entry -- the defect it injected
  can no longer be expressed, because the code that could hold it is gone. Replaced with the
  parsed-map entry above; 7 of 7 behave as declared.

  Default build re-verified by binary inspection: neither `serde` nor `serde_json` appears on a
  normal dependency edge, and `probe-topology.exe` contains no parser string.
