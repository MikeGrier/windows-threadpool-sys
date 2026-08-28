// Copyright (c) 2026 Mike Grier
//! Tests for the request surface.

use super::*;
use crate::error::RequestFailure;
use crate::pattern::NamePattern;
use crate::predicate::{PredicateClause, QueryByExample};

fn request(path: &str) -> EnumerationRequest {
    EnumerationRequest::new(&Wtf16String::from(path)).expect("a resolvable path")
}

#[test]
fn a_request_defaults_to_the_documented_bounds() {
    let subject = request(r"C:\Windows");
    assert_eq!(subject.buffer_capacity(), DEFAULT_BUFFER_CAPACITY);
    assert_eq!(subject.file_identity_mode(), FileIdentityMode::Omit);
    assert!(subject.predicate().matches_everything());
}

#[test]
fn a_request_stores_the_resolved_path() {
    let subject = request("C:/Windows/../Windows");
    assert_eq!(subject.path().to_string_lossy(), r"C:\Windows");
}

#[test]
fn a_request_owns_its_path_independently_of_the_caller() {
    let subject = {
        let borrowed = Wtf16String::from(r"C:\Windows");
        EnumerationRequest::new(&borrowed).expect("resolvable")
    };
    assert_eq!(subject.path().to_string_lossy(), r"C:\Windows");
}

#[test]
fn a_request_can_be_built_from_a_std_path() {
    let subject = EnumerationRequest::for_path(r"C:\Windows".as_ref()).expect("resolvable");
    assert_eq!(subject.path().to_string_lossy(), r"C:\Windows");
}

#[test]
fn an_invalid_path_is_reported_when_the_request_is_built() {
    let error = EnumerationRequest::new(&Wtf16String::new()).expect_err("an empty path");
    assert_eq!(error.failure(), RequestFailure::EmptyPath);
}

#[test]
fn a_capacity_below_the_minimum_is_clamped_up() {
    let subject = request(r"C:\Windows")
        .with_buffer_capacity(0)
        .expect("representable");
    assert_eq!(subject.buffer_capacity(), MINIMUM_BUFFER_CAPACITY);

    let subject = request(r"C:\Windows")
        .with_buffer_capacity(MINIMUM_BUFFER_CAPACITY - 1)
        .expect("representable");
    assert_eq!(subject.buffer_capacity(), MINIMUM_BUFFER_CAPACITY);
}

#[test]
fn a_capacity_is_rounded_up_to_the_record_alignment() {
    let subject = request(r"C:\Windows")
        .with_buffer_capacity(4093)
        .expect("representable");
    assert_eq!(subject.buffer_capacity(), 4096);
    assert_eq!(subject.buffer_capacity() % RECORD_ALIGNMENT, 0);
}

#[test]
fn an_already_aligned_capacity_is_unchanged() {
    let subject = request(r"C:\Windows")
        .with_buffer_capacity(32 * 1024)
        .expect("representable");
    assert_eq!(subject.buffer_capacity(), 32 * 1024);
}

#[test]
fn the_minimum_and_default_capacities_are_already_aligned() {
    assert_eq!(MINIMUM_BUFFER_CAPACITY % RECORD_ALIGNMENT, 0);
    assert_eq!(DEFAULT_BUFFER_CAPACITY % RECORD_ALIGNMENT, 0);
    const { assert!(DEFAULT_BUFFER_CAPACITY > MINIMUM_BUFFER_CAPACITY) };
}

#[test]
#[cfg(target_pointer_width = "64")]
fn a_capacity_that_cannot_reach_win32_is_rejected() {
    // The capacity is passed to Win32 as a u32, so anything past that is a
    // caller mistake to report rather than a value to silently truncate. Only
    // reachable where a `usize` is wider than a `u32`.
    let beyond = usize::try_from(u32::MAX).expect("u32 fits a 64-bit usize") + 8;
    let error = request(r"C:\Windows")
        .with_buffer_capacity(beyond)
        .expect_err("beyond a Win32 u32");
    assert_eq!(
        error.failure(),
        RequestFailure::BufferCapacityUnrepresentable
    );
}

#[test]
fn the_largest_representable_capacity_is_accepted() {
    let largest = usize::try_from(u32::MAX).expect("u32 fits usize") & !(RECORD_ALIGNMENT - 1);
    let subject = request(r"C:\Windows")
        .with_buffer_capacity(largest)
        .expect("representable");
    assert_eq!(subject.buffer_capacity(), largest);
}

#[test]
fn a_capacity_whose_alignment_would_overflow_is_rejected() {
    let error = request(r"C:\Windows")
        .with_buffer_capacity(usize::MAX)
        .expect_err("rounding up would wrap");
    assert_eq!(
        error.failure(),
        RequestFailure::BufferCapacityUnrepresentable
    );
}

#[test]
fn the_identity_mode_is_carried_on_the_request() {
    for mode in [
        FileIdentityMode::Omit,
        FileIdentityMode::BestEffort,
        FileIdentityMode::Required,
    ] {
        let subject = request(r"C:\Windows").with_file_identity(mode);
        assert_eq!(subject.file_identity_mode(), mode);
    }
}

#[test]
fn a_predicate_can_be_supplied_as_a_query_or_a_predicate() {
    let query = QueryByExample::new()
        .with(PredicateClause::Name {
            pattern: NamePattern::literal(&Wtf16String::from("a")),
            case: crate::pattern::CaseSensitivity::Sensitive,
            negated: false,
        })
        .expect("valid");

    let from_query = request(r"C:\Windows").with_predicate(query.clone());
    let from_predicate = request(r"C:\Windows").with_predicate(EntryPredicate::from(query));
    assert_eq!(from_query.predicate(), from_predicate.predicate());
    assert!(!from_query.predicate().matches_everything());
}

#[test]
fn a_request_is_cloneable_so_one_directory_can_be_enumerated_twice() {
    let subject = request(r"C:\Windows")
        .with_file_identity(FileIdentityMode::BestEffort)
        .with_buffer_capacity(2048)
        .expect("representable");
    assert_eq!(subject.clone(), subject);
}
