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

## Moved 2026-10-07 23:45:07 -04:00 -- WT-1.4: system time

### <a id="wt-14"></a>WT-1.4 -- System time: the `FileTime` timeline and its coarse and precise clocks. *(completed 2026-10-07 23:45:07 -04:00)*

`FileTime` -- 100 ns ticks since 1601-01-01 UTC -- with `CoarseSystemClock` over
`GetSystemTimeAsFileTime` and `PreciseSystemClock` over `GetSystemTimePreciseAsFileTime`
([WT-D-3](DESIGN-NOTES.md#wt-d-3)). The documentation says plainly that system time is not steady --
it moves when the time is set, backwards included -- and leaves elapsed time to interrupt time. A
`FILETIME`'s two halves are joined in one function, tested directly. No conversion to
`std::time::SystemTime` is offered, per [WT-D-1](DESIGN-NOTES.md#wt-d-1).

**Tests** (`system/tests.rs`), reading the real clocks: a precise reading lies between two of
`SystemTime::now`, which reads the same call -- the check that the epoch and the halves are right;
a coarse reading lags a precise one by at most a tick; the precise clock advances within fractions of
the finest tick and the coarse one across the coarsest; a precise interval nests inside `Instant`'s;
the period; zero-sized clocks; and the halves joined in the right order. Because system time is not
steady, no test asserts it never goes backwards. A `compile_fail` doctest pinned to `E0308` shows its
points do not compare with interrupt time's. The test binary ran 30 times in sequence without a
failure.

**Verification.** Four sabotages, each caught: the precise clock reading the coarse call, a frozen
coarse clock, the halves swapped, and the high half shifted too little; with adding the halves rather
than or-ing them as a third control. The whole manifest ran as declared.

The item as it stood at completion:

- [x] **WT-1.4** -- **System time**: the `FileTime` timeline and its two clocks, over
  `GetSystemTimeAsFileTime` and `GetSystemTimePreciseAsFileTime`.

## Moved 2026-10-07 23:48:47 -04:00 -- WT-1.5: performance time

### <a id="wt-15"></a>WT-1.5 -- Performance time: the `PerformanceCounter` timeline, its frequency read once and kept, and `PerformanceClock` over QPC's raw ticks. *(completed 2026-10-07 23:48:47 -04:00)*

The timeline is named `PerformanceCounter` -- what [WT-D-3](DESIGN-NOTES.md#wt-d-3) calls
"Performance" -- and its clock `PerformanceClock`, over `QueryPerformanceCounter`'s raw ticks rather
than `Instant` ([WT-D-4](DESIGN-NOTES.md#wt-d-4)). Its period is `QueryPerformanceFrequency`, read on
first use and kept in a `OnceLock`, which Microsoft documents as fixed at boot. Both calls are
documented to always succeed on Windows XP and later: the counter's result is asserted in debug
builds, and the frequency's in every build, since a zero period would make every conversion wrong.
The README now lists all four timelines.

**Tests** (`performance/tests.rs`), reading the real counter: it never goes backwards; it advances in
every one of twenty windows shorter than the finest system tick; the period equals the frequency
Windows reports, and is the same on every call; an interval nests inside `Instant`'s measure of it --
with no tolerance on the upper side, since `Instant` reads the same counter; and the clock is
zero-sized. A `compile_fail` doctest pinned to `E0308` shows its points do not compare with interrupt
time's. The test binary ran 30 times in sequence without a failure.

**Verification.** Two sabotages, each caught: a period of twice the frequency, and a frozen clock;
with reinterpreting the count by `as` as a fourth control. Added to `notCoveredHere`: QPC's
documented-unreachable failures, and reading the frequency on every call, which changes a call's
cost and never its answer. The whole manifest ran as declared.

The item as it stood at completion:

- [x] **WT-1.5** -- **Performance time**: the timeline over QPC's frequency, read once and kept, and
  its clock over `QueryPerformanceCounter`'s raw ticks ([WT-D-4](DESIGN-NOTES.md#wt-d-4)).

## Moved 2026-10-07 23:55:08 -04:00 -- WT-1.6, and WT-M1 complete: the crate and its clocks

WT-M1 completed with this item; its heading and its items' stubs left [CHECKLIST.md](CHECKLIST.md)
together. The milestone's closing checks: the default workspace builds with no warnings in debug and
release, and this crate's tests, doctests and the example's tests pass. The branch is not pushed: it
waits on the engineer.

### <a id="wt-16"></a>WT-1.6 -- A cost probe: the `clock_costs` example times every clock on the machine it runs on and reports what it observed. *(completed 2026-10-07 23:55:08 -04:00)*

Placed as an example in this crate rather than in `windows-platform-probes`, because an example
ships with the published crate and the probe exists for consumers' hardware
([WT-D-8](DESIGN-NOTES.md#wt-d-8), for the engineer to confirm). For each of the seven clocks, and
`Instant` and `SystemTime` for reference, it reports the least, median and most cost per read over
thirty-one batches of ten thousand reads, timed with `Instant` after a warm-up batch, and the smallest
step seen between readings that differ. It compares nothing and quotes nothing into the
documentation; all of its output goes through one writer. The README says what it measures and how
to run it.

**Tests** (`examples/clock_costs/tests.rs`, run by `cargo test` because the example is declared
`test = true`): the summary's least, lower-middle median and most for odd, even and single counts,
and none for no samples; the smallest step ignoring repeats and backward moves, and none for a clock
that never moved; and the report naming every clock and saying when no step was seen. The probe was
run in release on the development machine and completed in a fraction of a second.

**Verification.** Two sabotages, each caught by the test named for it: an upper-middle median, and a
repeated reading counted as a zero step. The whole manifest ran as declared.

The item as it stood at completion:

- [x] **WT-1.6** -- **A cost probe**: time each clock on the machine it runs on and report what was
  observed, so the choice between getters rests on the consumer's own hardware rather than on figures
  quoted from elsewhere. Where it lives -- an example here, or `windows-platform-probes` -- is decided
  with the item.

The milestone's stubs as they stood when it left [CHECKLIST.md](CHECKLIST.md):

- [x] **WT-1.1** -- The crate exists: a Windows-only workspace member registered for release and publication, with its README as the crate documentation and an undocumented `unsafe` refused by the build. -> [completed 2026-10-07](COMPLETED-CHECKLIST.md#wt-11)
- [x] **WT-1.2** -- The two layers: `Timeline`, `TimePoint<T>`, `Ticks<T>` and `Clock`, with their arithmetic and conversions to and from `Duration`. -> [completed 2026-10-07](COMPLETED-CHECKLIST.md#wt-12)
- [x] **WT-1.3** -- Interrupt time and unbiased interrupt time: two timelines and their four clocks, plain and precise. -> [completed 2026-10-07](COMPLETED-CHECKLIST.md#wt-13)
- [x] **WT-1.4** -- System time: the `FileTime` timeline and its coarse and precise clocks. -> [completed 2026-10-07](COMPLETED-CHECKLIST.md#wt-14)
- [x] **WT-1.5** -- Performance time: the `PerformanceCounter` timeline, its frequency read once and kept, and `PerformanceClock` over QPC's raw ticks. -> [completed 2026-10-07](COMPLETED-CHECKLIST.md#wt-15)
- [x] **WT-1.6** -- A cost probe: the `clock_costs` example times every clock on the machine it runs on and reports what it observed. -> [completed 2026-10-07](COMPLETED-CHECKLIST.md#wt-16)

## Moved 2026-10-08 00:10:01 -04:00 -- WT-2.1: the thread pool reads interrupt time through this crate

### <a id="wt-21"></a>WT-2.1 -- `windows-threadpool-sys` reads interrupt time through `InterruptClock`, so the workspace has one reader of its one time base. *(completed 2026-10-08 00:10:01 -04:00)*

The self-heal's crate-private `heal::now()` no longer calls `QueryInterruptTime` itself: its body is
`InterruptClock.now().ticks()`, and its `unsafe` block is gone with the call. The function stays, as
a `u64` adapter, because every stamp it feeds is an `AtomicU64` -- a `TimePoint` cannot sit in an
atomic -- and because its call sites are what three of the thread pool's sabotages anchor on.
`OVERDUE_AFTER` stays a raw count of 100 ns ticks for the same reason. `win-time-sys` is an optional
dependency of the thread pool, pulled in by `self-heal` alone, which no longer needs
`windows-sys/Win32_System_WindowsProgramming`; a consumer who turns `self-heal` off compiles neither.

**Behaviour is unchanged**, so the commit is a `refactor` and cuts no release. The values are the
same counter's, read by the same call: `InterruptClock` is `QueryInterruptTime`
([WT-D-3](DESIGN-NOTES.md#wt-d-3)).

**Verification.** The thread pool's whole suite passed, doctests included, with default features and
built with `--no-default-features`. The three sabotages anchored on `now()` call sites -- a
cancellation that does not arm a repair, a dispatch that never updates its stamp, and a repair
waited on forever -- were each still caught. The thirty-run check of the self-heal tests turned up
an intermittent failure in two tests that force a repair-allocation failure; the unchanged tree
showed it too, so it predates this item. It is recorded in the thread pool's
[UNRESOLVED-TEST-FAILURES.md](../windows-threadpool-sys/UNRESOLVED-TEST-FAILURES.md).

The item as it stood at completion:

- [x] **WT-2.1** -- **`windows-threadpool-sys` reads interrupt time through this crate**, retiring its
  crate-private `heal::now()`, so the workspace has one reader of its one time base.

## Moved 2026-10-08 00:29:30 -04:00 -- WT-2.2.1: Steady

### <a id="wt-221"></a>WT-2.2.1 -- `Steady`, the marker trait for a clock whose readings never decrease, made by the interrupt and performance clocks and not by the system clocks. *(completed 2026-10-08 00:29:30 -04:00)*

`pub trait Steady: Clock {}`, in `clock.rs` and exported from the crate root
([WT-D-9](DESIGN-NOTES.md#wt-d-9)). Its documentation states the promise -- a reading taken after
another is at or after it -- and what it does not promise: a rate, freshness, or that two readings
differ. `InterruptClock`, `PreciseInterruptClock`, `UnbiasedInterruptClock`,
`PreciseUnbiasedInterruptClock` and `PerformanceClock` implement it; `CoarseSystemClock` and
`PreciseSystemClock` do not. The README's description of the layers names it.

**Tests.** The helpers that check a thousand readings never go backwards now take `C: Steady`, so they
are the trait's promise checked on every clock that makes it; the performance clock's check became
such a helper too. A passing doctest uses `InterruptClock` and `PerformanceClock` where `Steady` is
required, and a `compile_fail` doctest pinned to `E0277` shows `PreciseSystemClock` is refused there.
The test binary ran 30 times in sequence without a failure.

**Verification.** Three sabotages, each as declared: a system clock claiming `Steady` is caught by the
`compile_fail` doctest, and the performance and plain interrupt clocks losing their implementations
are refused by the build, with the `E0277` message naming each. Added to `notCoveredHere`: a clock
that claims `Steady` with no test holding it to the promise, since the tests list their clocks by hand
and Rust cannot enumerate a trait's implementations; and a mock elsewhere that breaks the promise,
which is its owner's to catch. Every entry behaved as declared: the whole manifest ran, then the two
`refused-by-build` entries again after being reclassified from `caught`.

The item as it stood at completion:

- [x] **WT-2.2.1** -- **`Steady`** ([WT-D-9](DESIGN-NOTES.md#wt-d-9)): the marker trait for a clock
  whose readings never decrease, implemented by the four interrupt clocks and `PerformanceClock` and
  not by the system clocks. The tests that no clock goes backwards are bound to the trait, so every
  `Steady` clock is held to it; a `compile_fail` doctest shows a system clock is not `Steady`.
