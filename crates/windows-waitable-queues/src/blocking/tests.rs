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

// The `Parked` protocol itself, across every shape that implements it.
//
// # Why this is here and not in each shape's suite
//
// `Parked` is what `recv` and `recv_timeout` are written against, and its four
// methods are a contract each shape restates. A mutation run found the whole
// group unguarded: `finish` could return `None` and `arm` could return
// `Ok(true)` on all three shapes with the suite green.
//
// Neither is cosmetic. `finish` is the last take before the end of a stream is
// reported, so a `None` silently discards an item that was successfully sent.
// `arm` reports whether parking is safe, so an unconditional `true` blesses a
// wait over a queue that already has an item -- which is a lost wakeup, the one
// ordering bug this crate has actually had (D-15).
//
// The methods are exercised **through the trait**, because that is the surface
// the loop uses. Calling the inherent method instead is what left these alive:
// it shadows the trait one, so a broken forwarder is never reached.

use super::Parked;
use crate::{reserving_mpsc, slotwise_mpsc, spsc};

/// `finish` must hand back an item that arrived before disconnection was seen.
///
/// Called directly rather than by scheduling the race it guards. That is the
/// stated reason it exists as a named step: the window between a receive's
/// first `pop` and its disconnection check cannot be hit reliably from a test,
/// so the step is reachable on its own instead.
fn finish_returns_the_owed_item<C>(consumer: &C)
where
    C: Parked<Item = u32>,
{
    assert_eq!(
        Parked::finish(consumer),
        Some(1),
        "an item pushed before the producer went is still owed to the consumer"
    );
    assert_eq!(
        Parked::finish(consumer),
        None,
        "and once taken it is gone, so the stream really has ended"
    );
}

/// `arm` must refuse to bless a wait while an item is sitting there.
fn arm_refuses_to_park_over_an_item<C>(consumer: &C, has_item: bool)
where
    C: Parked<Item = u32>,
{
    let safe = Parked::arm(consumer).expect("arming must succeed");
    if has_item {
        assert!(
            !safe,
            "parking over a queued item is a wait nothing will wake"
        );
    } else {
        assert!(safe, "an empty queue is safe to park on");
    }
}

#[test]
fn every_shape_hands_back_the_last_item_through_parked_finish() {
    let (spsc_tx, spsc_rx) = spsc::bounded::<u32>(4).expect("4 is valid");
    let (slot_tx, slot_rx) = slotwise_mpsc::bounded::<u32>(4).expect("4 is valid");
    let (res_tx, res_rx) = reserving_mpsc::bounded::<u32>(4).expect("4 is valid");

    // Push, then drop the producer: the item is owed even though the stream has
    // ended, which is exactly the state `finish` exists to resolve.
    spsc_tx.push(1).expect("there is room");
    slot_tx.push(1).expect("there is room");
    res_tx.push(1).expect("there is room");
    drop(spsc_tx);
    drop(slot_tx);
    drop(res_tx);

    finish_returns_the_owed_item(&spsc_rx);
    finish_returns_the_owed_item(&slot_rx);
    finish_returns_the_owed_item(&res_rx);
}

#[test]
fn every_shape_refuses_to_park_over_an_item_through_parked_arm() {
    let (spsc_tx, spsc_rx) = spsc::bounded::<u32>(4).expect("4 is valid");
    let (slot_tx, slot_rx) = slotwise_mpsc::bounded::<u32>(4).expect("4 is valid");
    let (res_tx, res_rx) = reserving_mpsc::bounded::<u32>(4).expect("4 is valid");

    // Empty first, so the `true` answer is shown to be a real reading rather
    // than the only answer this method ever gives.
    arm_refuses_to_park_over_an_item(&spsc_rx, false);
    arm_refuses_to_park_over_an_item(&slot_rx, false);
    arm_refuses_to_park_over_an_item(&res_rx, false);

    spsc_tx.push(1).expect("there is room");
    slot_tx.push(1).expect("there is room");
    res_tx.push(1).expect("there is room");

    arm_refuses_to_park_over_an_item(&spsc_rx, true);
    arm_refuses_to_park_over_an_item(&slot_rx, true);
    arm_refuses_to_park_over_an_item(&res_rx, true);
}

#[test]
fn every_shape_reports_disconnection_and_pops_through_parked() {
    // The other two methods of the same contract, so the trait is covered as a
    // whole rather than only where mutants happened to survive.
    fn pop_and_disconnection<C: Parked<Item = u32>>(
        consumer: &C,
        expect_item: Option<u32>,
    ) -> bool {
        assert_eq!(Parked::pop(consumer), expect_item);
        Parked::is_disconnected(consumer)
    }

    let (spsc_tx, spsc_rx) = spsc::bounded::<u32>(4).expect("4 is valid");
    let (slot_tx, slot_rx) = slotwise_mpsc::bounded::<u32>(4).expect("4 is valid");
    let (res_tx, res_rx) = reserving_mpsc::bounded::<u32>(4).expect("4 is valid");

    spsc_tx.push(5).expect("there is room");
    slot_tx.push(5).expect("there is room");
    res_tx.push(5).expect("there is room");

    assert!(!pop_and_disconnection(&spsc_rx, Some(5)));
    assert!(!pop_and_disconnection(&slot_rx, Some(5)));
    assert!(!pop_and_disconnection(&res_rx, Some(5)));

    drop(spsc_tx);
    drop(slot_tx);
    drop(res_tx);

    assert!(pop_and_disconnection(&spsc_rx, None));
    assert!(pop_and_disconnection(&slot_rx, None));
    assert!(pop_and_disconnection(&res_rx, None));
}
