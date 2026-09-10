# Completed checklists: windows-platform-probes

Append-only. Newest groups at the bottom.

## Moved 2026-09-05 -- claim-word layout: measured the apportionment, then shipped it as a caller's choice

# Checklist: claim-word layout

Measures how the `reserving_mpsc` claim word's bit apportionment and width
affect push throughput, then offers the layouts as documented, caller-selectable
options in `windows-waitable-queues`.

Design decisions land in [DESIGN-NOTES.md](DESIGN-NOTES.md) for the measurement
and in the queue crate's
[DESIGN-NOTES.md](../windows-waitable-queues/DESIGN-NOTES.md) for the API
(`D-36`, `D-37`).

## Background

`reserving_mpsc` packs `reserved` and `position` into one `AtomicU64` because
the claim protocol needs a single compare-and-swap to update both (`D-17`,
`D-34`). The split is 32/32, which caps positions at 2^32 and is the whole
source of the `SH-14.1` recurrence hazard disclosed by `D-36`.

**The 32/32 split is not forced by the platform.** It follows from a capacity
ceiling of 2^31, because the `reserved` half must be able to hold the entire
capacity. Two independent constraints bound the capacity:

- ring arithmetic: `capacity <= 2^(POSITION_BITS - 1)`
- packing: `capacity <= 2^(64 - POSITION_BITS) - 1`

`BOUNDS_MAX` is currently derived from the first alone and the second is only
*asserted*, so widening the position raises the ceiling while shrinking the
field obliged to hold it -- which is why widening trips the assertion instead of
working.

## M1: measure the layouts -- done

Built as a duplicated path in this crate so the measurement added no third-party
dependency to a publishable crate and could not disturb the
`windows-waitable-queues` branch being peeled off PR #56.

- [x] **CW-1.1** -- Add `portable-atomic` with `default-features = false` to
  this crate only, and record whether `AtomicU128` exists and is lock-free.
  Measured: `is_always_lock_free()` is true and `cmpxchg16b` is a default target
  feature here, so no CPUID branch was timed as though it were the algorithm.

- [x] **CW-1.2** -- Implement the three claim-word layouts as self-contained
  `u64`-item queues in [claim_layout.rs](src/claim_layout.rs).

- [x] **CW-1.3** -- Wire the three layouts into `probe-queue-contention` as
  named shapes in both regimes.

- [x] **CW-1.4** -- Run the probe and capture the report. Result:
  re-apportioning is free (16/48 tracks 32/32 within noise in both regimes);
  widening to `u128` costs 2-3x isolated and 5-12% drained, and the drained
  figure understates it because a slower producer earns fewer refusals.

- [x] **CW-1.5** -- Record the measurement and the rollover table in
  [DESIGN-NOTES.md](DESIGN-NOTES.md).

## M2: offer the layouts as options

**Decided: offer a set of named layouts rather than one, on the condition that
each carries its own ramifications.** The engineer's direction was that options
are right "as long as quality is maintained" and "the ramifications of the
choices are available". Both halves are binding, and the second is the one an
options API usually fails: a caller who cannot see what a layout costs will pick
by name, and the names are the least informative thing about them.

Three obligations apply to every item in this milestone:

- **Each layout states its own consequences where it is named** -- reservation
  ceiling, capacity ceiling, and time-to-recurrence at a stated push rate. The
  rollover figures in [DESIGN-NOTES.md](DESIGN-NOTES.md) are the source; the
  crate documentation restates them once and nothing else does.
- **Quality is per-layout, not per-crate.** Every layout gets the same const
  assertions, tests, and mutation coverage as the shipping one. A layout
  exercised only by a doctest is worse than no option, because its presence
  claims a support level nothing verifies.
- **Adding a layout must not weaken the default.** The layout parameter must
  not leak into the signatures of callers who do not use it. If it cannot be
  kept out, say so rather than accepting the churn.

**This milestone is no longer parked.** It was gated on the peel merging, on the
reasoning that touching `windows-waitable-queues` would re-grow a branch under
review. That reasoning expired: `mikegrier/waitable-queues` has no pull request
open, so there is no review to disturb, and the `u64` layouts need no new
dependency at all -- only 64/64 does, which is `CW-2.3`.

