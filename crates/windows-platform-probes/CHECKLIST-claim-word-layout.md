# Checklist: claim-word layout measurement

Measures how the `reserving_mpsc` claim word's bit apportionment and width affect
push throughput, so the shipping crate's layout can be chosen on evidence rather
than on the single 32/32 split it inherited.

Design decisions land in [DESIGN-NOTES.md](DESIGN-NOTES.md); the decisions this
informs live in the queue crate's
[DESIGN-NOTES.md](../windows-waitable-queues/DESIGN-NOTES.md) (`D-36`, `D-37`).

## Background

`reserving_mpsc` packs `reserved` and `position` into one `AtomicU64` because the
claim protocol needs a single compare-and-swap to update both (`D-17`, `D-34`).
The split is 32/32, which caps positions at 2^32 and is the whole source of the
`SH-14.1` recurrence hazard disclosed by `D-36`.

**The 32/32 split is not forced by the platform.** It follows from a capacity
ceiling of 2^31, because the `reserved` half must be able to hold the entire
capacity. Two independent constraints bound the capacity:

- ring arithmetic: `capacity <= 2^(POSITION_BITS - 1)`
- packing: `capacity <= 2^(64 - POSITION_BITS) - 1`

`BOUNDS_MAX` is currently derived from the first alone and the second is only
*asserted*, so widening the position raises the ceiling while shrinking the field
obliged to hold it -- which is why widening trips the assertion instead of
working. Deriving the ceiling as the minimum of both makes asymmetric splits
expressible.

`D-37` offers only 32/32 and a 128-bit 64/64. The asymmetric middle ground is
unexplored, and it needs no new dependency and no 128-bit exchange.

## M1: measure the layouts

The variants are built as a **duplicated, experimental path in this crate**, not
in the queue crate. `windows-platform-probes` is explicitly experiments rather
than components, so the measurement adds no third-party dependency to a
publishable crate and cannot disturb the `windows-waitable-queues` branch being
peeled off PR #56. The merge-or-delete decision is `CW-1.6`.

- [x] **CW-1.1** -- Add `portable-atomic` with `default-features = false` to this
  crate only, and record whether `AtomicU128` exists and is lock-free on
  `x86_64-pc-windows-msvc`. `D-37` measured that the `use` statement is itself
  the gate; confirm that still holds and note whether the implementation uses a
  compile-time guarantee or runtime detection, because a CPUID branch in the
  claim path would be measured as if it were the algorithm's cost.

- [x] **CW-1.2** -- Implement the three claim-word layouts as self-contained
  `u64`-item queues: `narrow` (32/32 over `AtomicU64`, mirroring the shipping
  shape), `deep` (16/48 over `AtomicU64`), and `wide` (64/64 over `AtomicU128`).
  Hand-written rather than generic over a layout trait, matching the existing
  reason `time_isolated_permit` is a line-for-line twin of its neighbour: an
  abstraction that might not inline identically would be measured as the
  algorithm's cost. `deep` and `wide` must widen `head` and the per-slot
  `sequence` to 64 bits, since a sequence narrower than the position aliases and
  reintroduces the recurrence on the consumer side.

- [x] **CW-1.3** -- Wire the three layouts into `probe-queue-contention` as
  named shapes in both the isolated and drained regimes, and report them against
  the existing `BASELINE_FETCH_ADD` floor.

- [x] **CW-1.4** -- Run the probe and capture the report. State plainly whether
  32/32 and 16/48 differ: both are one `AtomicU64` exchange and should be
  indistinguishable in the claim itself, so a difference is evidence about slot
  metadata density rather than about the claim, and no difference is the result
  that makes the apportionment free.

- [x] **CW-1.5** -- Record the measurement in [DESIGN-NOTES.md](DESIGN-NOTES.md)
  with the host fingerprint, and raise the finding against the queue crate's
  `D-37` so the shipping decision has the number it currently lacks.

- [ ] **CW-1.6** -- Decide merge-or-delete for the duplicated path: either the
  layouts are promoted into `windows-waitable-queues` (which is `M2`) and the
  probe keeps only what it needs to compare them, or the experiment is deleted.
  Recorded here so a duplicated path cannot become permanent by nobody
  returning to it.

## M2+: expose the apportionment

> **CROSS-COMPONENT PREREQUISITE:** every item below changes
> `windows-waitable-queues` and is gated on `CW-1.4`'s numbers and on the
> `mikegrier/waitable-queues` peel merging. Parked deliberately, not pending.

