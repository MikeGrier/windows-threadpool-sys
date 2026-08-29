// Copyright (c) Mike Grier.

//! Tests for composite capture.

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_NO_TOKEN, HANDLE};
use windows_sys::Win32::Security::TOKEN_QUERY;
use windows_sys::Win32::System::Threading::{GetCurrentThread, OpenThreadToken};

use super::AmbientState;
use crate::capture_set::{CapturableAspect, CaptureSet};
use crate::declared::MemoryPriority;
use crate::error_mode::ThreadErrorMode;
use crate::{Captured, Declared};

/// Whether the calling thread currently carries an impersonation token.
fn thread_has_token() -> bool {
    let mut handle: HANDLE = std::ptr::null_mut();
    // SAFETY: `handle` is a valid writable destination and the pseudo-handle
    // needs no cleanup.
    let ok = unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &mut handle) };
    if ok == 0 {
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(ERROR_NO_TOKEN as i32)
        );
        return false;
    }
    // SAFETY: the call above produced this handle and nothing else owns it.
    unsafe { CloseHandle(handle) };
    true
}

#[test]
fn capturing_nothing_captures_nothing() {
    let state = AmbientState::capture(CaptureSet::NONE).expect("capture");
    assert_eq!(state.captured_set(), CaptureSet::NONE);
    assert!(matches!(state.impersonation(), Captured::NotCaptured));
    assert_eq!(*state.error_mode(), Captured::NotCaptured);
    assert!(matches!(state.transaction(), Captured::NotCaptured));
}

#[test]
fn the_captured_set_reports_exactly_what_was_asked_for() {
    for set in [
        CaptureSet::NONE,
        CaptureSet::IMPERSONATION,
        CaptureSet::ERROR_MODE,
        CaptureSet::TRANSACTION,
        CaptureSet::DEFAULT,
        CaptureSet::ALL,
    ] {
        let state = AmbientState::capture(set).expect("capture");
        assert_eq!(
            state.captured_set(),
            set,
            "the derived set disagreed with the requested one"
        );
    }
}

#[test]
fn an_uncaptured_aspect_is_not_captured_rather_than_absent() {
    // The distinction the whole three-state shape exists for, at composite
    // level: leaving an aspect out is not the same as capturing it and finding
    // nothing, and the composite must not blur them.
    let state = AmbientState::capture(CaptureSet::ERROR_MODE).expect("capture");
    assert!(matches!(state.impersonation(), Captured::NotCaptured));
    assert!(
        !state.impersonation().was_captured(),
        "an omitted aspect must not report itself as captured"
    );
    assert!(state.error_mode().was_captured());
}

#[test]
fn a_captured_error_mode_matches_the_calling_thread() {
    let live = ThreadErrorMode::capture().expect("representable");
    let state = AmbientState::capture(CaptureSet::ERROR_MODE).expect("capture");
    assert_eq!(*state.error_mode(), Captured::Present(live));
}

#[test]
fn a_captured_impersonation_context_is_present_even_without_a_thread_token() {
    // The dependency snapshots the process identity rather than reporting
    // absence, so `Absent` is unreachable here; asserted so a change in that
    // contract surfaces at the composite too.
    assert!(!thread_has_token(), "precondition: no thread token");
    let state = AmbientState::capture(CaptureSet::IMPERSONATION).expect("capture");
    assert!(matches!(state.impersonation(), Captured::Present(_)));
}

#[test]
fn a_transaction_is_captured_as_absent_on_an_untransacted_thread() {
    // Absent, not NotCaptured: the caller asked, and the answer was none.
    let state = AmbientState::capture(CaptureSet::TRANSACTION).expect("capture");
    assert!(matches!(state.transaction(), Captured::Absent));
    assert!(state.transaction().was_captured());
}

#[test]
fn the_default_set_captures_impersonation_and_error_mode_only() {
    let state = AmbientState::capture(CaptureSet::DEFAULT).expect("capture");
    assert!(state.impersonation().was_captured());
    assert!(state.error_mode().was_captured());
    assert!(
        !state.transaction().was_captured(),
        "the default must not quietly capture a transaction"
    );
}

#[test]
fn capturing_all_captures_every_listed_aspect() {
    let state = AmbientState::capture(CaptureSet::ALL).expect("capture");
    for aspect in CapturableAspect::EVERY {
        assert!(
            state.captured_set().contains(aspect.as_set()),
            "{aspect} was not captured by ALL"
        );
    }
}

#[test]
fn declared_aspects_are_absent_until_attached() {
    let state = AmbientState::capture(CaptureSet::NONE).expect("capture");
    assert_eq!(*state.declared(), Declared::none());
    assert!(state.declared().is_empty());
}

