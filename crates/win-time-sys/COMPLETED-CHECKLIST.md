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
