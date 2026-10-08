// Copyright (c) 2026 Mike Grier
//! Clocks: the getters that read now on a timeline ([WT-D-2]).
//!
//! [WT-D-2]: https://github.com/MikeGrier/windows-threadpool-sys/blob/main/crates/win-time-sys/DESIGN-NOTES.md#wt-d-2

use crate::timeline::{TimePoint, Timeline};

/// A way to read "now" on one timeline. Several clocks may share a timeline, differing only in
/// what a reading costs and how fresh it is; their readings compare with each other because they
/// are points on the same timeline.
///
/// `now` takes `&self` ([WT-D-7]), so a clock may carry state -- a test's fake clock, or a value it
/// reads once and keeps -- and code that is generic over a clock is handed one.
///
/// ```
/// use core::cell::Cell;
/// use core::num::NonZeroU64;
/// use win_time_sys::{Clock, TimePoint, Ticks, Timeline};
///
/// /// Ticks of one millisecond since some zero.
/// enum Millis {}
/// impl Timeline for Millis {
///     fn ticks_per_second() -> NonZeroU64 { NonZeroU64::new(1_000).unwrap() }
/// }
///
/// /// A fake clock a test advances by hand.
/// struct Manual(Cell<u64>);
/// impl Clock for Manual {
///     type Timeline = Millis;
///     fn now(&self) -> TimePoint<Millis> { TimePoint::from_ticks(self.0.get()) }
/// }
///
/// fn elapsed<C: Clock>(clock: &C, since: TimePoint<C::Timeline>) -> Ticks<C::Timeline> {
///     clock.now() - since
/// }
///
/// let clock = Manual(Cell::new(1_000));
/// let start = clock.now();
/// clock.0.set(1_250);
/// assert_eq!(elapsed(&clock, start).to_duration(), Some(core::time::Duration::from_millis(250)));
/// ```
///
/// [WT-D-7]: https://github.com/MikeGrier/windows-threadpool-sys/blob/main/crates/win-time-sys/DESIGN-NOTES.md#wt-d-7
pub trait Clock {
    /// The timeline this clock reads.
    type Timeline: Timeline;

    /// The current point on the timeline.
    fn now(&self) -> TimePoint<Self::Timeline>;
}
