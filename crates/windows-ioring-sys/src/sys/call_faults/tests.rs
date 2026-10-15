// Copyright (c) Mike Grier
//! Tests for the safe call-fault responder (M37.1, M38.1).
//!
//! # What these establish
//!
//! Each rule in both directions: nothing fails until armed; an armed call fails with exactly the
//! injected code and, counted, only that many times, and only that call; clearing stops it; the
//! thread of each failure is reported; a failure that is not one is refused when armed, leaving
//! the responder as it was; and a stalled submit returns without reaching the kernel until the
//! stall ends, from its first stalled submit.
//!
//! # What they deliberately do not do
//!
//! **Open a ring.** The responder is called directly, with a null ring handle. A call that
//! passes reaches `SubmitIoRing(null)`, which measured -- see `IoRing::refused_by_the_kernel` --
//! answers `ERROR_INVALID_HANDLE` rather than dereferencing it, and that answer is what tells a
//! pass-through from a failure here. Only `submit` is ever passed through; a failing call is
//! checked for every other. What a failed build, a failed completion and a stalled submit do to a
//! real ring is `tests/call_faults.rs`'s.

use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::S_OK;
use windows_sys::core::HRESULT;

use super::{
    Call, CallFaultControl, CallFaults, Responses, SubmitFaultControl, call_faults, submit_faults,
};
use crate::ring::IORING_E_WAIT_TIMEOUT;
use crate::{InjectedFailure, RingCondition};

/// `HRESULT_FROM_WIN32(ERROR_INVALID_HANDLE)`: what the kernel answers a submit on a null ring,
/// so what a call that was not failed returns here.
const PASSED: HRESULT = 0x8007_0006_u32.cast_signed();

/// `HRESULT_FROM_WIN32(ERROR_IO_DEVICE)`, as `InjectedFailure::Win32(1117)` spells it.
const DEVICE: HRESULT = 0x8007_045D_u32.cast_signed();

const EVERY_CALL: [Call; 9] = [
    Call::Submit,
    Call::Pop,
    Call::BuildRead,
    Call::BuildWrite,
    Call::BuildFlush,
    Call::BuildCancel,
    Call::BuildRegisterFiles,
    Call::BuildRegisterBuffers,
    Call::SetCompletionEvent,
];

fn device() -> InjectedFailure {
    InjectedFailure::Win32(1117)
}

/// One submit, answered by `faults` or, if it does not fail, by the kernel on a null ring.
fn submit(faults: &mut CallFaults) -> HRESULT {
    submit_waiting(faults, 0, 0)
}

/// One submit that waits for `wait_operations` for `milliseconds`.
fn submit_waiting(faults: &mut CallFaults, wait_operations: u32, milliseconds: u32) -> HRESULT {
    let mut submitted = 99_u32;
    // SAFETY: a null ring is refused by the kernel, not dereferenced, and the out-pointer is
    // valid.
    unsafe {
        faults.submit(
            null_mut(),
            wait_operations,
            milliseconds,
            &raw mut submitted,
        )
    }
}

/// `call`, made on `faults` with arguments that would be unsound to forward -- so only for a call
/// that is failed -- and the answer.
fn call(faults: &mut CallFaults, call: Call) -> HRESULT {
    // SAFETY: only ever invoked while `call` is armed to fail, so nothing reaches the kernel;
    // the zeroed references are never read.
    unsafe {
        match call {
            Call::Submit => submit(faults),
            Call::Pop => faults.pop(null_mut(), null_mut()),
            Call::BuildRead => faults.build_read(
                null_mut(),
                std::mem::zeroed(),
                std::mem::zeroed(),
                0,
                0,
                0,
                0,
            ),
            Call::BuildWrite => faults.build_write(
                null_mut(),
                std::mem::zeroed(),
                std::mem::zeroed(),
                0,
                0,
                0,
                0,
                0,
            ),
            Call::BuildFlush => {
                faults.build_flush(null_mut(), std::mem::zeroed(), std::mem::zeroed(), 0, 0)
            }
            Call::BuildCancel => faults.build_cancel(null_mut(), std::mem::zeroed(), 0, 0),
            Call::BuildRegisterFiles => faults.build_register_files(null_mut(), 0, null(), 0),
            Call::BuildRegisterBuffers => faults.build_register_buffers(null_mut(), 0, null(), 0),
            Call::SetCompletionEvent => faults.set_completion_event(null_mut(), null_mut()),
        }
    }
}

#[test]
fn nothing_fails_until_armed() {
    let (mut faults, control) = call_faults();
    for _ in 0..3 {
        assert_eq!(submit(&mut faults), PASSED);
    }
    assert!(EVERY_CALL.iter().all(|c| control.failures(*c).is_empty()));
}

