// Copyright (c) 2026 Mike Grier
//! The performance clock, read for real.

use std::thread;
use std::time::{Duration, Instant};

use windows_sys::Win32::System::Performance::QueryPerformanceFrequency;

use super::{PerformanceClock, PerformanceCounter};
use crate::{Clock, Steady, Timeline};

/// Shorter than the finest system clock tick, 0.5 ms.
const WITHIN_A_TICK: Duration = Duration::from_micros(50);

fn spin(duration: Duration) {
    let started = Instant::now();
    while started.elapsed() < duration {
        std::hint::spin_loop();
    }
}

/// What `Steady` promises, over a thousand readings.
fn never_goes_backwards<C: Steady>(clock: C) {
    let mut last = clock.now();
    for _ in 0..1_000 {
        let next = clock.now();
        assert!(next >= last, "{next:?} after {last:?}");
        last = next;
    }
}

#[test]
fn the_clock_never_goes_backwards() {
    never_goes_backwards(PerformanceClock);
}

#[test]
fn the_clock_advances_within_a_fraction_of_a_tick() {
    for window in 0..20 {
        let before = PerformanceClock.now();
        spin(WITHIN_A_TICK);
        let after = PerformanceClock.now();
        assert!(after > before, "window {window}: still at {before:?}");
    }
}

#[test]
fn the_period_is_the_frequency_windows_reports() {
    let mut reported = 0;
    // SAFETY: the out-parameter is a live, writable local for the duration of the call.
    unsafe { QueryPerformanceFrequency(&mut reported) };
    assert_eq!(
        PerformanceCounter::ticks_per_second().get(),
        reported.cast_unsigned()
    );
    assert_eq!(
        PerformanceCounter::ticks_per_second(),
        PerformanceCounter::ticks_per_second(),
        "read once and kept"
    );
}

#[test]
fn an_interval_measures_what_instant_measures() {
    // `Instant` reads the same counter, so this checks the period as much as the reading. The
    // clock's interval sits between two `Instant` intervals, one inside it and one around it, so
    // it holds whatever the scheduler does in the gaps and needs no limit on how long they take.
    let outer_start = Instant::now();
    let start = PerformanceClock.now();
    let inner_start = Instant::now();
    thread::sleep(Duration::from_millis(100));
    let inner = inner_start.elapsed();
    let end = PerformanceClock.now();
    let outer = outer_start.elapsed();
    let measured = (end - start).to_duration().expect("forwards");
    assert!(
        inner <= measured,
        "{measured:?} is shorter than {inner:?} inside it"
    );
    assert!(
        measured <= outer,
        "{measured:?} is longer than {outer:?} around it"
    );
}
#[test]
fn the_clock_is_a_zero_sized_value() {
    assert_eq!(size_of::<PerformanceClock>(), 0);
}