#[test]
fn declared_aspects_are_attached_not_captured() {
    let declared = Declared::none().with_memory_priority(MemoryPriority::Low);
    let state = AmbientState::capture(CaptureSet::NONE)
        .expect("capture")
        .with_declared(declared);

    assert_eq!(*state.declared(), declared);
    assert_eq!(
        state.captured_set(),
        CaptureSet::NONE,
        "declaring an aspect must not make the state look captured"
    );
}

#[test]
fn attaching_declared_aspects_replaces_rather_than_merges() {
    let first = Declared::none().with_memory_priority(MemoryPriority::Low);
    let second = Declared::none().with_memory_priority(MemoryPriority::Medium);
    let state = AmbientState::capture(CaptureSet::NONE)
        .expect("capture")
        .with_declared(first)
        .with_declared(second);
    assert_eq!(*state.declared(), second);
}

#[test]
fn capture_happens_on_the_calling_thread_not_a_later_one() {
    // Capture is an admission-time act: the value a worker receives is the one
    // read on the submitting thread, not whatever the worker happens to have.
    let submitter = ThreadErrorMode::capture().expect("representable");
    let guard = ThreadErrorMode::NO_OPEN_FILE_ERROR_BOX
        .apply()
        .expect("install a distinctive mode");
    let state = AmbientState::capture(CaptureSet::ERROR_MODE).expect("capture");
    guard.release().expect("restore");

    assert_eq!(
        *state.error_mode(),
        Captured::Present(ThreadErrorMode::NO_OPEN_FILE_ERROR_BOX),
        "capture read the thread's state at the wrong moment"
    );
    assert_eq!(
        ThreadErrorMode::capture().expect("representable"),
        submitter,
        "capture disturbed the calling thread"
    );
}

#[test]
fn a_captured_state_can_be_moved_to_another_thread() {
    // The entire point of the composite. Asserted rather than assumed, since a
    // single non-Send field would break it and nothing else would notice.
    let state = AmbientState::capture(CaptureSet::ALL).expect("capture");
    let observed = std::thread::spawn(move || state.captured_set())
        .join()
        .expect("the worker did not panic");
    assert_eq!(observed, CaptureSet::ALL);
}

#[test]
fn an_ambient_state_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<AmbientState>();
}

#[test]
fn an_ambient_state_is_sync_so_one_capture_can_serve_many_workers() {
    // `Send` alone is not enough for the shape this crate exists to serve. A
    // traversal engine captures once at submission and shares that one state
    // across every worker it runs, which needs `Sync` and an `Arc`. Asserting
    // only `Send` would let the crate pass its whole suite and then fail to
    // compile in the consumer that motivated it.
    fn assert_sync<T: Sync>() {}
    assert_sync::<AmbientState>();
    fn assert_shareable<T: Send + Sync + 'static>() {}
    assert_shareable::<std::sync::Arc<AmbientState>>();
}

#[test]
fn capture_does_not_disturb_the_thread_it_reads() {
    let entry_mode = ThreadErrorMode::capture().expect("representable");
    let entry_token = thread_has_token();
    let _state = AmbientState::capture(CaptureSet::ALL).expect("capture");
    assert_eq!(
        ThreadErrorMode::capture().expect("representable"),
        entry_mode
    );
    assert_eq!(thread_has_token(), entry_token);
}

// --- composition (M23.3) ---------------------------------------------------

use crate::declared::{BackgroundMode, Wow64Redirection};
use crate::state::ApplyFailure;

#[test]
fn applying_an_empty_state_runs_the_operation_and_touches_nothing() {
    let entry_mode = ThreadErrorMode::capture().expect("representable");
    let entry_priority = MemoryPriority::current().expect("readable");

    let state = AmbientState::capture(CaptureSet::NONE).expect("capture");
    let applied = state.with_applied(|| 7).expect("apply");

    assert_eq!(*applied.value(), 7);
    assert!(applied.restore().is_clean());
    assert_eq!(
        ThreadErrorMode::capture().expect("representable"),
        entry_mode
    );
    assert_eq!(MemoryPriority::current().expect("readable"), entry_priority);
}

