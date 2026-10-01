// Copyright (c) Mike Grier
//! Unit tests for [`RearmSuppression`].
//!
//! These pin the count's arithmetic and its poison behaviour. That each type's
//! arming path actually consults it is pinned by those types' own tests --
//! `rearming_during_teardown_is_suppressed` exists for both the wait and the
//! timer, and both go red if the suppression stops working.

use super::RearmSuppression;

#[test]
fn nothing_is_suppressed_initially() {
    assert_eq!(*RearmSuppression::new().lock(), 0);
}

#[test]
fn suppressing_raises_the_count_before_the_disarm_runs() {
    // The order matters: the closure is the native disarm, and it must see a
    // count that already includes its own suppression, because that is what a
    // concurrent arming would see too.
    let suppression = RearmSuppression::new();
    let mut seen = None;
    suppression.suppress_and(|count| seen = Some(count));
    assert_eq!(seen, Some(1));
    assert_eq!(*suppression.lock(), 1);
}

#[test]
fn releasing_lowers_the_count() {
    let suppression = RearmSuppression::new();
    suppression.suppress_and(|_| {});
    suppression.release();
    assert_eq!(*suppression.lock(), 0);
}

#[test]
fn two_suppressions_both_have_to_be_released() {
    // The reason this is a count and not a flag: `stop_and_drain` raises and
    // lowers it, so one of them finishing must not clear a suppression another
    // still needs.
    let suppression = RearmSuppression::new();
    suppression.suppress_and(|count| assert_eq!(count, 1));
    suppression.suppress_and(|count| assert_eq!(count, 2));
    suppression.release();
    assert_eq!(
        *suppression.lock(),
        1,
        "one release must not discharge the other caller's suppression"
    );
    suppression.release();
    assert_eq!(*suppression.lock(), 0);
}

#[test]
fn releasing_more_than_was_suppressed_saturates_at_zero() {
    // `Drop` raises the count and never releases it, so the counts are not
    // required to balance. An unmatched release must not wrap to u32::MAX and
    // suppress arming forever.
    let suppression = RearmSuppression::new();
    suppression.release();
    assert_eq!(*suppression.lock(), 0);
}

#[test]
fn the_count_saturates_rather_than_wrapping_at_the_top() {
    // The mirror of the test above: wrapping to zero would silently *lift* a
    // suppression that is still needed, which is the direction that reopens the
    // teardown hazard.
    let suppression = RearmSuppression::new();
    *suppression.lock() = u32::MAX;
    suppression.suppress_and(|count| assert_eq!(count, u32::MAX));
    assert_eq!(*suppression.lock(), u32::MAX);
}

#[test]
fn a_poisoned_lock_is_still_usable() {
    // A callback panicking while arming poisons this. The count is a plain
    // integer with no half-updated state, and refusing to tear down because of
    // an earlier panic would turn a reported fault into a hang.
    let suppression = RearmSuppression::new();
    suppression.suppress_and(|_| {});
    let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = suppression.lock();
        panic!("poison the lock");
    }));
    assert!(poisoned.is_err(), "the closure above must have panicked");
    assert_eq!(
        *suppression.lock(),
        1,
        "the count survives, and the lock is still usable"
    );
    suppression.release();
    assert_eq!(*suppression.lock(), 0);
}
