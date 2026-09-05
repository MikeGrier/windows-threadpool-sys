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

- [ ] **CW-2.4** -- Document the layouts as a choice, in the crate documentation
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

- [ ] **CW-2.5** -- Reopen `D-36` with the measurement in hand, then sweep every
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

- [ ] **CW-1.6** -- Delete the duplicated *implementation* in
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
