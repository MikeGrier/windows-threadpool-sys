// Copyright (c) 2026 Mike Grier
//! Construction's registration paths (DI-7.2): what the backend hands back when its ring refuses a
//! registration, fails its completion, never completes it, or will not take its completion event.
//!
//! Each runs on a real ring with the ring crate's safe call-fault responder registered as the ring
//! is created, through [`Construction`]: a responder registered afterwards cannot answer the calls
//! construction itself makes. This crate forbids `unsafe` (DI-D-4), and implementing the seam's
//! trait needs it, so the responder is the ring crate's.
//!
//! The contract under test is what comes back. The targets always do; the buffers do while the
//! kernel has not been given them, and not once it has, since it cannot be shown to have given
//! them back.

use std::io;
use std::time::Duration;

use windows_ioring_sys::sys::{Call, CallFaultControl};
use windows_ioring_sys::{InjectedFailure, IoRingErrorExt};

use super::{Backend, GIVEN, TempFile, Win32FileTarget, WindowsBackendError, setup};
use crate::windows_backend::{Construction, WindowsBackendRefusal};
use durable_ioring::IoTargetKey;
use durable_ioring::backend::IoBackend;

/// `HRESULT_FROM_WIN32(ERROR_IO_DEVICE)`, as `InjectedFailure::Win32(1117)` spells it.
const DEVICE: i32 = 0x8007_045D_u32.cast_signed();

const LONG: Duration = Duration::from_secs(5);

fn device() -> InjectedFailure {
    InjectedFailure::Win32(1117)
}

/// The `HRESULT` a native report carries, or a plain error's OS code.
fn code(error: &io::Error) -> Option<i32> {
    error
        .as_ioring_error()
        .map(|native| native.code())
        .or_else(|| error.raw_os_error())
}

/// The ring's error behind a refusal.
fn ring_error(refused: &WindowsBackendError<Vec<u8>>) -> &io::Error {
    match &refused.reason {
        WindowsBackendRefusal::Ring(error) => error,
        other => panic!("refused for the wrong reason: {other:?}"),
    }
}

/// Construct with `files` and `buffers`, `arm`ed before the ring exists, waiting `bound` for each
/// registration, and return the refusal and the control that caused it.
fn refused_with(
    files: &[&TempFile],
    buffers: Vec<Vec<u8>>,
    bound: Duration,
    arm: impl FnOnce(&CallFaultControl),
) -> (WindowsBackendError<Vec<u8>>, CallFaultControl) {
    let (construction, control) = Construction::faulted(bound);
    arm(&control);
    let targets = files
        .iter()
        .enumerate()
        .map(|(i, file)| {
            (
                IoTargetKey(GIVEN.0 + i as u64),
                Win32FileTarget::new(file.open()),
            )
        })
        .collect();
    let refused = Backend::construct(setup(targets, buffers), construction)
        .err()
        .expect("construction was refused");
    (refused, control)
}

#[test]
fn a_refused_file_registration_build_hands_back_the_targets_and_the_buffers() {
    let file = TempFile::new("wbc-files-build");
    let (refused, control) = refused_with(&[&file], vec![vec![7; 4]], LONG, |control| {
        control.fail_next(Call::BuildRegisterFiles, 1, device());
    });
    assert_eq!(code(ring_error(&refused)), Some(DEVICE));
    assert_eq!(refused.targets.len(), 1);
    assert_eq!(
        refused.buffers,
        Some(vec![vec![7; 4]]),
        "the kernel was given nothing"
    );
    assert_eq!(control.failures(Call::BuildRegisterFiles).len(), 1);
    assert!(
        control.failures(Call::Submit).is_empty(),
        "refused before anything was submitted"
    );
    drop(refused);
    assert!(!file.is_held(), "no ring is left holding the file");
}

#[test]
fn a_refused_buffer_registration_build_hands_the_buffers_back_whole() {
    let file = TempFile::new("wbc-buffers-build");
    let (refused, control) = refused_with(&[&file], vec![vec![1; 8], vec![2; 16]], LONG, |c| {
        c.fail_next(Call::BuildRegisterBuffers, 1, device());
    });
    assert_eq!(code(ring_error(&refused)), Some(DEVICE));
    assert_eq!(refused.targets.len(), 1, "the files had registered");
    assert_eq!(
        refused.buffers,
        Some(vec![vec![1; 8], vec![2; 16]]),
        "in order, untouched"
    );
    assert_eq!(control.failures(Call::BuildRegisterBuffers).len(), 1);
}

