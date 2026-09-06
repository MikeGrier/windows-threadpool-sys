// Copyright (c) 2026 Mike Grier
//! Tests for the error taxonomy.

use super::*;
use std::error::Error as _;
use windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED;

#[test]
fn a_win32_code_round_trips_through_io_error() {
    let code = Win32Error::from_code(ERROR_ACCESS_DENIED);
    assert_eq!(code.code(), ERROR_ACCESS_DENIED);
    assert_eq!(
        code.to_io_error().raw_os_error(),
        Some(ERROR_ACCESS_DENIED as i32)
    );
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

// ---------------------------------------------------------------------------
// The description surface.
//
// A mutation sweep replaced every `describe` with `"xyzzy"` and with `""`, and
// nothing failed. The tests above ask only whether the rendered string is
// non-empty, which both a constant and -- for the variants whose text is
// wrapped by an outer `write!` -- the empty string satisfy.
//
// What was missing is the assertion that the variants say *different* things.
// A description exists to tell one failure from another, so collapsing the set
// onto one value is precisely the defect worth catching, and distinctness
// catches every constant substitution at once rather than one string at a time.
// ---------------------------------------------------------------------------

/// Asserts that a set of descriptions is usable: each non-empty, and no two the
/// same.
///
/// Both halves are load-bearing and neither implies the other. Non-emptiness
/// alone passes when every variant returns the same constant; distinctness
/// alone passes when the descriptions are distinct but meaningless.
fn assert_descriptions_are_distinct(descriptions: &[(&str, &str)]) {
    for (variant, text) in descriptions {
        assert!(
            !text.is_empty(),
            "{variant} has no description, so a reader learns nothing from it"
        );
    }
    for (index, (variant, text)) in descriptions.iter().enumerate() {
        for (other_variant, other_text) in &descriptions[index + 1..] {
            assert_ne!(
                text, other_text,
                "{variant} and {other_variant} describe themselves identically, so the \
                 description cannot tell them apart"
            );
        }
    }
}

#[test]
fn every_request_failure_describes_itself_distinctly() {
    assert_descriptions_are_distinct(&[
        ("EmptyPath", RequestFailure::EmptyPath.describe()),
        ("InteriorNul", RequestFailure::InteriorNul.describe()),
        ("PathTooLong", RequestFailure::PathTooLong.describe()),
        (
            "NotFullyQualified",
            RequestFailure::NotFullyQualified.describe(),
        ),
        ("PathResolution", RequestFailure::PathResolution.describe()),
        (
            "BufferCapacityUnrepresentable",
            RequestFailure::BufferCapacityUnrepresentable.describe(),
        ),
    ]);
}

#[test]
fn every_begin_failure_describes_itself_distinctly() {
    assert_descriptions_are_distinct(&[
        (
            "SubmissionRingFull",
            BeginFailure::SubmissionRingFull.describe(),
        ),
        (
            "CompletionRingFull",
            BeginFailure::CompletionRingFull.describe(),
        ),
        ("Abandoned", BeginFailure::Abandoned.describe()),
        ("TokenCapture", BeginFailure::TokenCapture.describe()),
        (
            "BufferAllocation",
            BeginFailure::BufferAllocation.describe(),
        ),
    ]);
}

#[test]
fn every_session_failure_describes_itself_distinctly() {
    // The two capacity variants are the pair most at risk: they differ by one
    // word, and a copy-paste that left both saying "submission" would be
    // invisible to a non-emptiness check while sending a reader to the wrong
    // ring.
    assert_descriptions_are_distinct(&[
        (
            "SubmissionCapacityTooSmall",
            SessionFailure::SubmissionCapacityTooSmall.describe(),
        ),
        (
            "CompletionCapacityTooSmall",
            SessionFailure::CompletionCapacityTooSmall.describe(),
        ),
        ("WorkObject", SessionFailure::WorkObject.describe()),
    ]);
}

#[test]
fn every_predicate_failure_describes_itself_distinctly() {
    assert_descriptions_are_distinct(&[
        (
            "EmptyAttributeMask",
            PredicateFailure::EmptyAttributeMask.describe(),
        ),
        ("EmptyNameSet", PredicateFailure::EmptyNameSet.describe()),
    ]);
}

#[test]
fn every_malformed_record_reason_describes_itself_distinctly() {
    // `every_malformed_record_reason_describes_itself` above checks the
    // *rendered error*, which wraps these in "a native record failed
    // validation: {}" -- so it stays non-empty even when `describe` returns
    // nothing at all. This checks the descriptions themselves.
    assert_descriptions_are_distinct(&[
        ("Alignment", MalformedRecord::Alignment.describe()),
        (
            "TruncatedFixedFields",
            MalformedRecord::TruncatedFixedFields.describe(),
        ),
        (
            "NextEntryOffset",
            MalformedRecord::NextEntryOffset.describe(),
        ),
        ("OddNameLength", MalformedRecord::OddNameLength.describe()),
        (
            "NameOutOfBounds",
            MalformedRecord::NameOutOfBounds.describe(),
        ),
        ("NegativeSize", MalformedRecord::NegativeSize.describe()),
    ]);
}

#[test]
fn a_malformed_record_error_carries_its_reason_into_the_rendered_text() {
    // Binds the wrapper to what it wraps. Without this the outer `write!` could
    // drop the description entirely and every remaining assertion would still
    // hold, because the prefix alone is non-empty.
    for detail in [
        MalformedRecord::Alignment,
        MalformedRecord::TruncatedFixedFields,
        MalformedRecord::NextEntryOffset,
        MalformedRecord::OddNameLength,
        MalformedRecord::NameOutOfBounds,
        MalformedRecord::NegativeSize,
    ] {
        let rendered = EnumerationError::MalformedRecord(detail).to_string();
        assert!(
            rendered.contains(detail.describe()),
            "{detail:?} renders as {rendered:?}, which does not contain its own description"
        );
    }
}

// ---------------------------------------------------------------------------
// Session errors: the source chain.
// ---------------------------------------------------------------------------

#[test]
fn a_session_failure_without_an_os_error_behind_it_has_neither_source_nor_suffix() {
    let error = SessionError::new(SessionFailure::WorkObject);

    assert_eq!(error.failure(), SessionFailure::WorkObject);
    assert!(error.os_error().is_none());
    assert!(error.source().is_none());
    assert_eq!(
        error.to_string(),
        SessionFailure::WorkObject.describe(),
        "with nothing behind it the rendering is exactly the description"
    );
}

#[test]
fn a_session_failure_with_an_os_error_exposes_it_three_ways() {
    // Three separate routes to the same underlying error, each of which a
    // caller may reasonably use: the typed accessor, the standard `source`
    // chain, and the rendered text. A mutation that returned `None` from either
    // accessor left the other two intact, so each needs asserting.
    let error = SessionError::with_source(
        SessionFailure::WorkObject,
        io::Error::from_raw_os_error(ERROR_ACCESS_DENIED as i32),
    );

    let os_error = error.os_error().expect("the OS error was supplied");
    assert_eq!(os_error.raw_os_error(), Some(ERROR_ACCESS_DENIED as i32));

    let source = error.source().expect("the OS error is the source");
    assert_eq!(source.to_string(), os_error.to_string());

    let rendered = error.to_string();
    assert!(
        rendered.starts_with(SessionFailure::WorkObject.describe()),
        "{rendered}"
    );
    assert!(
        rendered.len() > SessionFailure::WorkObject.describe().len(),
        "the OS error must be appended rather than dropped: {rendered}"
    );
}
