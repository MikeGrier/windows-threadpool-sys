// Copyright (c) 2026 Mike Grier
//! Oracles: driving a handler and detecting when it goes wrong.
//!
//! Three pathologies are detected, matching the three ways a notification
//! handler tends to fail under an unexpected schedule: it panics, it violates
//! its own invariant, or it wedges. [`run`] catches the first two; the
//! wedge-catching [`run_with_deadline`] adds the third.

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use windows_file_watcher::{DEFAULT_BOUND, channel_with_bound};

use crate::{Handler, Schedule};

/// The result of running a schedule against a handler.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Every notification was processed and the handler's invariant held.
    Healthy,
    /// A pathology was detected; the run stopped there.
    Pathology(PathologyKind),
}

impl Outcome {
    /// Whether the run was healthy.
    #[must_use]
    pub fn is_healthy(&self) -> bool {
        matches!(self, Outcome::Healthy)
    }

    /// The pathology, if any.
    #[must_use]
    pub fn pathology(&self) -> Option<&PathologyKind> {
        match self {
            Outcome::Healthy => None,
            Outcome::Pathology(kind) => Some(kind),
        }
    }
}

/// What went wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PathologyKind {
    /// The handler panicked while processing the notification at `at_step`.
    Panicked {
        /// The schedule step being delivered when the panic happened.
        at_step: usize,
        /// The panic message, if it was a string.
        message: String,
    },
    /// The handler's own invariant ([`Handler::check`]) reported a violation
    /// after the notification at `at_step` (or at end of run).
    InvariantViolated {
        /// The step after which the invariant failed (`steps.len()` for the
        /// end-of-run check).
        at_step: usize,
        /// The reason the handler gave.
        reason: String,
    },
    /// The handler wedged -- it did not finish within the deadline. Only
    /// [`run_with_deadline`] reports this.
    Stalled {
        /// The deadline that elapsed.
        deadline: Duration,
    },
}

/// Drive `handler` through `schedule`, watching for a panic or an invariant
/// violation, and return at the first pathology (or [`Outcome::Healthy`]).
///
/// The handler's `on` is wrapped in `catch_unwind`: the handler is *consumer*
/// code, not an FFI callback, so containing its panic is correct here -- the
/// opposite of file-watcher's own trampolines, which must let a panic abort the
/// process. This does not catch a handler that *hangs*; use
/// [`run_with_deadline`] for that.
pub fn run(schedule: &Schedule, handler: &mut impl Handler) -> Outcome {
    let (sender, receiver) = channel_with_bound(DEFAULT_BOUND);
    for (step, spec) in schedule.steps.iter().enumerate() {
        let _ = sender.send(spec.to_notification());
        while let Some(notification) = receiver.try_recv() {
            if let Err(payload) = catch_unwind(AssertUnwindSafe(|| handler.on(&notification))) {
                return Outcome::Pathology(PathologyKind::Panicked {
                    at_step: step,
                    message: panic_message(&*payload),
                });
            }
            if let Err(reason) = handler.check() {
                return Outcome::Pathology(PathologyKind::InvariantViolated {
                    at_step: step,
                    reason,
                });
            }
        }
    }
    if let Err(reason) = handler.check() {
        return Outcome::Pathology(PathologyKind::InvariantViolated {
            at_step: schedule.steps.len(),
            reason,
        });
    }
    Outcome::Healthy
}

/// Like [`run`], but bounds the whole run by `deadline` so a handler that
/// *wedges* (deadlocks or loops forever) on some notification is caught as
/// [`PathologyKind::Stalled`] rather than hanging the test.
///
/// The handler runs on a worker thread (hence the `Send + 'static` bound and the
/// schedule clone). If it does not finish in time the worker is **leaked** -- a
/// wedged thread cannot be killed safely in Rust -- which is fine for a one-shot
/// test process. On completion the handler is dropped in the worker; use [`run`]
/// when you need to inspect the handler afterward.
#[must_use]
pub fn run_with_deadline<H>(schedule: &Schedule, handler: H, deadline: Duration) -> Outcome
where
    H: Handler + Send + 'static,
{
    let schedule = schedule.clone();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut handler = handler;
        let _ = tx.send(run(&schedule, &mut handler));
    });
    match rx.recv_timeout(deadline) {
        Ok(outcome) => outcome,
        Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {
            Outcome::Pathology(PathologyKind::Stalled { deadline })
        }
    }
}

/// Best-effort extraction of a panic message from a caught payload.
fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}
