// Copyright (c) 2026 Mike Grier
use windows_sys::Win32::Foundation::{
    IORING_E_COMPLETION_QUEUE_TOO_FULL, IORING_E_CORRUPT, IORING_E_SUBMISSION_QUEUE_FULL,
    IORING_E_SUBMIT_IN_PROGRESS, IORING_E_VERSION_NOT_SUPPORTED,
};

use super::{IoRingError, IoRingErrorExt, RingCondition, check};

/// Every condition this crate names, for the round-trip and agreement tests.
/// Listed explicitly rather than iterated, because `RingCondition` is
/// `#[non_exhaustive]` and has no iterator -- a new variant must be added
/// here deliberately.
const ALL: [RingCondition; 8] = [
    RingCondition::RequiredFlagNotSupported,
    RingCondition::VersionNotSupported,
    RingCondition::SubmissionQueueFull,
    RingCondition::SubmissionQueueTooBig,
    RingCondition::CompletionQueueTooBig,
    RingCondition::Corrupt,
    RingCondition::SubmitInProgress,
    RingCondition::CompletionQueueTooFull,
];

#[test]
fn a_non_negative_hresult_is_ok() {
    check(0).expect("S_OK is success");
    check(1).expect("S_FALSE is success");
}

#[test]
fn a_negative_hresult_is_an_error() {
    let error = check(IORING_E_SUBMISSION_QUEUE_FULL).expect_err("a failure HRESULT");
    assert_eq!(
        error.get_ref().expect("wraps an IoRingError").to_string(),
        error.to_string()
    );
}

#[test]
fn a_named_hresult_displays_its_name() {
    let error = IoRingError::new(IORING_E_VERSION_NOT_SUPPORTED);
    assert_eq!(error.name(), Some("IORING_E_VERSION_NOT_SUPPORTED"));
    assert!(error.to_string().contains("IORING_E_VERSION_NOT_SUPPORTED"));
}

#[test]
fn an_unnamed_hresult_still_displays_its_raw_value() {
    let error = IoRingError::new(-1);
    assert_eq!(error.name(), None);
    assert!(error.to_string().contains("HRESULT"));
}

#[test]
fn code_reports_the_raw_value() {
    let error = IoRingError::new(IORING_E_SUBMISSION_QUEUE_FULL);
    assert_eq!(error.code(), IORING_E_SUBMISSION_QUEUE_FULL);
}

// --- named conditions and predicates (M10.5, D-30) ---

#[test]
fn every_condition_round_trips_through_its_hresult() {
    for condition in ALL {
        assert_eq!(
            RingCondition::from_hresult(condition.code()),
            Some(condition),
            "{} does not round-trip through its HRESULT",
            condition.name()
        );
    }
}

#[test]
fn name_agrees_with_condition_for_every_named_code() {
    // `name` is derived from `condition` rather than matching a second time,
    // so these cannot disagree -- this asserts the derivation rather than a
    // copy of the mapping.
    for condition in ALL {
        let error = IoRingError::new(condition.code());
        assert_eq!(error.condition(), Some(condition));
        assert_eq!(error.name(), Some(condition.name()));
    }
}

#[test]
fn every_condition_name_is_the_ioring_e_constant_spelling() {
    for condition in ALL {
        assert!(
            condition.name().starts_with("IORING_E_"),
            "{} is not an IORING_E_* spelling",
            condition.name()
        );
    }
}

#[test]
fn an_unnamed_code_has_no_condition() {
    let error = IoRingError::new(-1);
    assert_eq!(error.condition(), None);
    assert!(!error.is_submission_queue_full());
}

#[test]
fn the_predicates_select_exactly_their_own_condition() {
    let full = IoRingError::new(IORING_E_SUBMISSION_QUEUE_FULL);
    assert!(full.is_submission_queue_full());
    assert!(!full.is_completion_queue_too_full());
    assert!(!full.is_submit_in_progress());

    let cq_full = IoRingError::new(IORING_E_COMPLETION_QUEUE_TOO_FULL);
    assert!(cq_full.is_completion_queue_too_full());
    assert!(!cq_full.is_submission_queue_full());

    let in_progress = IoRingError::new(IORING_E_SUBMIT_IN_PROGRESS);
    assert!(in_progress.is_submit_in_progress());
    assert!(!in_progress.is_submission_queue_full());
}

#[test]
fn the_extension_trait_answers_through_a_wrapped_io_error() {
    // The whole point of M10.5: an `io::Error` from `check` reports
    // `ErrorKind::Other`, so `kind()` cannot discriminate -- but the
    // extension trait can.
    let error = check(IORING_E_SUBMISSION_QUEUE_FULL).expect_err("a failure HRESULT");
    assert_eq!(error.kind(), std::io::ErrorKind::Other);
    assert!(error.is_submission_queue_full());
    assert_eq!(
        error.ring_condition(),
        Some(RingCondition::SubmissionQueueFull)
    );
    assert_eq!(
        error.as_ioring_error().map(IoRingError::code),
        Some(IORING_E_SUBMISSION_QUEUE_FULL)
    );
}

#[test]
fn the_extension_trait_reports_nothing_for_this_crates_own_rejections() {
    // This crate's own refusals carry a meaningful `kind` and wrap no
    // `HRESULT`, so every ring predicate must answer `false` rather than
    // matching by accident.
    let error = std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "this ring does not support X",
    );
    assert!(error.as_ioring_error().is_none());
    assert_eq!(error.ring_condition(), None);
    assert!(!error.is_submission_queue_full());
    assert!(!error.is_completion_queue_too_full());
    assert!(!error.is_submit_in_progress());
}

#[test]
fn a_fatal_condition_is_reachable_even_without_its_own_predicate() {
    // Only the runtime-actionable conditions get predicates; the rest stay
    // reachable through `condition()`, so the platform is not narrowed to
    // the three branches a submission loop happens to need.
    let error = check(IORING_E_CORRUPT).expect_err("a failure HRESULT");
    assert_eq!(error.ring_condition(), Some(RingCondition::Corrupt));
    assert!(!error.is_submission_queue_full());
}
