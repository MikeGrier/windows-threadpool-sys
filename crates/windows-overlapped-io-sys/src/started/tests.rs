// Copyright (c) 2026 Mike Grier
//! Unit tests for the adapter submission outcome.

use super::Started;

/// A stand-in token; the enum is generic and never inspects it.
#[derive(Debug, PartialEq, Eq)]
struct Token(u32);

fn pending() -> Started<Token, Vec<u8>> {
    Started::Pending(Token(7))
}

fn completed() -> Started<Token, Vec<u8>> {
    Started::Completed {
        payload: vec![1, 2, 3],
        bytes_transferred: 3,
    }
}

#[test]
fn a_pending_outcome_reports_only_pending() {
    let started = pending();
    assert!(started.is_pending());
    assert!(!started.is_completed());
}

#[test]
fn a_completed_outcome_reports_only_completed() {
    let started = completed();
    assert!(started.is_completed());
    assert!(!started.is_pending());
}

#[test]
fn pending_yields_the_token_and_completed_yields_nothing() {
    assert_eq!(pending().pending(), Some(Token(7)));
    assert!(completed().pending().is_none());
}

#[test]
fn completed_yields_the_payload_and_count_and_pending_yields_nothing() {
    assert_eq!(completed().completed(), Some((vec![1, 2, 3], 3)));
    assert!(pending().completed().is_none());
}

#[test]
fn expect_pending_unwraps_a_pending_outcome() {
    assert_eq!(pending().expect_pending("not skip-on-success"), Token(7));
}

#[test]
#[should_panic(expected = "the endpoint was in skip-on-success mode")]
fn expect_pending_panics_on_a_synchronous_completion() {
    let _ = completed().expect_pending("the endpoint was in skip-on-success mode");
}

#[test]
fn an_empty_synchronous_completion_is_representable() {
    // A zero-byte transfer is a real outcome (a read at EOF, say), not a
    // sentinel for "nothing happened".
    let started: Started<Token, Vec<u8>> = Started::Completed {
        payload: Vec::new(),
        bytes_transferred: 0,
    };
    assert!(started.is_completed());
    assert_eq!(started.completed(), Some((Vec::new(), 0)));
}

#[test]
fn the_payload_type_is_not_constrained_to_a_byte_buffer() {
    // The scatter/gather adapters report `PageBuffers`, not `Vec<u8>`.
    let started: Started<Token, String> = Started::Completed {
        payload: String::from("pages"),
        bytes_transferred: 5,
    };
    assert_eq!(started.completed(), Some((String::from("pages"), 5)));
}
