// Copyright (c) 2026 Mike Grier
//! The readiness check (DI-D-28) against the fake: it passes an implementation that sets its signal
//! on every empty-to-non-empty transition and fails one that breaks either half of the rule. The
//! check waits on the signal through the thread pool, and the signal is a Windows event, so these
//! are the oracle's tests that are Windows'; they sit apart from the oracle's rules so the split
//! took them whole.

use std::time::Duration;

use super::{ReadinessFailure, check_readiness};
use crate::fake::{Fake, Signals};

/// Generous: the readiness check passing waits for one pool callback.
const BOUND: Duration = Duration::from_secs(5);
/// Spent in full by each failing readiness check, so short.
const FAILING_BOUND: Duration = Duration::from_millis(200);

#[test]
fn the_readiness_check_passes_an_implementation_that_sets_on_every_transition() {
    let mut fake = Fake::new(Signals::OnEveryTransition);
    // A stale entry and a stale signal, which the check must clear before it starts.
    fake.complete_one();
    let entries = check_readiness(&mut fake, Fake::complete_one, BOUND).expect("the rule is kept");
    assert_eq!(
        entries.len(),
        3,
        "the stale entry and one for each push, handed back for the caller's oracle"
    );
}

#[test]
fn the_readiness_check_fails_an_implementation_that_never_sets() {
    let mut fake = Fake::new(Signals::Never);
    assert!(matches!(
        check_readiness(&mut fake, Fake::complete_one, FAILING_BOUND),
        Err(ReadinessFailure::NotSetForFirstEntry)
    ));
}

#[test]
fn the_readiness_check_fails_an_implementation_that_sets_only_once() {
    let mut fake = Fake::new(Signals::OnceOnly);
    assert!(matches!(
        check_readiness(&mut fake, Fake::complete_one, FAILING_BOUND),
        Err(ReadinessFailure::NotSetAgain)
    ));
}
