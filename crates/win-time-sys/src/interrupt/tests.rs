// Copyright (c) 2026 Mike Grier
//! The interrupt-time clocks, read for real. Each test checks a property a defect would break:
//! readings that go backwards, a precise clock no finer than the system tick, a plain clock that
//! does not advance, a wrong period, or one timeline's reading standing in for the other's.

use std::thread;
use std::time::{Duration, Instant};

use super::{
    InterruptClock, InterruptTime, PreciseInterruptClock, PreciseUnbiasedInterruptClock,
    UnbiasedInterruptClock, UnbiasedInterruptTime,
};
use crate::{Clock, Steady, Ticks, Timeline};

/// Longer than the coarsest system clock tick, 15.625 ms, with room to spare.
const ACROSS_A_TICK: Duration = Duration::from_millis(50);
/// Shorter than the finest system clock tick, 0.5 ms, and 500 ticks of 100 ns.
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

/// Twenty windows, each shorter than any system tick: a clock limited to the tick would almost
/// surely stand still in one of them.
fn advances_within_a_tick<C: Clock>(clock: C) {
    for window in 0..20 {
        let before = clock.now();
        spin(WITHIN_A_TICK);
        let after = clock.now();
        assert!(after > before, "window {window}: still at {before:?}");
    }
}

fn advances_across_a_tick<C: Clock>(clock: C) {
    let before = clock.now();
    thread::sleep(ACROSS_A_TICK);
    assert!(clock.now() > before);
}

/// A plain reading is the precise one as of the last tick: never ahead of a precise reading taken
/// after it, and behind one taken before it by at most a tick.
fn plain_lags_precise_by_at_most_a_tick<T: Timeline>(
    plain: impl Clock<Timeline = T>,
    precise: impl Clock<Timeline = T>,
) {
    let before = precise.now();
    let reading = plain.now();
    let after = precise.now();
    assert!(reading <= after, "{reading:?} is ahead of {after:?}");
    let lag = before - reading;
    assert!(
        lag < Ticks::from_duration(Duration::from_millis(20)).unwrap(),
        "{lag:?} behind"
    );
}

/// The precise clock's measure of an interval, nested inside `Instant`'s measure of it.
fn agrees_with_instant<C: Clock>(clock: C) {
    let outer_start = Instant::now();
    let start = clock.now();
    thread::sleep(Duration::from_millis(100));
    let end = clock.now();
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
fn no_clock_goes_backwards() {
    never_goes_backwards(InterruptClock);
    never_goes_backwards(PreciseInterruptClock);
    never_goes_backwards(UnbiasedInterruptClock);
    never_goes_backwards(PreciseUnbiasedInterruptClock);
}

#[test]
fn the_precise_clocks_advance_within_a_fraction_of_a_tick() {
    advances_within_a_tick(PreciseInterruptClock);
    advances_within_a_tick(PreciseUnbiasedInterruptClock);
}

#[test]
fn the_plain_clocks_advance_across_a_tick() {
    advances_across_a_tick(InterruptClock);
    advances_across_a_tick(UnbiasedInterruptClock);
}

#[test]
fn a_plain_reading_is_a_precise_one_as_of_the_last_tick() {
    plain_lags_precise_by_at_most_a_tick(InterruptClock, PreciseInterruptClock);
    plain_lags_precise_by_at_most_a_tick(UnbiasedInterruptClock, PreciseUnbiasedInterruptClock);
}

#[test]
fn an_interval_measures_what_instant_measures() {
    agrees_with_instant(PreciseInterruptClock);
    agrees_with_instant(PreciseUnbiasedInterruptClock);
}

#[test]
fn unbiased_time_is_never_ahead_of_interrupt_time() {
    // The bias is time spent asleep, which is never negative; read afterwards, interrupt time is at
    // least unbiased time. Equal on a machine that has never slept.
    let unbiased = PreciseUnbiasedInterruptClock.now();
    let interrupt = PreciseInterruptClock.now();
    assert!(unbiased.ticks() <= interrupt.ticks());
    let unbiased = UnbiasedInterruptClock.now();
    let interrupt = InterruptClock.now();
    assert!(unbiased.ticks() <= interrupt.ticks());
}

#[test]
fn both_timelines_tick_every_100_nanoseconds() {
    assert_eq!(InterruptTime::ticks_per_second().get(), 10_000_000);
    assert_eq!(UnbiasedInterruptTime::ticks_per_second().get(), 10_000_000);
    assert_eq!(
        Ticks::<InterruptTime>::new(10_000_000).to_duration(),
        Some(Duration::from_secs(1))
    );
}

#[test]
fn a_clock_is_a_zero_sized_value() {
    assert_eq!(size_of::<InterruptClock>(), 0);
    assert_eq!(size_of::<PreciseInterruptClock>(), 0);
    assert_eq!(size_of::<UnbiasedInterruptClock>(), 0);
    assert_eq!(size_of::<PreciseUnbiasedInterruptClock>(), 0);
}
