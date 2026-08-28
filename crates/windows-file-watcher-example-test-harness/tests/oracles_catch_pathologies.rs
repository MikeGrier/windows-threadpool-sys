// Copyright (c) 2026 Mike Grier
//! M3.2: each oracle trips on a deliberately-buggy handler, and a healthy
//! handler stays healthy.

#![cfg(windows)]

use std::time::Duration;

use windows_file_watcher_example_test_harness::{
    DesyncCauseSpec, Handler, Notification, NotificationSpec, OutcomeSpec, PathologyKind, Schedule,
    example_handler::PresenceTracker, run, run_with_deadline,
};

/// Establish one watch, then deliver a desync (step index 1).
fn establish_then_desync() -> Schedule {
    let mut schedule = Schedule::new();
    schedule
        .push(NotificationSpec::Completion {
            watch: 1,
            outcome: OutcomeSpec::Subscribed,
        })
        .push(NotificationSpec::Desync {
            watch: 1,
            cause: DesyncCauseSpec::Overflow,
        });
    schedule
}

/// Panics the first time it sees a desync.
struct PanicsOnDesync;
impl Handler for PanicsOnDesync {
    fn on(&mut self, notification: &Notification) {
        assert!(
            !matches!(notification, Notification::Desync { .. }),
            "handler cannot cope with a desync"
        );
    }
}

/// Reports an invariant violation once it has seen a desync.
#[derive(Default)]
struct ForbidsDesync {
    saw_desync: bool,
}
impl Handler for ForbidsDesync {
    fn on(&mut self, notification: &Notification) {
        if matches!(notification, Notification::Desync { .. }) {
            self.saw_desync = true;
        }
    }
    fn check(&self) -> Result<(), String> {
        if self.saw_desync {
            Err("a desync was delivered".to_string())
        } else {
            Ok(())
        }
    }
}

/// Wedges (parks forever) the first time it sees a desync.
struct HangsOnDesync;
impl Handler for HangsOnDesync {
    fn on(&mut self, notification: &Notification) {
        if matches!(notification, Notification::Desync { .. }) {
            // Looped: `park` is documented to return spuriously, and nothing
            // ever unparks this thread, so a bare call could let the
            // deliberately-wedged handler resume and report Healthy instead of
            // Stalled -- a flake in the test that proves the wedge oracle works.
            loop {
                std::thread::park();
            }
        }
    }
}

/// Panics *inside `check`* (rather than returning `Err`) once it has seen a
/// desync. `run` wraps both handler hooks in `catch_unwind`, so this is caught
/// and reported as a handler [`PathologyKind::Panicked`] carrying an
/// `in check():` message -- not as a stall, and not as a harness panic.
#[derive(Default)]
struct PanicsInCheck {
    saw_desync: bool,
}
impl Handler for PanicsInCheck {
    fn on(&mut self, notification: &Notification) {
        if matches!(notification, Notification::Desync { .. }) {
            self.saw_desync = true;
        }
    }
    fn check(&self) -> Result<(), String> {
        assert!(!self.saw_desync, "check panicked instead of returning Err");
        Ok(())
    }
}

#[test]
fn a_healthy_handler_is_healthy() {
    let mut handler = PresenceTracker::new();
    assert!(run(&establish_then_desync(), &mut handler).is_healthy());
}

#[test]
fn a_panic_is_caught() {
    let mut handler = PanicsOnDesync;
    let outcome = run(&establish_then_desync(), &mut handler);
    assert!(
        matches!(
            outcome.pathology(),
            Some(PathologyKind::Panicked { at_step: 1, .. })
        ),
        "expected a caught panic at step 1, got {outcome:?}"
    );
}

#[test]
fn an_invariant_violation_is_caught() {
    let mut handler = ForbidsDesync::default();
    let outcome = run(&establish_then_desync(), &mut handler);
    assert!(
        matches!(
            outcome.pathology(),
            Some(PathologyKind::InvariantViolated { at_step: 1, .. })
        ),
        "expected an invariant violation at step 1, got {outcome:?}"
    );
}

#[test]
fn a_wedged_handler_is_caught_by_the_deadline() {
    let outcome = run_with_deadline(
        &establish_then_desync(),
        HangsOnDesync,
        Duration::from_millis(200),
    );
    assert!(
        matches!(outcome.pathology(), Some(PathologyKind::Stalled { .. })),
        "expected a stall, got {outcome:?}"
    );
}

#[test]
fn a_panic_in_check_is_reported_as_a_handler_panic() {
    // Regression test (PR #42 review, twice). First round: a Handler::check
    // that panics instead of returning Err unwound straight through
    // run_with_deadline's worker thread, dropped the sender, and was
    // misdiagnosed as Stalled. Second round: catching it at the worker
    // boundary classified it as HarnessPanicked, which is also wrong --
    // `check` is the consumer's code exactly as `on` is, and the oracle
    // advertises that it catches handler panics. Both hooks are now caught
    // inside `run` and reported as Panicked.
    let outcome = run_with_deadline(
        &establish_then_desync(),
        PanicsInCheck::default(),
        Duration::from_millis(500),
    );
    let Some(PathologyKind::Panicked { message, .. }) = outcome.pathology() else {
        panic!("expected a handler panic, got {outcome:?}");
    };
    assert!(
        message.starts_with("in check():"),
        "the message must identify which hook panicked, got {message:?}"
    );
}
