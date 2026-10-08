// Copyright (c) 2026 Mike Grier
//! The performance counter, and the clock that reads it ([WT-D-3], [WT-D-4]).
//!
//! QPC's raw ticks, not `std::time::Instant` -- which reads the same counter but keeps its value to
//! itself -- so a point can be recorded and read back. The counter is steady within a boot and, as
//! Microsoft documents it, "independent of and isn't synchronized to any external time reference":
//! its zero is unspecified, and a point means nothing after a restart.
//!
//! The tick's length is the counter's frequency, known only at run time. Microsoft documents it as
//! "fixed at system boot and is consistent across all processors", so it is read once and kept.
//!
//! [WT-D-3]: https://github.com/MikeGrier/windows-threadpool-sys/blob/main/crates/win-time-sys/DESIGN-NOTES.md#wt-d-3
//! [WT-D-4]: https://github.com/MikeGrier/windows-threadpool-sys/blob/main/crates/win-time-sys/DESIGN-NOTES.md#wt-d-4

use core::num::NonZeroU64;
use std::sync::OnceLock;

use windows_sys::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

use crate::clock::Clock;
use crate::timeline::{TimePoint, Timeline};

#[cfg(test)]
mod tests;

/// The performance counter: ticks at its frequency, from an unspecified zero within this boot.
///
/// Its points do not compare with interrupt time's, though both count from around the boot:
///
/// ```compile_fail,E0308
/// use win_time_sys::{Clock, InterruptClock, PerformanceClock};
///
/// let _ = PerformanceClock.now() < InterruptClock.now();
/// ```
pub enum PerformanceCounter {}

impl Timeline for PerformanceCounter {
    fn ticks_per_second() -> NonZeroU64 {
        static FREQUENCY: OnceLock<NonZeroU64> = OnceLock::new();
        *FREQUENCY.get_or_init(frequency)
    }
}

/// [`PerformanceCounter`], through `QueryPerformanceCounter`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PerformanceClock;

impl Clock for PerformanceClock {
    type Timeline = PerformanceCounter;

    fn now(&self) -> TimePoint<PerformanceCounter> {
        let mut ticks = 0;
        // SAFETY: the out-parameter is a live, writable local for the duration of the call.
        let succeeded = unsafe { QueryPerformanceCounter(&mut ticks) };
        // Documented to "always succeed" on Windows XP and later, with a count that is never
        // negative.
        debug_assert_ne!(succeeded, 0, "QueryPerformanceCounter failed");
        TimePoint::from_ticks(ticks.cast_unsigned())
    }
}

/// The counter's frequency, in ticks per second.
fn frequency() -> NonZeroU64 {
    let mut per_second = 0;
    // SAFETY: the out-parameter is a live, writable local for the duration of the call.
    let succeeded = unsafe { QueryPerformanceFrequency(&mut per_second) };
    // Documented to "always succeed" on Windows XP and later, so the frequency is never zero.
    assert_ne!(succeeded, 0, "QueryPerformanceFrequency failed");
    NonZeroU64::new(per_second.cast_unsigned()).expect("a performance counter frequency of zero")
}