**Decided: offer a set of named layouts rather than one, on the condition that
each carries its own ramifications.** The engineer's direction was that options
are the right answer "as long as quality is maintained" and "the ramifications
of the choices are available". Both halves are binding, and the second is the
one an options API usually fails: a caller who cannot see what a layout costs
will pick by name, and the names are the least informative thing about them.

Three obligations follow, and they apply to every item in this milestone:

- **Each layout states its own consequences where it is named** -- reservation
  ceiling, capacity ceiling, and time-to-recurrence at a stated push rate. Not
  in a table elsewhere that the layout links to; a caller reading the type must
  see the trade. The rollover figures already exist in
  [DESIGN-NOTES.md](DESIGN-NOTES.md) and are the source, not a second copy: the
  crate documentation restates them once and nothing else does.
- **Quality is per-layout, not per-crate.** Every layout gets the same const
  assertions, the same tests, and the same mutation coverage as the shipping
  one. A layout that exists but is exercised only by a doctest is worse than no
  option, because its presence claims a support level nothing verifies.
- **Adding a layout must not weaken the default.** Callers who never name one
  keep today's semantics unless `CW-2.3` explicitly changes them, and the
  layout parameter must not leak into the signatures of callers who do not use
  it. If it cannot be kept out of them, say so rather than accepting the churn.

- [ ] **CW-2.1** -- Derive `BOUNDS_MAX` from both constraints rather than from
  the ring bound alone, so that widening the position narrows the capacity
  ceiling instead of tripping a const assertion.

  **The measurement changed this item's shape.** Deriving the ceiling from both
  constraints is not sufficient on its own: a 16-bit `reserved` half would cap
  the capacity at 65535, and the probe's own isolated regime wants 2^21. What
  makes 16/48 usable is **decoupling the reservation ceiling from the
  capacity** -- capping *outstanding reservations* at `MAX_RESERVED` while the
  capacity stays bounded only by the ring. That is what
  [claim_layout.rs](src/claim_layout.rs) measured, and it is a **contract
  change**: the shipping shape promises every slot may be reserved at once, and
  this replaces that with a fixed reservation ceiling. A different promise
  rather than a broken one, but it must be decided and stated, not slipped in.

- [ ] **CW-2.2** -- Make the apportionment caller-selectable at compile time,
  defaulting to today's behaviour so no existing caller changes. Generic
  defaults are permitted on types but not on functions, so the entry points
  need deciding rather than assuming.

  **The default is the substantive question, not the mechanism.** The rollover
  table in [DESIGN-NOTES.md](DESIGN-NOTES.md) shows the reservation half holds
  four billion where hundreds would do, and that trading it away is what buys
  the position bits: 2^12 reservations leaves over a year before recurrence and
  2^8 leaves twenty years, against today's 37 seconds. A changed default is a
  contract change for anyone who read the current capacity ceiling, so it is
  `CW-2.3`'s decision to make explicitly -- but leaving the default at 32/32
  because it is the status quo would preserve `SH-14.1` by inertia.

- [ ] **CW-2.3** -- Decide whether a 128-bit claim word becomes the default, on
  `CW-1.4`'s evidence. This is the question behind `D-37`'s conditional gating,
  and the engineer has said 32-bit Windows deployment is not a present concern
  -- which changes `D-18`'s premise and must be recorded rather than assumed.

- [ ] **CW-2.4** -- Document the layouts as a choice, in the crate documentation
  and the README, with the rollover table and the two axes a caller actually
  trades between: outstanding reservations against time-to-recurrence. Lead with
  the fact `CW-1.4` measured -- re-apportioning is free, widening is not -- so a
  caller is not left assuming the safest option must be the slowest. State the
  push rate the figures assume and that a draining queue cannot sustain the
  fastest of them, or the numbers will be read as forecasts rather than as the
  floor they are.

  **Compiled, not merely written.** Any README example naming a layout is a
  doctest per this repository's CONTRACT INTEGRITY rule, so a layout that is
  renamed or removed breaks the build instead of leaving the documentation
  teaching a name that no longer exists.

- [ ] **CW-2.5** -- Sweep the existing `SH-14.1` disclosure once the layouts
  land. `D-36` states the hazard in the crate documentation, the README, and
  `reserving_mpsc`'s module documentation, each leading with "on every target,
  not only 32-bit ones", and `lib.rs` separately claims the shape is "sound
  below the wrap". Every one of those statements is scoped to a 32-bit position
  and becomes wrong for a caller who selected a different layout.

  This is a blast-radius sweep, not an edit of the site someone reports: grep
  the distinguishing terms across `src/`, `tests/`, `examples/` and `*.md` for
  the crate and its dependents, fix every hit or say why it is out of scope, and
  record the sweep in the commit message.
