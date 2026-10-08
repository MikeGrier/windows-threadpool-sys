// Copyright (c) 2026 Mike Grier
//! The system-time clocks, read for real. System time is not steady -- it moves when the time is
//! set -- so nothing here asserts it never goes backwards; each test reads over too short a span for
//! an adjustment to land in it by chance.

use std::thread;
use std::time::{Duration, Instant, SystemTime};

use super::{CoarseSystemClock, FileTime, PreciseSystemClock};
use crate::{Clock, Ticks, Timeline};

/// `FILETIME`'s count at the Unix epoch, 1970-01-01 UTC: 369 years of 100 ns ticks after 1601.
const UNIX_EPOCH_TICKS: u64 = 116_444_736_000_000_000;
/// Longer than the coarsest system clock tick, 15.625 ms, with room to spare.
const ACROSS_A_TICK: Duration = Duration::from_millis(50);
/// Shorter than the finest system clock tick, 0.5 ms.
const WITHIN_A_TICK: Duration = Duration::from_micros(50);

fn spin(duration: Duration) {
    let started = Instant::now();
    while started.elapsed() < duration {
        std::hint::spin_loop();
    }
}

/// `SystemTime::now`, as `FILETIME` ticks.
fn std_now_ticks() -> u64 {
    let since_unix = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("after 1970");
    let ticks = Ticks::<FileTime>::from_duration(since_unix).expect("fits");
    UNIX_EPOCH_TICKS + ticks.count().unsigned_abs()
}

#[test]
fn a_precise_reading_is_what_std_reads_as_the_time() {
    // `SystemTime::now` reads the same call, so the only difference is the time between the reads.
    // A wrong epoch, or the `FILETIME` halves put together wrongly, is off by years.
    let before = std_now_ticks();
    let ours = PreciseSystemClock.now().ticks();
    let after = std_now_ticks();
    assert!(
        before <= ours && ours <= after,
        "{before} <= {ours} <= {after}"
    );
}

#[test]
fn a_coarse_reading_is_a_precise_one_as_of_the_last_tick() {
    let before = PreciseSystemClock.now();
    let coarse = CoarseSystemClock.now();
    let after = PreciseSystemClock.now();
    assert!(coarse <= after, "{coarse:?} is ahead of {after:?}");
    let lag = before - coarse;
    assert!(
        lag < Ticks::from_duration(Duration::from_millis(20)).unwrap(),
        "{lag:?} behind"
    );
}

#[test]
fn the_precise_clock_advances_within_a_fraction_of_a_tick() {
    for window in 0..20 {
        let before = PreciseSystemClock.now();
        spin(WITHIN_A_TICK);
        let after = PreciseSystemClock.now();
        assert!(after > before, "window {window}: still at {before:?}");
    }
}

#[test]
fn the_coarse_clock_advances_across_a_tick() {
    let before = CoarseSystemClock.now();
    thread::sleep(ACROSS_A_TICK);
    assert!(CoarseSystemClock.now() > before);
}

#[test]
fn an_interval_measures_what_instant_measures() {
    let outer_start = Instant::now();
    let start = PreciseSystemClock.now();
    thread::sleep(Duration::from_millis(100));
    let end = PreciseSystemClock.now();
    let outer = outer_start.elapsed();
    let inner = (end - start).to_duration().expect("forwards");
    assert!(
        inner <= outer + Duration::from_millis(2),
        "{inner:?} inside {outer:?}"
    );
    assert!(
        outer - inner.min(outer) < Duration::from_millis(25),
        "{inner:?} inside {outer:?}"
    );
}

#[test]
fn system_time_ticks_every_100_nanoseconds() {
    assert_eq!(FileTime::ticks_per_second().get(), 10_000_000);
}

#[test]
fn a_clock_is_a_zero_sized_value() {
    assert_eq!(size_of::<CoarseSystemClock>(), 0);
    assert_eq!(size_of::<PreciseSystemClock>(), 0);
}

#[test]
fn both_halves_of_a_filetime_reach_the_count() {
    use windows_sys::Win32::Foundation::FILETIME;
    let time = FILETIME {
        dwLowDateTime: 0x8765_4321,
        dwHighDateTime: 0x0123_4567,
    };
    assert_eq!(super::point(time).ticks(), 0x0123_4567_8765_4321);
}