#[test]
fn a_captured_error_mode_is_in_force_inside_the_operation() {
    let entry = ThreadErrorMode::capture().expect("representable");
    let guard = ThreadErrorMode::NO_OPEN_FILE_ERROR_BOX
        .apply()
        .expect("install a distinctive mode to capture");
    let state = AmbientState::capture(CaptureSet::ERROR_MODE).expect("capture");
    guard.release().expect("restore before applying");

    let applied = state
        .with_applied(|| ThreadErrorMode::capture().expect("representable"))
        .expect("apply");

    assert_eq!(
        *applied.value(),
        ThreadErrorMode::NO_OPEN_FILE_ERROR_BOX,
        "the captured mode was not installed for the operation"
    );
    assert_eq!(
        ThreadErrorMode::capture().expect("representable"),
        entry,
        "the thread was not restored"
    );
}

#[test]
fn declared_aspects_are_installed_alongside_captured_ones() {
    let entry = MemoryPriority::current().expect("readable");
    let state = AmbientState::capture(CaptureSet::DEFAULT)
        .expect("capture")
        .with_declared(Declared::none().with_memory_priority(MemoryPriority::Low));

    let applied = state
        .with_applied(|| MemoryPriority::current().expect("readable"))
        .expect("apply");

    assert_eq!(*applied.value(), MemoryPriority::Low);
    assert_eq!(MemoryPriority::current().expect("readable"), entry);
}

#[test]
fn impersonation_is_in_force_innermost() {
    assert!(!thread_has_token(), "precondition: no thread token");
    let state = AmbientState::capture(CaptureSet::ALL).expect("capture");
    let applied = state.with_applied(thread_has_token).expect("apply");
    assert!(*applied.value(), "the captured context was not applied");
    assert!(!thread_has_token(), "the thread was left carrying a token");
}

#[test]
fn every_aspect_is_restored_after_a_full_application() {
    let entry_mode = ThreadErrorMode::capture().expect("representable");
    let entry_priority = MemoryPriority::current().expect("readable");
    let entry_token = thread_has_token();

    let state = AmbientState::capture(CaptureSet::ALL)
        .expect("capture")
        .with_declared(
            Declared::none()
                .with_memory_priority(MemoryPriority::VeryLow)
                .with_background_mode(BackgroundMode::Begin),
        );
    let applied = state.with_applied(|| ()).expect("apply");

    assert!(applied.restore().is_clean(), "{}", applied.restore());
    assert_eq!(
        ThreadErrorMode::capture().expect("representable"),
        entry_mode
    );
    assert_eq!(MemoryPriority::current().expect("readable"), entry_priority);
    assert_eq!(thread_has_token(), entry_token);
}

#[test]
fn the_error_mode_is_already_in_force_while_inner_aspects_apply() {
    // The reason the error mode is outermost: a hard error raised while a later
    // aspect is being installed must already be suppressed. Observed from
    // inside, which is the only place the ordering is visible.
    let guard = ThreadErrorMode::FAIL_CRITICAL_ERRORS
        .apply()
        .expect("install a distinctive mode to capture");
    let state = AmbientState::capture(CaptureSet::ALL).expect("capture");
    guard.release().expect("restore");

    let applied = state
        .with_applied(|| ThreadErrorMode::capture().expect("representable"))
        .expect("apply");
    assert!(
        applied
            .value()
            .contains(ThreadErrorMode::FAIL_CRITICAL_ERRORS),
        "the error mode was not in force inside the composition"
    );
}

#[test]
fn a_failing_aspect_releases_the_ones_already_installed() {
    // Redirection is installed after the error mode and the memory priority, and
    // fails in a 64-bit process. Everything applied before it must come back.
    if cfg!(not(target_pointer_width = "64")) {
        eprintln!("skipped: relies on redirection failing");
        return;
    }
    let entry_mode = ThreadErrorMode::capture().expect("representable");
    let entry_priority = MemoryPriority::current().expect("readable");

    let state = AmbientState::capture(CaptureSet::ERROR_MODE)
        .expect("capture")
        .with_declared(
            Declared::none()
                .with_memory_priority(MemoryPriority::Low)
                .with_wow64_redirection(Wow64Redirection::Disabled),
        );

    let error = state
        .with_applied(|| ())
        .expect_err("redirection cannot be disabled in a 64-bit process");
    assert!(matches!(error.failure(), ApplyFailure::Declared(_)));

    assert_eq!(
        ThreadErrorMode::capture().expect("representable"),
        entry_mode,
        "a failed inner aspect left the error mode installed"
    );
    assert_eq!(
        MemoryPriority::current().expect("readable"),
        entry_priority,
        "a failed aspect left an earlier declared aspect installed"
    );
}

