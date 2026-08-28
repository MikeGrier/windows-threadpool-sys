// Copyright (c) 2026 Mike Grier
//! Tests for the completion record surface.

use super::*;
use crate::error::{MalformedRecord, Win32Error};
use crate::testing::named_file;
use windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED;

#[test]
fn an_identifier_round_trips_through_its_raw_value() {
    let id = EnumerationId::from_raw(17);
    assert_eq!(id.get(), 17);
    assert_eq!(EnumerationId::from_raw(id.get()), id);
}

#[test]
fn identifiers_order_and_hash_by_value() {
    use std::collections::HashSet;

    assert!(EnumerationId::from_raw(1) < EnumerationId::from_raw(2));
    let set: HashSet<_> = [EnumerationId::from_raw(1), EnumerationId::from_raw(1)]
        .into_iter()
        .collect();
    assert_eq!(set.len(), 1);
}

#[test]
fn an_identifier_displays_readably() {
    assert_eq!(EnumerationId::from_raw(3).to_string(), "enumeration 3");
}

#[test]
fn an_entry_record_names_its_enumeration() {
    let record = Completion::Entry {
        enumeration: EnumerationId::from_raw(5),
        entry: named_file("a.txt"),
    };
    assert_eq!(record.enumeration(), EnumerationId::from_raw(5));
    assert!(!record.is_terminal());
}

#[test]
fn a_terminal_record_names_its_enumeration_and_ends_it() {
    let record = Completion::Terminal {
        enumeration: EnumerationId::from_raw(6),
        outcome: TerminalOutcome::Completed,
    };
    assert_eq!(record.enumeration(), EnumerationId::from_raw(6));
    assert!(record.is_terminal());
}

#[test]
fn a_completed_outcome_carries_no_failure() {
    let outcome = TerminalOutcome::Completed;
    assert!(outcome.is_completed());
    assert!(outcome.failure().is_none());
}

#[test]
fn a_cancelled_outcome_is_neither_completed_nor_failed() {
    let outcome = TerminalOutcome::Cancelled;
    assert!(!outcome.is_completed());
    assert!(outcome.failure().is_none());
}

#[test]
fn a_failed_outcome_carries_its_error_inside_the_terminal() {
    // The failure travels *in* the terminal, which is what makes one reserved
    // slot enough to report it even when the ring is otherwise full.
    let outcome = TerminalOutcome::Failed(EnumerationError::DirectoryQuery(Win32Error::from_code(
        ERROR_ACCESS_DENIED,
    )));
    assert!(!outcome.is_completed());
    let failure = outcome.failure().expect("a failed outcome has an error");
    assert_eq!(
        failure.code(),
        Some(Win32Error::from_code(ERROR_ACCESS_DENIED))
    );
}

#[test]
fn a_malformed_record_failure_has_no_win32_code() {
    let error = EnumerationError::MalformedRecord(MalformedRecord::Alignment);
    assert_eq!(error.code(), None);
}
