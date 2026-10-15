// Copyright (c) 2026 Mike Grier
//! System time, and the clocks that read it ([WT-D-3]).
//!
//! System time is UTC wall-clock time, so unlike interrupt time it means something after a
//! restart and on another machine -- and unlike interrupt time it is not steady: it moves when the
//! time is set or synchronised, backwards included. Elapsed time within one boot is interrupt
//! time's to measure.
//!
//! Both clocks return the same `FILETIME`, so they share one timeline and their readings compare;
//! they differ only in how fresh the reading is. The coarse clock is as fresh as the last system
//! clock tick. The precise clock is documented as "the highest possible level of precision
//! (<1us)". What each costs on a given machine is `WT-1.6`'s probe to measure.
//!
//! [WT-D-3]: https://github.com/MikeGrier/windows-threadpool-sys/blob/main/crates/win-time-sys/DESIGN-NOTES.md#wt-d-3

use core::num::NonZeroU64;

use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::System::SystemInformation::{
    GetSystemTimeAsFileTime, GetSystemTimePreciseAsFileTime,
};

use crate::clock::Clock;
use crate::timeline::{HUNDRED_NANOSECOND_TICKS, TimePoint, Timeline};

#[cfg(test)]
mod tests;

/// System time: 100 ns ticks since midnight at the start of 1 January 1601, UTC -- Windows'
/// `FILETIME`.
///
/// Its points do not compare with interrupt time's, which share its unit but not its zero:
///
/// ```compile_fail,E0308
/// use win_time_sys::{Clock, InterruptClock, PreciseSystemClock};
///
/// let _ = PreciseSystemClock.now() < InterruptClock.now();
/// ```
pub enum FileTime {}

impl Timeline for FileTime {
    fn ticks_per_second() -> NonZeroU64 {
        HUNDRED_NANOSECOND_TICKS
    }
}

/// [`FileTime`] as of the last system clock tick, through `GetSystemTimeAsFileTime`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct CoarseSystemClock;

impl Clock for CoarseSystemClock {
    type Timeline = FileTime;

    fn now(&self) -> TimePoint<FileTime> {
        let mut time = FILETIME::default();
        // SAFETY: the out-parameter is a live, writable local for the duration of the call.
        unsafe { GetSystemTimeAsFileTime(&mut time) };
        point(time)
    }
}

/// [`FileTime`] to under a microsecond, through `GetSystemTimePreciseAsFileTime`: finer than
/// [`CoarseSystemClock`], and its readings compare with it. `std::time::SystemTime::now` reads the
/// same call.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PreciseSystemClock;

impl Clock for PreciseSystemClock {
    type Timeline = FileTime;

    fn now(&self) -> TimePoint<FileTime> {
        let mut time = FILETIME::default();
        // SAFETY: the out-parameter is a live, writable local for the duration of the call.
        unsafe { GetSystemTimePreciseAsFileTime(&mut time) };
        point(time)
    }
}

/// A `FILETIME`'s two halves, as the one count they are.
fn point(time: FILETIME) -> TimePoint<FileTime> {
    TimePoint::from_ticks((u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime))
}