#[test]
fn every_call_fails_when_armed_with_the_injected_code() {
    for armed in EVERY_CALL {
        let (mut faults, control) = call_faults();
        control.fail_until_cleared(armed, device());
        assert_eq!(call(&mut faults, armed), DEVICE, "{armed:?}");
        assert_eq!(control.failures(armed).len(), 1, "{armed:?} is recorded");
    }
}

#[test]
fn arming_one_call_fails_no_other() {
    // Submit is the one call that may be passed through, so it is the witness: arming any other
    // call leaves it reaching the kernel, and arming it leaves nothing else recorded.
    for armed in EVERY_CALL.into_iter().filter(|c| *c != Call::Submit) {
        let (mut faults, control) = call_faults();
        control.fail_until_cleared(armed, device());
        assert_eq!(submit(&mut faults), PASSED, "arming {armed:?}");
        assert!(control.failures(Call::Submit).is_empty());
    }
    let (mut faults, control) = call_faults();
    control.fail_until_cleared(Call::Submit, device());
    assert_eq!(submit(&mut faults), DEVICE);
    assert!(
        EVERY_CALL
            .into_iter()
            .filter(|c| *c != Call::Submit)
            .all(|c| control.failures(c).is_empty()),
        "a failed submit is recorded as a submit and nothing else"
    );
}

#[test]
fn armed_until_cleared_fails_every_call_with_the_injected_code() {
    let (mut faults, control) = call_faults();
    control.fail_until_cleared(Call::Submit, device());
    for _ in 0..5 {
        assert_eq!(submit(&mut faults), DEVICE);
    }
    assert_eq!(control.failures(Call::Submit).len(), 5, "each is recorded");
}

#[test]
fn clearing_stops_every_failure_and_keeps_what_was_recorded() {
    let (mut faults, control) = call_faults();
    control.fail_until_cleared(Call::Submit, device());
    control.fail_until_cleared(Call::BuildRead, device());
    assert_eq!(submit(&mut faults), DEVICE);
    control.clear();
    assert_eq!(submit(&mut faults), PASSED, "the kernel answers again");
    assert_eq!(control.failures(Call::Submit).len(), 1, "the record stays");
}

#[test]
fn a_count_fails_exactly_that_many_and_then_passes() {
    let (mut faults, control) = call_faults();
    control.fail_next(Call::Submit, 3, device());
    for _ in 0..3 {
        assert_eq!(submit(&mut faults), DEVICE);
    }
    for _ in 0..2 {
        assert_eq!(submit(&mut faults), PASSED, "the fourth onward passes");
    }
    assert_eq!(control.failures(Call::Submit).len(), 3);
}

#[test]
fn a_count_of_one_fails_once() {
    // The boundary a decrement-then-compare gets wrong.
    let (mut faults, control) = call_faults();
    control.fail_next(Call::Submit, 1, device());
    assert_eq!(submit(&mut faults), DEVICE);
    assert_eq!(submit(&mut faults), PASSED);
}

#[test]
fn a_count_of_zero_clears_that_call() {
    let (mut faults, control) = call_faults();
    control.fail_until_cleared(Call::Submit, device());
    control.fail_next(Call::Submit, 0, device());
    assert_eq!(submit(&mut faults), PASSED, "zero failures means none");
}

#[test]
fn arming_a_call_again_replaces_what_was_armed_for_it() {
    let (mut faults, control) = call_faults();
    control.fail_until_cleared(Call::Submit, device());
    control.fail_next(
        Call::Submit,
        1,
        InjectedFailure::Ring(RingCondition::SubmissionQueueFull),
    );
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
    // The case D-85 exists for: a call answered on a thread other than the one that armed it,
    // which a test can only show if it is told.
    let (faults, control) = call_faults();
    control.fail_until_cleared(Call::Submit, device());
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
        control.failures(Call::Submit),
        [there],
        "named as the thread that made it"
    );
}

#[test]
fn the_failures_are_in_the_order_they_happened() {
    let (mut faults, control) = call_faults();
    control.fail_until_cleared(Call::Submit, device());
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
    assert_eq!(control.failures(Call::Submit), [here, there, here]);
}

#[test]
fn a_failure_that_is_not_one_is_refused_when_armed_and_changes_nothing() {
    let (mut faults, control) = call_faults();
    control.fail_next(Call::Submit, 1, device());

    for refused in [
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            control.fail_until_cleared(Call::Submit, InjectedFailure::Hresult(0));
        })),
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            control.fail_next(Call::Submit, 2, InjectedFailure::Hresult(0));
        })),
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            control.fail_next_completions(2, InjectedFailure::Hresult(0));
        })),
    ] {
        assert!(refused.is_err(), "a success is not an injectable failure");
    }

    // The responder is as it was: still failing once, as armed before.
    assert_eq!(submit(&mut faults), DEVICE);
    assert_eq!(submit(&mut faults), PASSED);
}