#[test]
fn the_operation_does_not_run_when_an_aspect_cannot_be_installed() {
    if cfg!(not(target_pointer_width = "64")) {
        eprintln!("skipped: relies on redirection failing");
        return;
    }
    let mut ran = false;
    let state = AmbientState::capture(CaptureSet::ALL)
        .expect("capture")
        .with_declared(Declared::none().with_wow64_redirection(Wow64Redirection::Disabled));
    let _ = state.with_applied(|| ran = true);
    assert!(
        !ran,
        "the operation ran despite an aspect failing to install"
    );
}

#[test]
fn a_clean_application_yields_its_value_through_into_clean_value() {
    let state = AmbientState::capture(CaptureSet::DEFAULT).expect("capture");
    let value = state
        .with_applied(|| String::from("carried"))
        .expect("apply")
        .into_clean_value()
        .expect("a clean restore");
    assert_eq!(value, "carried");
}

#[test]
fn a_clean_report_says_so() {
    let state = AmbientState::capture(CaptureSet::NONE).expect("capture");
    let applied = state.with_applied(|| ()).expect("apply");
    assert!(applied.restore().is_clean());
    assert!(applied.restore().error_mode().is_none());
    assert!(applied.restore().declared().is_none());
    assert!(applied.restore().transaction().is_none());
    assert_eq!(
        applied.restore().to_string(),
        "the thread was restored cleanly"
    );
}

#[test]
fn a_state_applies_on_the_worker_it_was_carried_to() {
    // The composite's whole purpose, end to end: capture here, apply there, and
    // leave the worker as it was found.
    let state = AmbientState::capture(CaptureSet::ALL).expect("capture");
    let observed = std::thread::spawn(move || {
        let inherited = thread_has_token();
        let applied = state.with_applied(thread_has_token).expect("apply");
        let clean = applied.restore().is_clean();
        (inherited, *applied.value(), clean, thread_has_token())
    })
    .join()
    .expect("the worker did not panic");

    assert!(!observed.0, "a fresh worker should inherit no token");
    assert!(observed.1, "the context did not reach the worker");
    assert!(observed.2, "the worker was not restored cleanly");
    assert!(!observed.3, "the worker was left contaminated");
}

#[test]
fn applying_the_same_state_twice_is_not_special() {
    let state = AmbientState::capture(CaptureSet::ALL).expect("capture");
    for _ in 0..3 {
        let applied = state.with_applied(thread_has_token).expect("apply");
        assert!(*applied.value());
        assert!(applied.restore().is_clean());
    }
    assert!(!thread_has_token());
}

#[test]
fn nesting_two_states_restores_through_each_layer() {
    let entry = ThreadErrorMode::capture().expect("representable");

    let outer_guard = ThreadErrorMode::FAIL_CRITICAL_ERRORS
        .apply()
        .expect("install");
    let outer = AmbientState::capture(CaptureSet::ERROR_MODE).expect("capture");
    outer_guard.release().expect("restore");

    let inner_guard = ThreadErrorMode::NO_OPEN_FILE_ERROR_BOX
        .apply()
        .expect("install");
    let inner = AmbientState::capture(CaptureSet::ERROR_MODE).expect("capture");
    inner_guard.release().expect("restore");

    let deepest = outer
        .with_applied(|| {
            assert_eq!(
                ThreadErrorMode::capture().expect("representable"),
                ThreadErrorMode::FAIL_CRITICAL_ERRORS
            );
            let deepest = inner
                .with_applied(|| ThreadErrorMode::capture().expect("representable"))
                .expect("inner apply")
                .into_value();
            assert_eq!(
                ThreadErrorMode::capture().expect("representable"),
                ThreadErrorMode::FAIL_CRITICAL_ERRORS,
                "the inner release skipped the outer state"
            );
            deepest
        })
        .expect("outer apply")
        .into_value();

    assert_eq!(deepest, ThreadErrorMode::NO_OPEN_FILE_ERROR_BOX);
    assert_eq!(ThreadErrorMode::capture().expect("representable"), entry);
}

#[test]
fn an_uncaptured_aspect_leaves_the_running_threads_value_alone() {
    // Subset application: the composite must not impose a default on an aspect
    // nobody asked about.
    let installed = ThreadErrorMode::NO_GP_FAULT_ERROR_BOX
        .apply()
        .expect("install a mode the state knows nothing about");
    let state = AmbientState::capture(CaptureSet::IMPERSONATION).expect("capture");
    let during = state
        .with_applied(|| ThreadErrorMode::capture().expect("representable"))
        .expect("apply")
        .into_value();
    installed.release().expect("restore");

    assert_eq!(
        during,
        ThreadErrorMode::NO_GP_FAULT_ERROR_BOX,
        "an uncaptured aspect was overwritten instead of left alone"
    );
}
