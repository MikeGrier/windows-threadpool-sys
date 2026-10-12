// Copyright (c) Mike Grier
//! Tests for the safe submit-fault responder (M37.1).
//!
//! # What these establish
//!
//! Each rule in both directions: nothing fails until armed; an armed responder
//! fails with exactly the injected code and, counted, only that many times;
//! clearing stops it; the thread of each failure is reported; and a failure
//! that is not one is refused when armed, leaving the responder as it was.
//!
//! # What they deliberately do not do
//!
//! **Open a ring.** The responder is called directly, with a null ring handle.
//! A call that passes reaches `SubmitIoRing(null)`, which measured -- see
//! `IoRing::refused_by_the_kernel` -- answers `ERROR_INVALID_HANDLE` rather than
//! dereferencing it, and that answer is what tells a pass-through from a
//! failure here. That a failed submit leaves a real ring's entries queued is
//! `tests/responder_per_ring.rs`'s.

use std::panic::{self, AssertUnwindSafe};

use windows_sys::core::HRESULT;

use super::{Responses, SubmitFaultControl, SubmitFaults, submit_faults};
use crate::{InjectedFailure, RingCondition};

/// `HRESULT_FROM_WIN32(ERROR_INVALID_HANDLE)`: what the kernel answers a submit
/// on a null ring, so what a call that was not failed returns here.
const PASSED: HRESULT = 0x8007_0006_u32.cast_signed();

/// `HRESULT_FROM_WIN32(ERROR_IO_DEVICE)`, as `InjectedFailure::Win32(1117)`
/// spells it.
const DEVICE: HRESULT = 0x8007_045D_u32.cast_signed();

fn device() -> InjectedFailure {
    InjectedFailure::Win32(1117)
}

/// One submit, answered by `faults` or, if it does not fail, by the kernel on
/// a null ring.
fn submit(faults: &mut SubmitFaults) -> HRESULT {
    let mut submitted = 0_u32;
    // SAFETY: a null ring is refused by the kernel, not dereferenced, and the
    // out-pointer is valid.
    unsafe { faults.submit(std::ptr::null_mut(), 0, 0, &raw mut submitted) }
}

#[test]
fn nothing_fails_until_armed() {
    let (mut faults, control) = submit_faults();
    for _ in 0..3 {
        assert_eq!(submit(&mut faults), PASSED);
    }
    assert!(control.failures().is_empty(), "so nothing was recorded");
}

#[test]
fn armed_until_cleared_fails_every_submit_with_the_injected_code() {
    let (mut faults, control) = submit_faults();
    control.fail_until_cleared(device());
    for _ in 0..5 {
        assert_eq!(submit(&mut faults), DEVICE);
    }
    assert_eq!(control.failures().len(), 5, "each failure is recorded");
}

#[test]
fn clearing_stops_the_failures_and_keeps_what_was_recorded() {
    let (mut faults, control) = submit_faults();
    control.fail_until_cleared(device());
    assert_eq!(submit(&mut faults), DEVICE);
    control.clear();
    assert_eq!(submit(&mut faults), PASSED, "the kernel answers again");
    assert_eq!(control.failures().len(), 1, "the record is not cleared");
}

#[test]
fn a_count_fails_exactly_that_many_and_then_passes() {
    let (mut faults, control) = submit_faults();
    control.fail_next(3, device());
    for _ in 0..3 {
        assert_eq!(submit(&mut faults), DEVICE);
    }
    for _ in 0..2 {
        assert_eq!(submit(&mut faults), PASSED, "the fourth onward passes");
    }
    assert_eq!(control.failures().len(), 3);
}

#[test]
fn a_count_of_one_fails_once() {
    // The boundary a decrement-then-compare gets wrong.
    let (mut faults, control) = submit_faults();
    control.fail_next(1, device());
    assert_eq!(submit(&mut faults), DEVICE);
    assert_eq!(submit(&mut faults), PASSED);
}

#[test]
fn a_count_of_zero_clears() {
    let (mut faults, control) = submit_faults();
    control.fail_until_cleared(device());
    control.fail_next(0, device());
    assert_eq!(submit(&mut faults), PASSED, "zero failures means none");
}

#[test]
fn arming_again_replaces_what_was_armed() {
    let (mut faults, control) = submit_faults();
    control.fail_until_cleared(device());
    control.fail_next(1, InjectedFailure::Ring(RingCondition::SubmissionQueueFull));
    assert_eq!(
        submit(&mut faults),
        RingCondition::SubmissionQueueFull.code(),
        "the newer arming answers, in its own spelling"
    );
    assert_eq!(
        submit(&mut faults),
        PASSED,
        "and replaced the unbounded one"
    );
}

#[test]
fn the_thread_of_each_failure_is_reported() {
    // The case D-85 exists for: a submit answered on a thread other than the
    // one that armed it, which a test can only show if it is told.
    let (faults, control) = submit_faults();
    control.fail_until_cleared(device());
    let here = std::thread::current().id();
    let there = std::thread::spawn(move || {
        let mut faults = faults;
        assert_eq!(submit(&mut faults), DEVICE);
        std::thread::current().id()
    })
    .join()
    .expect("the other thread does not panic");

    assert_ne!(here, there);
    assert_eq!(
        control.failures(),
        [there],
        "named as the thread that made it"
    );
}

#[test]
fn the_failures_are_in_the_order_they_happened() {
    let (mut faults, control) = submit_faults();
    control.fail_until_cleared(device());
    let here = std::thread::current().id();
    assert_eq!(submit(&mut faults), DEVICE);
    let there = std::thread::scope(|scope| {
        scope
            .spawn(|| {
                assert_eq!(submit(&mut faults), DEVICE);
                std::thread::current().id()
            })
            .join()
            .expect("the other thread does not panic")
    });
    assert_eq!(submit(&mut faults), DEVICE);
    assert_eq!(control.failures(), [here, there, here]);
}

#[test]
fn a_failure_that_is_not_one_is_refused_when_armed_and_changes_nothing() {
    let (mut faults, control) = submit_faults();
    control.fail_next(1, device());

    let refused = panic::catch_unwind(AssertUnwindSafe(|| {
        control.fail_until_cleared(InjectedFailure::Hresult(0));
    }));
    assert!(refused.is_err(), "a success is not an injectable failure");

    // The responder is as it was: still failing once, as armed before.
    assert_eq!(submit(&mut faults), DEVICE);
    assert_eq!(submit(&mut faults), PASSED);
}

#[test]
fn the_control_is_a_handle_to_the_one_responder() {
    fn shared<T: Send + Sync + Clone>() {}
    shared::<SubmitFaultControl>();

    let (mut faults, control) = submit_faults();
    let other = control.clone();
    other.fail_next(1, device());
    assert_eq!(
        submit(&mut faults),
        DEVICE,
        "a clone arms the same responder"
    );
    assert_eq!(control.failures(), other.failures());

    // And the responder itself can move to the thread registering it.
    fn sendable<T: Send>() {}
    sendable::<SubmitFaults>();
}
