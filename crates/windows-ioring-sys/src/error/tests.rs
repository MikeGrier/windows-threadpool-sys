// Copyright (c) 2026 Mike Grier
use windows_sys::Win32::Foundation::{
    IORING_E_SUBMISSION_QUEUE_FULL, IORING_E_VERSION_NOT_SUPPORTED,
};

use super::{IoRingError, check};

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
