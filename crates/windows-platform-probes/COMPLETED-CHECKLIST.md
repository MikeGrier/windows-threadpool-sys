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