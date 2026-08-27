// Copyright (c) 2026 Mike Grier
//! Tests for `replay`'s untrusted-input handling.

use std::time::Duration;

use windows_file_watcher_example_test_harness::{Outcome, PathologyKind};

use super::{
    DEFAULT_REPLAY_DEADLINE, MAX_REPLAY_DEADLINE, MIN_REPLAY_DEADLINE, Output, replay_deadline,
    reproduces,
};

/// An `Output` that discards everything, for tests that do not inspect it.
fn sink() -> Output<Vec<u8>, Vec<u8>> {
    Output {
        stderr: Vec::new(),
        stdout: Vec::new(),
    }
}

fn stalled(deadline_ms: u128) -> Outcome {
    Outcome::Pathology(PathologyKind::Stalled { deadline_ms })
}

#[test]
fn a_zero_deadline_is_clamped_up_to_the_floor() {
    // The attack this closes: a recording naming `deadline_ms: 0` expires
    // before the handler runs at all, and because `reproduces` compares
    // `Stalled` semantically, replay would report a wedge that never happened.
    assert_eq!(
        replay_deadline(&stalled(0), &mut sink()),
        MIN_REPLAY_DEADLINE
    );
}

#[test]
fn an_implausibly_small_deadline_is_clamped_up_to_the_floor() {
    assert_eq!(
        replay_deadline(&stalled(1), &mut sink()),
        MIN_REPLAY_DEADLINE
    );
}

#[test]
fn an_oversized_deadline_is_clamped_down_to_the_cap() {
    let huge = u128::from(u64::MAX);
    assert_eq!(
        replay_deadline(&stalled(huge), &mut sink()),
        MAX_REPLAY_DEADLINE
    );
}

#[test]
fn a_plausible_deadline_is_honoured_verbatim() {
    let requested = Duration::from_millis(1_500);
    assert!(requested > MIN_REPLAY_DEADLINE && requested < MAX_REPLAY_DEADLINE);
    assert_eq!(replay_deadline(&stalled(1_500), &mut sink()), requested);
}

#[test]
fn clamping_is_reported_but_honouring_is_silent() {
    let mut clamped = sink();
    let _ = replay_deadline(&stalled(0), &mut clamped);
    assert!(
        !clamped.stderr.is_empty(),
        "a clamp must be visible, or a reader cannot tell the replay was not run as recorded"
    );

    let mut honoured = sink();
    let _ = replay_deadline(&stalled(1_500), &mut honoured);
    assert!(honoured.stderr.is_empty(), "no clamp, so nothing to say");
}

#[test]
fn a_non_stall_outcome_uses_the_default_deadline() {
    assert_eq!(
        replay_deadline(&Outcome::Healthy, &mut sink()),
        DEFAULT_REPLAY_DEADLINE
    );
    let panicked = Outcome::Pathology(PathologyKind::Panicked {
        at_step: 0,
        message: "boom".to_string(),
    });
    assert_eq!(
        replay_deadline(&panicked, &mut sink()),
        DEFAULT_REPLAY_DEADLINE
    );
}

#[test]
fn stalls_reproduce_regardless_of_deadline_but_other_outcomes_compare_exactly() {
    // The deadline is the run's configuration, not something the handler did,
    // so a clamped stall still reproduces the recorded one.
    assert!(reproduces(&stalled(600_000), &stalled(300_000)));

    // Everything else stays an exact comparison.
    let a = Outcome::Pathology(PathologyKind::InvariantViolated {
        at_step: 1,
        reason: "x".to_string(),
    });
    let b = Outcome::Pathology(PathologyKind::InvariantViolated {
        at_step: 2,
        reason: "x".to_string(),
    });
    assert!(reproduces(&a, &a));
    assert!(!reproduces(&a, &b));
    assert!(!reproduces(&Outcome::Healthy, &stalled(100)));
}
