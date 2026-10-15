// Copyright (c) 2026 Mike Grier
//! Interrupt time and unbiased interrupt time, and the clocks that read them ([WT-D-3]).
//!
//! Both count 100 ns ticks from the start of this boot, so a point means nothing after a restart.
//! They differ only in time spent asleep or hibernating, which interrupt time counts and unbiased
//! interrupt time leaves out. After the machine has slept once they no longer describe the same
//! instants, so they are separate timelines and their points do not compare.
//!
//! Each has two clocks. The plain clock reads the value the kernel publishes into every process,
//! so it never enters the kernel; it is as fresh as the last system clock tick, which Microsoft's
//! documentation puts "typically in the range of 0.5 milliseconds to 15.625 milliseconds,
//! depending on the hardware platform". The precise clock "reads the timer hardware directly",
//! which the same documentation says "can be slower". What each costs on a given machine is
//! `WT-1.6`'s probe to measure.
//!
//! [WT-D-3]: https://github.com/MikeGrier/windows-threadpool-sys/blob/main/crates/win-time-sys/DESIGN-NOTES.md#wt-d-3

use core::num::NonZeroU64;

use windows_sys::Win32::System::WindowsProgramming::{
    QueryInterruptTime, QueryInterruptTimePrecise, QueryUnbiasedInterruptTime,
    QueryUnbiasedInterruptTimePrecise,
};

use crate::clock::{Clock, Steady};
use crate::timeline::{HUNDRED_NANOSECOND_TICKS, TimePoint, Timeline};

#[cfg(test)]
mod tests;

/// Interrupt time: 100 ns ticks since this boot began, including time asleep or hibernating.
///
/// Zero is the start of the boot. On a checked ("debug") build of Windows the count starts about 49
/// days ahead, so that code which mishandles long uptimes fails early; a point is still valid there,
/// only larger. This is the workspace's one time base
/// ([DI-D-37](https://github.com/MikeGrier/windows-threadpool-sys/blob/main/crates/durable-ioring/DESIGN-NOTES.md#di-d-37)).
pub enum InterruptTime {}

impl Timeline for InterruptTime {
    fn ticks_per_second() -> NonZeroU64 {
        HUNDRED_NANOSECOND_TICKS
    }
}

/// Unbiased interrupt time: interrupt time without the time spent asleep or hibernating.
///
/// Its points do not compare with [`InterruptTime`]'s, which share its unit but, after the first
/// sleep, not its zero:
///
/// ```compile_fail,E0308
/// use win_time_sys::{Clock, InterruptClock, UnbiasedInterruptClock};
///
/// let _ = InterruptClock.now() < UnbiasedInterruptClock.now();
/// ```
pub enum UnbiasedInterruptTime {}

impl Timeline for UnbiasedInterruptTime {
    fn ticks_per_second() -> NonZeroU64 {
        HUNDRED_NANOSECOND_TICKS
    }
}

/// [`InterruptTime`] as of the last system clock tick, through `QueryInterruptTime`: the
/// cheapest reading, which never enters the kernel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct InterruptClock;

impl Clock for InterruptClock {
    type Timeline = InterruptTime;

    fn now(&self) -> TimePoint<InterruptTime> {
        let mut ticks = 0;
        // SAFETY: the out-parameter is a live, writable local for the duration of the call.
        unsafe { QueryInterruptTime(&mut ticks) };
        TimePoint::from_ticks(ticks)
    }
}

/// [`InterruptTime`] read from the timer hardware, through `QueryInterruptTimePrecise`: finer than
/// [`InterruptClock`], and its readings compare with it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PreciseInterruptClock;

impl Clock for PreciseInterruptClock {
    type Timeline = InterruptTime;

    fn now(&self) -> TimePoint<InterruptTime> {
        let mut ticks = 0;
        // SAFETY: the out-parameter is a live, writable local for the duration of the call.
        unsafe { QueryInterruptTimePrecise(&mut ticks) };
        TimePoint::from_ticks(ticks)
    }
}

/// [`UnbiasedInterruptTime`] as of the last system clock tick, through
/// `QueryUnbiasedInterruptTime`, which never enters the kernel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct UnbiasedInterruptClock;

impl Clock for UnbiasedInterruptClock {
    type Timeline = UnbiasedInterruptTime;

    fn now(&self) -> TimePoint<UnbiasedInterruptTime> {
        let mut ticks = 0;
        // SAFETY: the out-parameter is a live, writable local for the duration of the call.
        let succeeded = unsafe { QueryUnbiasedInterruptTime(&mut ticks) };
        // Documented to fail only for a null pointer, which a reference cannot be.
        debug_assert_ne!(succeeded, 0, "QueryUnbiasedInterruptTime failed");
        TimePoint::from_ticks(ticks)
    }
}

/// [`UnbiasedInterruptTime`] read from the timer hardware, through
/// `QueryUnbiasedInterruptTimePrecise`: finer than [`UnbiasedInterruptClock`], and its readings
/// compare with it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PreciseUnbiasedInterruptClock;

impl Clock for PreciseUnbiasedInterruptClock {
    type Timeline = UnbiasedInterruptTime;

    fn now(&self) -> TimePoint<UnbiasedInterruptTime> {
        let mut ticks = 0;
        // SAFETY: the out-parameter is a live, writable local for the duration of the call.
        unsafe { QueryUnbiasedInterruptTimePrecise(&mut ticks) };
        TimePoint::from_ticks(ticks)
    }
}

// Both timelines count up from the start of the boot and are never set, so none of their four
// clocks goes backwards.
impl Steady for InterruptClock {}
impl Steady for PreciseInterruptClock {}
impl Steady for UnbiasedInterruptClock {}
impl Steady for PreciseUnbiasedInterruptClock {}
