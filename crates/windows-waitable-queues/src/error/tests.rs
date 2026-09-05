// Copyright (c) Mike Grier.

//! Tests for the error types.
//!
//! # Why these exist as their own file
//!
//! This module had no tests at all, and a mutation run said so: ten survivors,
//! covering every `Display`, both `source` implementations, and -- the ones
//! that matter -- both `is_retryable` predicates, which could be replaced by a
//! constant `true` or a constant `false` with the whole suite still green.
//!
//! `is_retryable` is not decoration. It is what a caller branches on to decide
//! between backing off and giving up, so a constant answer is either an
//! infinite retry against a dead queue or a dropped item that would have gone
//! through a moment later. That the shapes' own suites exercise the *happy*
//! direction is what let the constant survive: asserting only that a full
//! queue is retryable never asks what a disconnected one says.

use std::io;

use super::{CapacityError, Disconnected, PushError, RecvError, RecvTimeoutError};
use crate::capacity::Bounds;

/// An `io::Error` distinguishable from any other, so a `source` that returns
/// the wrong one is not mistaken for a right one.
fn io_error() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "the doorbell broke")
}

#[test]
fn a_push_refused_for_room_is_retryable_and_one_refused_for_disconnection_is_not() {
    // Both directions, because a predicate asserted in one direction only is
    // satisfied by the constant that agrees with it.
    assert!(
        PushError::Full(1_u32).is_retryable(),
        "a full queue may have room later"
    );
    assert!(
        !PushError::Disconnected(1_u32).is_retryable(),
        "a queue with no consumers will never take this item"
    );
}

#[test]
fn a_refused_push_hands_back_the_item_whichever_way_it_failed() {
    // The item is the caller's, and losing it is the failure this type exists
    // to prevent: `into_inner` is the only way back.
    assert_eq!(PushError::Full(7_u32).into_inner(), 7);
    assert_eq!(PushError::Disconnected(9_u32).into_inner(), 9);
    assert_eq!(Disconnected(11_u32).into_inner(), 11);
}

#[test]
fn only_a_timeout_is_retryable_among_the_timed_receive_failures() {
    assert!(
        RecvTimeoutError::Timeout.is_retryable(),
        "nothing arrived in time, and something still might"
    );
    assert!(
        !RecvTimeoutError::Disconnected.is_retryable(),
        "no further item will ever arrive"
    );
    assert!(
        !RecvTimeoutError::from(io_error()).is_retryable(),
        "a failed wait is not a reason to spin on the same call"
    );
}

#[test]
fn only_the_io_failures_carry_a_source() {
    use core::error::Error as _;

    assert!(
        RecvError::Disconnected.source().is_none(),
        "an ended stream is not caused by anything else"
    );
    assert!(
        RecvError::from(io_error()).source().is_some(),
        "a failed wait must expose the failure underneath it"
    );

    assert!(RecvTimeoutError::Timeout.source().is_none());
    assert!(RecvTimeoutError::Disconnected.source().is_none());
    assert!(RecvTimeoutError::from(io_error()).source().is_some());

    // And the source is the *right* error, not merely some error.
    let source = RecvError::from(io_error())
        .source()
        .expect("just asserted")
        .to_string();
    assert!(source.contains("the doorbell broke"), "got {source}");
}

#[test]
fn every_error_renders_something_that_names_its_cause() {
    // A `Display` that writes nothing satisfies any test that only checks it
    // does not panic, so each rendering is asked for a word only it would use.
    let cases: Vec<(String, &str)> = vec![
        (PushError::Full(1_u32).to_string(), "capacity"),
        (PushError::Disconnected(1_u32).to_string(), "consumer"),
        (Disconnected(1_u32).to_string(), "consumer"),
        (RecvError::Disconnected.to_string(), "producer"),
        (RecvError::from(io_error()).to_string(), "doorbell"),
        (RecvTimeoutError::Timeout.to_string(), "deadline"),
        (RecvTimeoutError::Disconnected.to_string(), "producer"),
        (RecvTimeoutError::from(io_error()).to_string(), "doorbell"),
    ];

    for (rendered, expected) in cases {
        assert!(
            rendered.to_lowercase().contains(expected),
            "{rendered:?} does not mention {expected:?}"
        );
    }
}

#[test]
fn a_capacity_error_renders_the_numbers_a_caller_needs_to_correct_the_call() {
    // The whole value of this error is the three numbers, so a rendering that
    // omits them leaves the caller guessing at a legal capacity.
    let bounds = Bounds {
        min: 2,
        max: 1 << 20,
    };
    let too_large = CapacityError::too_large(usize::MAX, bounds);
    let rendered = too_large.to_string();

    assert!(
        rendered.contains(&usize::MAX.to_string()),
        "the rejected capacity must appear: {rendered}"
    );
    assert_eq!(too_large.requested(), usize::MAX);
    assert_eq!(too_large.min_valid(), bounds.min);
    assert_eq!(too_large.max_valid(), bounds.max);
}
