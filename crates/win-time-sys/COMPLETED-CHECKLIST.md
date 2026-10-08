# Completed checklist: win-time-sys

Append-only record of finished [CHECKLIST.md](CHECKLIST.md) items.

## Moved 2026-10-07 23:11:28 -04:00 -- WT-1.1: the crate

### <a id="wt-11"></a>WT-1.1 -- The crate exists: a Windows-only workspace member registered for release and publication, with its README as the crate documentation and an undocumented `unsafe` refused by the build. *(completed 2026-10-07 23:11:28 -04:00)*

Registered as `win-sync-sys` was: a workspace member, in release-please's configuration and manifest
(at `0.0.0`, so the first release is `0.1.0`), and in the publish workflow's tags, manual-dispatch
choices and workspace-sibling list. The README states the two layers
([WT-D-2](DESIGN-NOTES.md#wt-d-2)) and what is deliberately absent. It has no dependencies yet: the
Win32 bindings arrive with the clocks that call them, so no feature list is guessed in advance.
"Each `unsafe` with its safety argument" is enforced rather than asked for:
`clippy::undocumented_unsafe_blocks` is denied, the first crate in the workspace to do so. Committed
as `chore`, because a crate with no API has nothing to release; `WT-1.2` is the first item that does.

The item as it stood at completion:

- [x] **WT-1.1** -- **Scaffold the crate.** `crates/win-time-sys`, a workspace member registered for
  release and publication, Windows-only, with a README saying what it is. Its `unsafe` is confined to
  the Win32 calls, each with its safety argument; everything it exports is safe.

## Moved 2026-10-07 23:16:03 -04:00 -- WT-1.2: the two layers

### <a id="wt-12"></a>WT-1.2 -- The two layers: `Timeline`, `TimePoint<T>`, `Ticks<T>` and `Clock`, with their arithmetic and conversions to and from `Duration`. *(completed 2026-10-07 23:16:03 -04:00)*

The shape is [WT-D-2](DESIGN-NOTES.md#wt-d-2); what the item left open was decided as
[WT-D-7](DESIGN-NOTES.md#wt-d-7), for the engineer to confirm -- above all that `Clock::now` takes
`&self`. A timeline is a type that is never made, so the value types implement their traits by hand
rather than by derive, which would demand them of the marker.

**Tests** (`timeline/tests.rs`), over three fake timelines -- 100 ns ticks, a period that divides no
power of ten, and ticks shorter than a nanosecond: recording and reading back a point; ordering and
hashing; signed distances; a distance that does not fit refused, at both edges of `i64`; moving a
point inside and off both ends of its timeline, including by the one distance with no opposite;
conversion to `Duration` exact, truncated and refused when backwards; conversion from `Duration`
truncated and refused when too long; the extremes converting without overflow; round trips; distance
arithmetic and its overflow; the panicking operators; and `Debug` naming the timeline. **Doctests:**
a fake clock a test advances by hand, read through a function generic over `Clock`; and a
`compile_fail` pinned to `E0308`, so it fails because points on two timelines are different types
and for no other reason.

**Verification.** A new [sabotage.json](sabotage.json): nine defects -- ordering, the sign of a
distance, both ends of a move, a backwards duration, both conversions' fractions, a wrapping
conversion, and a wrapping negation -- each caught by the test named for it, and an equivalent
change as the control, which survives.

The item as it stood at completion:

- [x] **WT-1.2** -- **The two layers** ([WT-D-2](DESIGN-NOTES.md#wt-d-2)): `Timeline`, `TimePoint<T>`,
  `Ticks<T>` and `Clock`, with ordering, subtraction to `Ticks`, adding `Ticks` to a point, and
  `Ticks` to `Duration`. Tested over a fake timeline and clock, including overflow at the edges and
  that points on two timelines cannot be compared (a `compile_fail` doctest).

## Moved 2026-10-07 23:29:06 -04:00 -- WT-1.3: interrupt time

### <a id="wt-13"></a>WT-1.3 -- Interrupt time and unbiased interrupt time: two timelines and their four clocks, plain and precise. *(completed 2026-10-07 23:29:06 -04:00)*

`InterruptTime` and `UnbiasedInterruptTime`, each with a plain clock and a precise one, over the four
`Query*InterruptTime*` calls ([WT-D-3](DESIGN-NOTES.md#wt-d-3)). The clocks are zero-sized values, as
[WT-D-7](DESIGN-NOTES.md#wt-d-7)'s `&self` lets them be. The 100 ns period is one constant system time
will share. `QueryUnbiasedInterruptTime`'s `BOOL` is asserted in debug builds; its documentation says
it fails only for a null pointer. The documentation the crate quotes on resolution and cost is
Microsoft's, cited at the module; the costs themselves are left to `WT-1.6`'s probe. The README, which
is the crate documentation, gained what is here and a runnable example.

**Tests** (`interrupt/tests.rs`), reading the real clocks: no clock goes backwards over a thousand
readings; each precise clock advances in every one of twenty windows shorter than the finest system
tick, which a tick-limited clock would almost surely fail; each plain clock advances across a sleep
longer than the coarsest tick; a plain reading is never ahead of a precise one taken after it, nor more
than a tick behind one taken before; a precise interval nests inside `Instant`'s measure of it; unbiased
time is never ahead of interrupt time; both periods are 100 ns; and the clocks are zero-sized. A
`compile_fail` doctest pinned to `E0308` shows the two timelines' points do not compare. The test binary
ran 30 times in sequence without a failure.

**Verification.** Four sabotages, each caught by the test named for it: each precise clock reading its
plain call, a frozen plain clock, and a wrong period; with an equivalent change as a second control.
Three defects no test can catch are named in the manifest's `notCoveredHere`, with why: a plain clock
reading the precise call (more precise, not wrong -- a cost difference); a clock reading the other
interrupt timeline (identical on a machine that has never slept); and the unreachable failure of
`QueryUnbiasedInterruptTime`.

The item as it stood at completion:

- [x] **WT-1.3** -- **Interrupt time and unbiased interrupt time** ([WT-D-3](DESIGN-NOTES.md#wt-d-3)):
  two timelines and four clocks over `QueryInterruptTime`, `QueryInterruptTimePrecise`,
  `QueryUnbiasedInterruptTime` and `QueryUnbiasedInterruptTimePrecise`.
