// Copyright (c) 2026 Mike Grier
//! Oracles: driving a handler and detecting when it goes wrong.
//!
//! Three pathologies a notification handler tends to fail with are detected:
//! it panics, it violates its own invariant, or it wedges. [`run`] catches the
//! first two -- a panic in *either* handler hook, `on` or [`Handler::check`] --
//! and the wedge-catching [`run_with_deadline`] adds the third. A panic in the
//! oracle machinery itself is kept distinct from all three (see
//! [`PathologyKind::HarnessPanicked`]).

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use windows_file_watcher::{DEFAULT_BOUND, channel_with_bound};

use crate::{Handler, Schedule};

/// The result of running a schedule against a handler.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PathologyKind {
    /// The handler panicked, in either [`Handler::on`] or [`Handler::check`],
    /// while processing the notification at `at_step`.
    ///
    /// Both hooks are the consumer's code, so a panic in either is a handler
    /// pathology rather than a [`PathologyKind::HarnessPanicked`]. A panic that
    /// came from `check` is prefixed `in check():` in `message`; `at_step` is
    /// `steps.len()` for the end-of-run check.
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
        /// The deadline that elapsed, in milliseconds (a `Duration` has no
        /// direct serde representation; this loses only sub-millisecond
        /// precision, irrelevant at these timescales).
        deadline_ms: u128,
    },
    /// The harness itself panicked while running the schedule.
    ///
    /// **Not** the handler: [`run`] wraps *both* handler hooks -- `on` and
    /// [`Handler::check`] -- in `catch_unwind` and reports either panic as
    /// [`PathologyKind::Panicked`], so a failure in consumer code never lands
    /// here. This is reserved for a defect in the oracle machinery itself, and
    /// seeing it means the harness is at fault rather than the handler under
    /// test. Only [`run_with_deadline`] can report it, since it is the only
    /// entry point that runs [`run`] behind a thread boundary it must guard.
    HarnessPanicked {
        /// The panic message, if it was a string.
        message: String,
    },
}

/// Drive `handler` through `schedule`, watching for a panic or an invariant
/// violation, and return at the first pathology (or [`Outcome::Healthy`]).
///
/// Both handler hooks -- `on` **and** `check` -- are wrapped in `catch_unwind`:
/// the handler is *consumer* code, not an FFI callback, so containing its panic
/// is correct here -- the opposite of file-watcher's own trampolines, which must
/// let a panic abort the process. A panic from either hook is reported as
/// [`PathologyKind::Panicked`], since both are the consumer's code failing. This
/// does not catch a handler that *hangs*; use [`run_with_deadline`] for that.
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
            match catch_unwind(AssertUnwindSafe(|| handler.check())) {
                Err(payload) => {
                    return Outcome::Pathology(PathologyKind::Panicked {
                        at_step: step,
                        message: format!("in check(): {}", panic_message(&*payload)),
                    });
                }
                Ok(Err(reason)) => {
                    return Outcome::Pathology(PathologyKind::InvariantViolated {
                        at_step: step,
                        reason,
                    });
                }
                Ok(Ok(())) => {}
            }
        }
    }
    match catch_unwind(AssertUnwindSafe(|| handler.check())) {
        Err(payload) => Outcome::Pathology(PathologyKind::Panicked {
            at_step: schedule.steps.len(),
            message: format!("in check(): {}", panic_message(&*payload)),
        }),
        Ok(Err(reason)) => Outcome::Pathology(PathologyKind::InvariantViolated {
            at_step: schedule.steps.len(),
            reason,
        }),
        Ok(Ok(())) => Outcome::Healthy,
    }
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
///
/// The worker's call to [`run`] is itself wrapped in `catch_unwind`, so a panic
/// that escapes `run` is reported as
/// [`PathologyKind::HarnessPanicked`] rather than silently disconnecting the
/// channel and being misdiagnosed as [`PathologyKind::Stalled`]. A genuine
/// stall -- the worker never finishing at all -- remains the only way to see
/// `Stalled`.
#[must_use]
pub fn run_with_deadline<H>(schedule: &Schedule, handler: H, deadline: Duration) -> Outcome
where
    H: Handler + Send + 'static,
{
    let schedule = schedule.clone();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut handler = handler;
        let outcome = match catch_unwind(AssertUnwindSafe(|| run(&schedule, &mut handler))) {
            Ok(outcome) => outcome,
            Err(payload) => Outcome::Pathology(PathologyKind::HarnessPanicked {
                message: panic_message(&*payload),
            }),
        };
        let _ = tx.send(outcome);
    });
    match rx.recv_timeout(deadline) {
        Ok(outcome) => outcome,
        Err(RecvTimeoutError::Timeout) => Outcome::Pathology(PathologyKind::Stalled {
            deadline_ms: deadline.as_millis(),
        }),
        // The worker above always sends an outcome, even on a caught panic, so
        // this should not occur in practice; kept as a distinct, honestly-named
        // fallback rather than folding it into `Stalled` (which it is not).
        Err(RecvTimeoutError::Disconnected) => Outcome::Pathology(PathologyKind::HarnessPanicked {
            message: "worker thread disconnected without reporting an outcome".to_string(),
        }),
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
