// Copyright (c) Mike Grier.

//! Tests for the blocking loop's timeout arithmetic.
//!
//! The loop itself is exercised through every shape's `recv_timeout`; what is
//! tested here is the one value that decides whether a bounded call stays
//! bounded, because the failing case takes 49 days to observe from the outside
//! and so can never be a test of the loop.

use std::time::Duration;

use windows_sys::Win32::System::Threading::INFINITE;

use super::{MAX_FINITE_WAIT_MILLIS, MIN_WAIT_MILLIS, wait_millis};

#[test]
fn an_ordinary_duration_is_passed_through_in_milliseconds() {
    assert_eq!(wait_millis(Duration::from_millis(1)), 1);
    assert_eq!(wait_millis(Duration::from_millis(250)), 250);
    assert_eq!(wait_millis(Duration::from_secs(1)), 1_000);
    assert_eq!(wait_millis(Duration::from_secs(60)), 60_000);
}

#[test]
fn a_duration_that_does_not_fit_is_clamped_rather_than_truncated() {
    // Truncating would be the opposite failure: a caller who asked to wait a
    // long time would be told "timed out" almost immediately.
    let fifty_days = Duration::from_secs(50 * 24 * 60 * 60);

    assert_eq!(wait_millis(fifty_days), MAX_FINITE_WAIT_MILLIS);
}

#[test]
fn the_clamp_is_never_the_value_that_means_wait_forever() {
    // **The bug this file exists for.** `INFINITE` is `u32::MAX`, so clamping
    // an oversized duration to `u32::MAX` does not mean "wait a very long
    // time"; it means wait forever, and the loop that was supposed to re-check
    // the deadline never runs again. A bounded call silently becomes unbounded.
    //
    // Every duration too large to fit lands on the clamp, so it is the clamp
    // that has to be checked, not any particular duration.
    assert_ne!(
        MAX_FINITE_WAIT_MILLIS, INFINITE,
        "the clamp is the value that means wait forever"
    );

    for excessive in [
        Duration::from_millis(u64::from(u32::MAX) + 1),
        Duration::from_secs(50 * 24 * 60 * 60),
        Duration::from_secs(u64::from(u32::MAX)),
        Duration::MAX,
    ] {
        assert_ne!(
            wait_millis(excessive),
            INFINITE,
            "a {excessive:?} timeout would have waited forever"
        );
    }
}

#[test]
fn the_largest_duration_that_still_fits_is_not_clamped() {
    // The boundary, from the side that must not move.
    let exact = Duration::from_millis(u64::from(MAX_FINITE_WAIT_MILLIS));

    assert_eq!(wait_millis(exact), MAX_FINITE_WAIT_MILLIS);
    assert_eq!(
        wait_millis(exact + Duration::from_millis(1)),
        MAX_FINITE_WAIT_MILLIS,
        "one millisecond past the boundary must clamp, not wrap to zero"
    );
}

#[test]
fn a_sub_millisecond_remainder_still_sleeps() {
    // The busy-wait. Anything under a millisecond truncates to zero, and a zero
    // wait returns immediately -- so the loop would re-arm and re-wait without
    // sleeping for the last fraction of the budget. Arming clears the doorbell,
    // which is a `ResetEvent` syscall, so the spin is a syscall storm rather
    // than merely a hot loop.
    for tiny in [
        Duration::from_nanos(1),
        Duration::from_micros(1),
        Duration::from_micros(999),
    ] {
        assert_eq!(
            wait_millis(tiny),
            MIN_WAIT_MILLIS,
            "a {tiny:?} remainder would have polled instead of waiting"
        );
    }
}

#[test]
fn no_duration_ever_produces_a_zero_wait() {
    // The property behind the case above, stated over the boundary values
    // rather than over three samples. Zero is a poll; the caller has already
    // returned `Timeout` when nothing remains, so a poll here is never what was
    // wanted.
    for remaining in [
        Duration::ZERO,
        Duration::from_nanos(1),
        Duration::from_micros(500),
        Duration::from_millis(1),
        Duration::from_secs(1),
        Duration::MAX,
    ] {
        assert_ne!(
            wait_millis(remaining),
            0,
            "a {remaining:?} remainder produced a poll rather than a wait"
        );
    }
}
