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