- [x] **CW-2.1** -- Introduce the layout as a compile-time parameter, widen the
  position to 64 bits, and decouple the reservation ceiling from the capacity.

  **Merged from two items during execution, because they cannot be verified
  apart.** Decoupling the ceiling is numerically invisible at 32/32:
  `MAX_RESERVED` is 2^32-1 while `BOUNDS_MAX` is 2^31, so a cap on outstanding
  reservations can never bind and no test can reach it. It becomes observable
  only once a layout makes the reservation half narrow. Landing them separately
  would have meant committing a branch nothing could exercise and calling it
  done.

  The three parts:

  - Cap *outstanding reservations* at `MAX_RESERVED` in `reserve`, and drop the
    `BOUNDS_MAX <= MAX_RESERVED` const assertion that ties the capacity to the
    reservation field. `BOUNDS_MAX` then follows from ring arithmetic and the
    crate-wide bound alone.
  - Widen `position`, `head`, and the per-slot `sequence` to 64 bits for every
    layout, since a position of more than 32 bits cannot be read out through
    `position_of`'s `u32`. Uniform 64-bit metadata is measured-safe rather than
    assumed: `CW-1.4` compared 32/32 with 32-bit metadata against 16/48 with
    64-bit metadata and found no difference, and for a `u64` payload the slot is
    16 bytes either way once alignment is applied.
  - Add the layout parameter with a default preserving today's behaviour.
    Generic defaults are permitted on types but not on functions, so `bounded`
    keeps its signature and returns the defaulted types, and a second entry
    point names a layout explicitly.

  **This is a contract change**: the shipping shape promises every slot may be
  reserved at once, and this replaces that with a fixed reservation ceiling. A
  different promise rather than a broken one, but it must be stated, not slipped
  in.

- [x] **CW-2.3** -- Decide whether a 128-bit claim word ships at all.

  **Decided: yes, behind an opt-in `dwcas` feature.** The `Wide` layout packs a
  `u128` divided 64 / 64. Without the feature the crate depends on
  `windows-sys` alone and every layout uses `AtomicU64`; with it,
  `portable-atomic` appears. So a caller who does not want the dependency does
  not carry it, and one who wants a guarantee rather than a twenty-year
  argument can have it.

  This resolves `CW-1.6`'s scope the other way from what the item anticipated:
  the shipping crate *can* now express a 128-bit layout, so the probe does not
  need to keep its own `wide` implementation to measure one.

  **Not a dependency question.** An earlier form of this item framed it as
  whether `portable-atomic` becomes a dependency of a published crate, which was
  wrong: `core::arch::x86_64::cmpxchg16b` is stable on the pinned toolchain, so
  a 64/64 layout needs no third-party crate. `D-7`'s and `D-37`'s dependency
  cost does not apply, and the decision must not be made on it.

  What it actually costs: hand-written `unsafe` with manual orderings in the
  file where that is worst to get wrong, x86-64 only (no ARM64 `casp`, no
  i686), and a `target-feature` or runtime-detection decision. Against that,
  `CW-1.4` measured the 128-bit exchange 2-3x slower on the claim in the
  isolated regime, and `CW-2.1` has since made `Perpetual` reach about 20 years
  before recurrence on a plain `AtomicU64` at no measured cost.

  So the question is narrow: is going from unreachable-in-any-deployment to
  unreachable-in-principle worth that? The engineer has said 32-bit Windows
  deployment is not a present concern, which changes `D-18`'s premise and must
  be recorded rather than assumed.

  **`CW-1.6`'s scope is decided by this item**: if `portable-atomic` is
  declined, this crate must keep its `wide` implementation, because a layout the
  queue crate cannot express is one the probe cannot instantiate.

- [x] **CW-2.4** -- Document the layouts as a choice, in the crate documentation
  and the README, with the rollover table and the two axes a caller trades
  between: outstanding reservations against time-to-recurrence. Lead with what
  `CW-1.4` measured -- re-apportioning is free, widening is not -- so a caller
  is not left assuming the safest option must be the slowest. State the push
  rate the figures assume, and that a draining queue cannot sustain the fastest
  of them.

  **Compiled, not merely written.** Any README example naming a layout is a
  doctest per this repository's CONTRACT INTEGRITY rule, so a renamed or removed
  layout breaks the build instead of leaving the documentation teaching a name
  that no longer exists.

