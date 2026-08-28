// Copyright (c) Mike Grier.

//! Tests for the impersonation aspect.
//!
//! These assert the adaptation this module actually performs -- subset
//! application, the three-state shape, and that a captured context survives a
//! thread boundary. They do not re-test capture, transport, or restoration
//! themselves, which belong to `windows-impersonation-token-sys` and are tested
//! there; re-asserting them here would be a second copy of that crate's
//! contract rather than a check of this one.

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_NO_TOKEN, HANDLE};
use windows_sys::Win32::Security::{
    ImpersonateSelf, RevertToSelf, SecurityImpersonation, TOKEN_QUERY,
};
use windows_sys::Win32::System::Threading::{GetCurrentThread, OpenThreadToken};

use super::{capture, with_applied};
use crate::captured::Captured;

/// Whether the calling thread currently carries an impersonation token.
fn thread_has_token() -> bool {
    let mut handle: HANDLE = std::ptr::null_mut();
    // SAFETY: `handle` is a valid writable destination; the pseudo-handle from
    // `GetCurrentThread` needs no cleanup.
    let ok = unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &mut handle) };
    if ok == 0 {
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(ERROR_NO_TOKEN as i32),
            "the only expected failure here is that there is no token"
        );
        return false;
    }
    // SAFETY: the call above produced this handle and nothing else owns it.
    unsafe { CloseHandle(handle) };
    true
}

/// Give the calling thread a token for the duration of `body`.
fn while_impersonating<T>(body: impl FnOnce() -> T) -> T {
    // SAFETY: no preconditions; paired with `RevertToSelf` below.
    let ok = unsafe { ImpersonateSelf(SecurityImpersonation) };
    assert!(ok != 0, "ImpersonateSelf failed");
    let outcome = body();
    // SAFETY: no preconditions.
    let reverted = unsafe { RevertToSelf() };
    assert!(reverted != 0, "RevertToSelf failed");
    outcome
}

#[test]
fn capture_yields_present_even_when_the_thread_is_not_impersonating() {
    // The documented behaviour of the dependency: it snapshots the process
    // identity rather than reporting absence, which is why `Absent` is
    // unreachable for this aspect.
    assert!(!thread_has_token(), "precondition: no thread token");
    let captured = capture().expect("capture succeeds without a thread token");
    assert!(matches!(captured, Captured::Present(_)));
}

#[test]
fn capture_yields_present_while_impersonating() {
    while_impersonating(|| {
        assert!(thread_has_token(), "precondition: the thread has a token");
        let captured = capture().expect("capture succeeds while impersonating");
        assert!(matches!(captured, Captured::Present(_)));
    });
}

#[test]
fn not_captured_runs_the_operation_without_touching_the_thread() {
    let before = thread_has_token();
    let value = with_applied(&Captured::NotCaptured, || 42).expect("no token to apply");
    assert_eq!(value, 42);
    assert_eq!(thread_has_token(), before, "the thread was disturbed");
}

#[test]
fn absent_runs_the_operation_without_touching_the_thread() {
    // Unreachable from `capture`, but the state is representable, so applying it
    // must be defined rather than left to whatever `present()` happens to do.
    let before = thread_has_token();
    let value = with_applied(&Captured::Absent, || 42).expect("nothing to apply");
    assert_eq!(value, 42);
    assert_eq!(thread_has_token(), before, "the thread was disturbed");
}

#[test]
fn a_present_token_is_applied_for_the_operation_and_reverted_after() {
    assert!(!thread_has_token(), "precondition: no thread token");
    let captured = capture().expect("capture");
    let saw_token = with_applied(&captured, thread_has_token).expect("apply");
    assert!(saw_token, "the token was not in force inside the operation");
    assert!(
        !thread_has_token(),
        "the thread was left carrying a token afterwards"
    );
}

#[test]
fn the_operations_return_value_is_passed_through() {
    let captured = capture().expect("capture");
    let value = with_applied(&captured, || String::from("carried")).expect("apply");
    assert_eq!(value, "carried");
}

#[test]
fn a_captured_context_applies_on_another_thread() {
    // The point of the crate: a worker inherits nothing, so the context has to
    // travel. Asserted rather than assumed.
    let captured = capture().expect("capture on the submitting thread");
    let observed = std::thread::spawn(move || {
        let inherited = thread_has_token();
        let during = with_applied(&captured, thread_has_token).expect("apply on the worker");
        (inherited, during, thread_has_token())
    })
    .join()
    .expect("the worker did not panic");

    assert!(!observed.0, "a fresh worker should inherit no token");
    assert!(observed.1, "the context did not arrive on the worker");
    assert!(!observed.2, "the worker was left contaminated");
}

#[test]
fn a_panicking_operation_still_leaves_the_thread_restored() {
    // Restoration on the unwind path is the case that only ever runs when
    // something has already gone wrong, so it is exercised deliberately.
    assert!(!thread_has_token(), "precondition: no thread token");
    let captured = capture().expect("capture");
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = with_applied(&captured, || panic!("the operation fails"));
    }))
    .is_err();

    assert!(panicked, "the panic should propagate to the caller");
    assert!(
        !thread_has_token(),
        "unwinding left the thread carrying a token"
    );
}

#[test]
fn a_token_can_be_applied_more_than_once() {
    let captured = capture().expect("capture");
    for _ in 0..3 {
        assert!(with_applied(&captured, thread_has_token).expect("apply"));
    }
    assert!(!thread_has_token());
}

#[test]
fn a_context_captured_while_impersonating_carries_a_token_to_a_worker() {
    let captured = while_impersonating(|| capture().expect("capture while impersonating"));
    let during = std::thread::spawn(move || {
        with_applied(&captured, thread_has_token).expect("apply on the worker")
    })
    .join()
    .expect("no panic");
    assert!(during, "the impersonated context did not reach the worker");
}

#[test]
fn nesting_applications_restores_through_each_layer() {
    let outer = capture().expect("capture");
    let inner = capture().expect("capture");
    let deepest = with_applied(&outer, || {
        assert!(thread_has_token(), "outer did not apply");
        with_applied(&inner, thread_has_token).expect("inner apply")
    })
    .expect("outer apply");
    assert!(deepest, "inner did not apply");
    assert!(!thread_has_token(), "nesting left the thread contaminated");
}