#[test]
fn a_failed_file_registration_submit_hands_back_the_targets_and_the_buffers() {
    let file = TempFile::new("wbc-files-submit");
    let (refused, control) = refused_with(&[&file], vec![vec![7; 4]], LONG, |control| {
        control.fail_next(Call::Submit, 1, device());
    });
    assert_eq!(code(ring_error(&refused)), Some(DEVICE));
    assert_eq!(refused.targets.len(), 1);
    assert_eq!(refused.buffers, Some(vec![vec![7; 4]]));
    assert_eq!(control.failures(Call::Submit).len(), 1);
    drop(refused);
    assert!(!file.is_held());
}

#[test]
fn a_failed_buffer_registration_submit_does_not_hand_the_buffers_back() {
    let (refused, control) = refused_with(&[], vec![vec![7; 4]], LONG, |control| {
        control.fail_next(Call::Submit, 1, device());
    });
    assert_eq!(code(ring_error(&refused)), Some(DEVICE));
    assert!(
        refused.buffers.is_none(),
        "the registration is queued with the ring, which may still hold them"
    );
    assert_eq!(control.failures(Call::Submit).len(), 1);
}

#[test]
fn a_failed_file_registration_completion_hands_back_the_targets_and_the_buffers() {
    let file = TempFile::new("wbc-files-completion");
    let (refused, control) = refused_with(&[&file], vec![vec![7; 4]], LONG, |control| {
        control.fail_next_completions(1, device());
    });
    assert_eq!(code(ring_error(&refused)), Some(DEVICE));
    assert_eq!(refused.targets.len(), 1);
    assert_eq!(
        refused.buffers,
        Some(vec![vec![7; 4]]),
        "the files' registration failed before the buffers' began"
    );
    assert!(
        control.failures(Call::Submit).is_empty() && control.failures(Call::Pop).is_empty(),
        "a failed completion is not a failed call"
    );
}

#[test]
fn a_failed_buffer_registration_completion_does_not_hand_the_buffers_back() {
    let (refused, _) = refused_with(&[], vec![vec![7; 4]], LONG, |control| {
        control.fail_next_completions(1, device());
    });
    assert_eq!(code(ring_error(&refused)), Some(DEVICE));
    assert!(
        refused.buffers.is_none(),
        "the kernel was given them, and cannot be shown to have given them back"
    );
}

#[test]
fn a_registration_that_does_not_complete_within_the_bound_fails_construction_on_that_bound() {
    let file = TempFile::new("wbc-stall");
    let bound = Duration::from_millis(50);
    let (refused, _control) = refused_with(&[&file], vec![vec![7; 4]], bound, |control| {
        control.stall_submits(Duration::from_millis(600));
    });
    let error = ring_error(&refused);
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert!(
        error.to_string().contains("50ms"),
        "it names the bound it waited, not the default: {error}"
    );
    assert_eq!(refused.targets.len(), 1);
    assert_eq!(refused.buffers, Some(vec![vec![7; 4]]));
    drop(refused);
    assert!(
        !file.is_held(),
        "the ring ran down once the stall ended, and released the file"
    );
}

#[test]
fn a_buffer_registration_that_does_not_complete_within_the_bound_fails_on_that_bound() {
    // No files, so the buffers' registration is the one that waits: a bound that reached only the
    // files' would leave this one on the default.
    let (refused, _control) = refused_with(
        &[],
        vec![vec![7; 4]],
        Duration::from_millis(50),
        |control| control.stall_submits(Duration::from_millis(600)),
    );
    let error = ring_error(&refused);
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert!(error.to_string().contains("50ms"), "{error}");
    assert!(
        refused.buffers.is_none(),
        "the kernel may have been given them"
    );
}

#[test]
fn a_completion_event_that_cannot_be_attached_hands_back_the_targets_only() {
    let file = TempFile::new("wbc-event");
    let (refused, control) = refused_with(&[&file], vec![vec![7; 4]], LONG, |control| {
        control.fail_next(Call::SetCompletionEvent, 1, device());
    });
    assert_eq!(code(ring_error(&refused)), Some(DEVICE));
    assert_eq!(refused.targets.len(), 1, "both registrations had completed");
    assert!(
        refused.buffers.is_none(),
        "the kernel was given them, so they stay with the ring"
    );
    assert_eq!(control.failures(Call::SetCompletionEvent).len(), 1);
}

#[test]
fn a_fault_armed_for_a_call_construction_does_not_make_leaves_it_alone() {
    let file = TempFile::new("wbc-unrelated");
    let (construction, control) = Construction::faulted(LONG);
    control.fail_until_cleared(Call::BuildRead, device());
    control.fail_until_cleared(Call::BuildCancel, device());
    let backend = Backend::construct(
        setup(
            vec![(GIVEN, Win32FileTarget::new(file.open()))],
            vec![vec![0; 4]],
        ),
        construction,
    )
    .expect("construction makes neither call");
    assert_eq!(backend.registered_buffers(), 1);
    assert!(control.failures(Call::BuildRead).is_empty());
    control.clear();
}
