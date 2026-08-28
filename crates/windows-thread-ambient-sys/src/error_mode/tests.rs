// Copyright (c) Mike Grier.

//! Tests for the thread error mode aspect.
//!
//! Every test restores the thread it ran on, because the test harness reuses
//! threads: a leaked error mode would silently change the conditions of whatever
//! test ran next on that thread, which is exactly the contamination this aspect
//! exists to prevent.

use windows_sys::Win32::System::Diagnostics::Debug::{
    GetThreadErrorMode, SEM_NOALIGNMENTFAULTEXCEPT,
};

use super::{ThreadErrorMode, UnsupportedBits};

/// The live thread error mode, read straight from Win32.
fn live() -> u32 {
    // SAFETY: the call takes no arguments and has no preconditions.
    unsafe { GetThreadErrorMode() }
}

#[test]
fn none_is_empty_and_zero() {
    assert!(ThreadErrorMode::NONE.is_empty());
    assert_eq!(ThreadErrorMode::NONE.bits(), 0);
}

#[test]
fn union_accumulates_and_contains_reports_membership() {
    let both = ThreadErrorMode::FAIL_CRITICAL_ERRORS.union(ThreadErrorMode::NO_OPEN_FILE_ERROR_BOX);
    assert!(both.contains(ThreadErrorMode::FAIL_CRITICAL_ERRORS));
    assert!(both.contains(ThreadErrorMode::NO_OPEN_FILE_ERROR_BOX));
    assert!(!both.contains(ThreadErrorMode::NO_GP_FAULT_ERROR_BOX));
    assert!(!both.is_empty());
}

#[test]
fn union_is_idempotent() {
    let one = ThreadErrorMode::FAIL_CRITICAL_ERRORS;
    assert_eq!(one.union(one), one);
}

#[test]
fn every_accepted_bit_round_trips_through_from_bits() {
    for mode in [
        ThreadErrorMode::NONE,
        ThreadErrorMode::FAIL_CRITICAL_ERRORS,
        ThreadErrorMode::NO_GP_FAULT_ERROR_BOX,
        ThreadErrorMode::NO_OPEN_FILE_ERROR_BOX,
    ] {
        assert_eq!(ThreadErrorMode::from_bits(mode.bits()), Ok(mode));
    }
}

#[test]
fn all_three_accepted_bits_together_are_representable() {
    let all = ThreadErrorMode::FAIL_CRITICAL_ERRORS
        .union(ThreadErrorMode::NO_GP_FAULT_ERROR_BOX)
        .union(ThreadErrorMode::NO_OPEN_FILE_ERROR_BOX);
    assert_eq!(ThreadErrorMode::from_bits(all.bits()), Ok(all));
}

#[test]
fn the_alignment_bit_is_not_representable() {
    // Measured: SetThreadErrorMode rejects this bit, and an invalid bit fails
    // the whole call. So the type refuses it rather than letting a caller lose
    // the accompanying valid bits.
    let error = ThreadErrorMode::from_bits(SEM_NOALIGNMENTFAULTEXCEPT)
        .expect_err("the alignment bit is not settable per thread");
    assert_eq!(error.bits(), SEM_NOALIGNMENTFAULTEXCEPT);
}

#[test]
fn an_unsupported_bit_is_rejected_even_beside_valid_ones() {
    // The case the rejection exists for: masking here would install the valid
    // bits in a caller's mental model while Windows installed nothing.
    let mixed = ThreadErrorMode::FAIL_CRITICAL_ERRORS.bits() | SEM_NOALIGNMENTFAULTEXCEPT;
    let error = ThreadErrorMode::from_bits(mixed).expect_err("mixed values are rejected");
    assert_eq!(
        error.bits(),
        SEM_NOALIGNMENTFAULTEXCEPT,
        "only the offending bits should be reported"
    );
}

#[test]
fn an_arbitrary_unknown_bit_is_rejected() {
    let error = ThreadErrorMode::from_bits(0x0100).expect_err("0x0100 is not a thread error mode");
    assert_eq!(error.bits(), 0x0100);
}

#[test]
fn unsupported_bits_names_the_offending_value() {
    let error = UnsupportedBits {
        bits: SEM_NOALIGNMENTFAULTEXCEPT,
    };
    assert!(error.to_string().contains("0x0004"));
}

