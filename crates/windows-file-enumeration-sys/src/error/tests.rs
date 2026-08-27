// Copyright (c) 2026 Mike Grier
//! Tests for the error taxonomy.

use super::*;
use std::error::Error as _;

#[test]
fn a_win32_code_round_trips_through_io_error() {
    let code = Win32Error::from_code(5);
    assert_eq!(code.code(), 5);
    assert_eq!(code.to_io_error().raw_os_error(), Some(5));
    assert_eq!(Win32Error::from_io(&code.to_io_error()), code);
}

#[test]
fn an_io_error_without_an_os_code_becomes_zero() {
    let fabricated = io::Error::other("no OS error behind this");
    assert_eq!(Win32Error::from_io(&fabricated).code(), 0);
}

#[test]
fn a_win32_code_displays_with_its_numeric_value() {
    let rendered = Win32Error::from_code(2).to_string();
    assert!(rendered.contains('2'), "{rendered}");
}

#[test]
fn a_request_error_without_a_call_behind_it_carries_no_code() {
    let error = RequestError::new(RequestFailure::EmptyPath);
    assert_eq!(error.failure(), RequestFailure::EmptyPath);
    assert_eq!(error.code(), None);
    assert!(!error.to_string().is_empty());
}

#[test]
fn a_resolution_failure_keeps_the_code_windows_reported() {
    let error = RequestError::with_code(RequestFailure::PathResolution, Win32Error::from_code(123));
    assert_eq!(error.code(), Some(Win32Error::from_code(123)));
    assert!(error.to_string().contains("123"));
}

#[test]
fn a_predicate_error_names_which_clause_was_vacuous() {
    for failure in [
        PredicateFailure::EmptyAttributeMask,
        PredicateFailure::EmptyNameSet,
    ] {
        let error = PredicateError::new(failure);
        assert_eq!(error.failure(), failure);
        assert!(!error.to_string().is_empty());
    }
}

#[test]
fn every_native_enumeration_failure_keeps_its_raw_code() {
    let code = Win32Error::from_code(87);
    let cases = [
        EnumerationError::DirectoryOpen(code),
        EnumerationError::VolumeIdentity(code),
        EnumerationError::UnsupportedExtendedDirectoryInfo(code),
        EnumerationError::DirectoryQuery(code),
        EnumerationError::RecordTooLarge {
            buffer_capacity: 1024,
            code,
        },
    ];
    for error in cases {
        assert_eq!(error.code(), Some(code), "{error:?}");
        assert!(!error.to_string().is_empty());
    }
}

#[test]
fn an_oversize_record_reports_the_capacity_it_did_not_fit() {
    let error = EnumerationError::RecordTooLarge {
        buffer_capacity: 4096,
        code: Win32Error::from_code(234),
    };
    assert!(error.to_string().contains("4096"), "{error}");
}

#[test]
fn every_malformed_record_reason_describes_itself() {
    let cases = [
        MalformedRecord::Alignment,
        MalformedRecord::TruncatedFixedFields,
        MalformedRecord::NextEntryOffset,
        MalformedRecord::OddNameLength,
        MalformedRecord::NameOutOfBounds,
        MalformedRecord::NegativeSize,
    ];
    for detail in cases {
        let error = EnumerationError::MalformedRecord(detail);
        assert!(!error.to_string().is_empty(), "{detail:?}");
        assert_eq!(error.code(), None);
        assert!(error.source().is_none());
    }
}

#[test]
fn a_native_failure_has_no_nested_source() {
    // The raw code is the whole story for a last-error API, so there is nothing
    // to chain to.
    let error = EnumerationError::DirectoryOpen(Win32Error::from_code(3));
    assert!(error.source().is_none());
}