- [x] **CW-2.5** -- Reopen `D-36` with the measurement in hand, then sweep every
  statement of the hazard.

  **`D-36`'s premise is falsified, and that is the finding, not the sweep.** It
  decided 0.1.0 ships `SH-14.1` disclosed rather than fixed *because the fix is
  a claim-protocol replacement (`D-35`) gated on an open question*. Re-
  apportionment is a second fix that neither `D-36` nor `D-37` considered, and
  `CW-1.4` measured it free. It does not eliminate the recurrence -- only moves
  it -- but 8/56 moves it from about 37 seconds to about 20 years at the
  disclosed rate, which takes `D-36`'s "computed exposure" from reachable in
  under a minute to unreachable in any real deployment.

  So the question is whether the crate ships this hazard at all. Answer that
  first; the sweep follows from the answer.

  The sweep is blast-radius, not an edit of one reported site: `D-36` states the
  hazard in the crate documentation, the README, and `reserving_mpsc`'s module
  documentation, each leading with "on every target, not only 32-bit ones", and
  `lib.rs` separately claims the shape is "sound below the wrap". Every one is
  scoped to a 32-bit position. Grep the distinguishing terms across `src/`,
  `tests/`, `examples/` and `*.md` for the crate and its dependents, fix every
  hit or say why it is out of scope, and record the sweep in the commit message.

## M3: retire the duplicate

- [x] **CW-1.6** -- Delete the duplicated *implementation* in
  [claim_layout.rs](src/claim_layout.rs), keeping only what `CW-2.3` leaves no
  other way to measure.

  **This is not a decision about which layouts to offer.** That is settled --
  multiple layouts ship as caller-selectable options, per `M2`. This item is
  only about the private copy of the reserving protocol in this crate, which
  existed so the layouts could be measured without touching
  `windows-waitable-queues`.

  **`M2` makes the copy obsolete.** Once the shipping crate takes the layout as
  a compile-time parameter, the probe instantiates the *real* type at any layout
  it wants to compare, including candidates that are not defaults -- so
  exploring a new apportionment no longer needs a duplicate.

  Deleting it is not tidiness. A second implementation of the same protocol
  drifts, and this one already did: `CW-1.4`'s first run measured 3.7x against
  the shipping shape on an entirely different scaling curve, because the
  duplicate had not cache-padded `head` and the claim word. Corrected, it still
  sits about 1.26x off. A duplicate that diverges silently produces a
  measurement that looks healthy and describes something nobody ships.

## Moved 2026-09-09 -- M1: a probe's report streams as it is measured

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
  of 160" `writeln!` sites; there are **504** across the production renderers, written into the `&mut
  String` of about twenty functions. Option (a) -- a `Report` method taking `fmt::Arguments` plus a
  macro -- is the most explicit and would have rewritten all 504; that is affordable at 160 and is not
  at 504. Option (b) moves the twenty signatures and leaves the 504 untouched, because `String`
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
  `emit_report` accordingly. All sixteen probes now take `out: &mut dyn std::fmt::Write`; the
  `emit_report` accordingly. All thirteen probes now take `out: &mut dyn std::fmt::Write`; the
  `catch_unwind`/`resume_unwind` pair is deleted, because with lines leaving as they are produced
  there is no buffer to rescue and keeping it would imply partial output still depends on the panic
  unwinding. `Captured` is unchanged and its tests pass untouched.

  **Three probes needed more than a signature change**, because they never went through
  `emit_report` at all -- `core_affinity`, `peer_index_cache` and `queue_contention` each composed a
  `String` and called `emit` directly. They are branch-local and so missed the round that fixed the
  same bypass in the peeled probes, which means the crate's "every probe routes through this" claim
  was false in three places. `core_affinity` additionally measured in `main`'s argument list, ahead
  of the renderer, so a topology read that failed produced no banner at all; it now measures after
  the banner and reports the failure as a failure to observe rather than as a finding.

  **Verified with a control, because these probes are not deterministic.** A direct before/after
  comparison flagged nine of fifteen reports, which is not evidence -- they print measured
  nanoseconds and branch their verdicts on them. Running the *same* build twice differed by as much
  or more (`peer-index-cache`: 22 lines between two runs of one build, against 20 across the
  conversion), and the twelve deterministic reports were structurally identical. A before/after diff
  on a probe means nothing without that control.
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
  [DESIGN-NOTES.md](DESIGN-NOTES.md). `probe-queue-contention` (~65 s), stdout redirected, killed at
  8 s: the streaming build had **114 bytes** on disk (banner and heading), the pre-M1.2 build built
  from `246687e` had **0**. The control is what makes it evidence rather than an observation.
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

## Moved 2026-09-09 -- M2: the report's parts are checked against each other

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