#[test]
fn the_control_is_a_handle_to_the_one_responder() {
    fn shared<T: Send + Sync + Clone>() {}
    shared::<CallFaultControl>();
    shared::<SubmitFaultControl>();
    fn sendable<T: Send>() {}
    sendable::<CallFaults>();

    let (mut faults, control) = call_faults();
    let other = control.clone();
    other.fail_next(Call::Submit, 1, device());
    assert_eq!(
        submit(&mut faults),
        DEVICE,
        "a clone arms the same responder"
    );
    assert_eq!(control.failures(Call::Submit), other.failures(Call::Submit));
}

#[test]
fn the_narrow_control_arms_submits_over_the_same_responder() {
    let (mut faults, control) = submit_faults();
    control.fail_next(2, device());
    assert_eq!(submit(&mut faults), DEVICE);
    assert_eq!(submit(&mut faults), DEVICE);
    assert_eq!(submit(&mut faults), PASSED);
    control.fail_until_cleared(device());
    assert_eq!(submit(&mut faults), DEVICE);
    control.clear();
    assert_eq!(submit(&mut faults), PASSED);
    assert_eq!(control.failures().len(), 3, "and reports only submits");
}

#[test]
fn a_stalled_submit_returns_without_reaching_the_kernel() {
    let (mut faults, control) = call_faults();
    control.stall_submits(Duration::from_millis(300));
    // The kernel would answer a null ring `PASSED`; success is the stall's, with nothing
    // submitted written to the out-parameter.
    let mut submitted = 99_u32;
    // SAFETY: as `submit_waiting`.
    let answer = unsafe { faults.submit(null_mut(), 0, 0, &raw mut submitted) };
    assert_eq!(answer, S_OK);
    assert_eq!(submitted, 0, "nothing was submitted");
}

#[test]
fn a_stalled_submit_that_waits_expires_its_wait_after_the_wait() {
    let (mut faults, control) = call_faults();
    control.stall_submits(Duration::from_secs(5));
    let started = Instant::now();
    assert_eq!(submit_waiting(&mut faults, 1, 60), IORING_E_WAIT_TIMEOUT);
    assert!(
        started.elapsed() >= Duration::from_millis(50),
        "it waited for the wait it was given: {:?}",
        started.elapsed()
    );
}

#[test]
fn a_stalled_wait_is_cut_to_what_is_left_of_the_stall() {
    let (mut faults, control) = call_faults();
    control.stall_submits(Duration::from_millis(80));
    assert_eq!(submit(&mut faults), S_OK, "the stall begins");
    let started = Instant::now();
    assert_eq!(
        submit_waiting(&mut faults, 1, 60_000),
        IORING_E_WAIT_TIMEOUT
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "a minute's wait was cut to the stall's end: {:?}",
        started.elapsed()
    );
}

#[test]
fn submits_reach_the_kernel_again_once_the_stall_ends() {
    let (mut faults, control) = call_faults();
    control.stall_submits(Duration::from_millis(60));
    assert_eq!(submit(&mut faults), S_OK, "stalled");
    std::thread::sleep(Duration::from_millis(90));
    assert_eq!(submit(&mut faults), PASSED, "the stall ended");
    assert_eq!(submit(&mut faults), PASSED, "and stays ended");
}

#[test]
fn the_stall_runs_from_its_first_stalled_submit_not_from_arming() {
    let (mut faults, control) = call_faults();
    control.stall_submits(Duration::from_millis(120));
    // Longer than the stall, but before any submit: it has not begun.
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(submit(&mut faults), S_OK, "still stalled: it begins now");
    std::thread::sleep(Duration::from_millis(160));
    assert_eq!(submit(&mut faults), PASSED);
}

#[test]
fn clearing_ends_a_stall() {
    let (mut faults, control) = call_faults();
    control.stall_submits(Duration::from_secs(60));
    assert_eq!(submit(&mut faults), S_OK);
    control.clear();
    assert_eq!(submit(&mut faults), PASSED);
}

#[test]
fn a_stall_of_zero_clears() {
    let (mut faults, control) = call_faults();
    control.stall_submits(Duration::from_secs(60));
    control.stall_submits(Duration::ZERO);
    assert_eq!(submit(&mut faults), PASSED);
}

#[test]
fn a_failed_submit_is_answered_before_a_stalled_one() {
    let (mut faults, control) = call_faults();
    control.stall_submits(Duration::from_secs(60));
    control.fail_next(Call::Submit, 1, device());
    assert_eq!(submit(&mut faults), DEVICE, "the failure is the answer");
    assert_eq!(submit(&mut faults), S_OK, "and the stall is still there");
}