#[test]
fn capture_reports_what_the_thread_actually_has() {
    let captured = ThreadErrorMode::capture().expect("a thread mode is always representable");
    assert_eq!(captured.bits(), live());
}

#[test]
fn apply_installs_the_mode_and_release_restores_it() {
    let entry = live();
    let guard = ThreadErrorMode::NO_OPEN_FILE_ERROR_BOX
        .apply()
        .expect("a supported mode installs");
    assert_eq!(live(), ThreadErrorMode::NO_OPEN_FILE_ERROR_BOX.bits());
    guard.release().expect("restoring succeeds");
    assert_eq!(live(), entry, "the entry mode was not restored");
}

#[test]
fn the_guard_reports_the_previous_mode() {
    let entry = live();
    let guard = ThreadErrorMode::FAIL_CRITICAL_ERRORS
        .apply()
        .expect("install");
    let previous = guard
        .previous()
        .expect("a real thread mode is representable");
    assert_eq!(previous.bits(), entry);
    guard.release().expect("restore");
}

#[test]
fn dropping_the_guard_also_restores() {
    let entry = live();
    {
        let _guard = ThreadErrorMode::NO_GP_FAULT_ERROR_BOX
            .apply()
            .expect("install");
        assert_eq!(live(), ThreadErrorMode::NO_GP_FAULT_ERROR_BOX.bits());
    }
    assert_eq!(live(), entry, "drop did not restore the entry mode");
}

#[test]
fn nesting_restores_in_exact_reverse() {
    // The property the composite depends on: guards released innermost-first
    // return the thread through each intermediate state to where it started.
    let entry = live();
    let outer = ThreadErrorMode::FAIL_CRITICAL_ERRORS
        .apply()
        .expect("install outer");
    let outer_installed = live();
    let inner = ThreadErrorMode::NO_OPEN_FILE_ERROR_BOX
        .apply()
        .expect("install inner");
    assert_eq!(live(), ThreadErrorMode::NO_OPEN_FILE_ERROR_BOX.bits());

    inner.release().expect("release inner");
    assert_eq!(live(), outer_installed, "inner release skipped a state");
    outer.release().expect("release outer");
    assert_eq!(live(), entry);
}

#[test]
fn applying_the_mode_a_thread_already_has_is_not_special() {
    let entry = ThreadErrorMode::capture().expect("representable");
    let guard = entry.apply().expect("install");
    assert_eq!(live(), entry.bits());
    guard.release().expect("restore");
    assert_eq!(live(), entry.bits());
}

#[test]
fn applying_none_clears_every_bit_and_restores() {
    let entry = live();
    let guard = ThreadErrorMode::FAIL_CRITICAL_ERRORS
        .apply()
        .expect("install something non-empty first");
    let cleared = ThreadErrorMode::NONE.apply().expect("install none");
    assert_eq!(live(), 0);
    cleared.release().expect("restore none");
    outer_restore(guard, entry);
}

/// Release `guard` and assert the thread returned to `entry`.
fn outer_restore(guard: super::ErrorModeGuard, entry: u32) {
    guard.release().expect("restore");
    assert_eq!(live(), entry);
}

#[test]
fn a_mode_survives_being_moved_to_another_thread() {
    // The aspect is a plain value; carrying it across a thread boundary is the
    // entire point of the crate, so it is asserted rather than assumed.
    let mode = ThreadErrorMode::FAIL_CRITICAL_ERRORS.union(ThreadErrorMode::NO_OPEN_FILE_ERROR_BOX);
    let observed = std::thread::spawn(move || {
        let entry = live();
        let guard = mode.apply().expect("install on the worker");
        let installed = live();
        guard.release().expect("restore on the worker");
        (entry, installed, live())
    })
    .join()
    .expect("the worker did not panic");

    assert_eq!(observed.1, mode.bits(), "the mode did not arrive intact");
    assert_eq!(observed.0, observed.2, "the worker was left contaminated");
}

#[test]
fn a_worker_thread_starts_with_no_bits_set() {
    // Records the platform fact the aspect exists for: a fresh thread has the
    // critical-error handler enabled, so a hard error there raises a dialog.
    let worker = std::thread::spawn(live).join().expect("no panic");
    assert_eq!(worker, 0);
}
