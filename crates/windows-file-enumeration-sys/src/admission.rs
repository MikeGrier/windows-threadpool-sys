// Copyright (c) 2026 Mike Grier
//! Admission: the point at which an enumeration becomes the session's problem.
//!
//! # Everything that can fail, fails here
//!
//! Admission is where a request stops being the caller's and starts being the
//! session's, so it is also the last place a failure can be reported to the
//! caller's own thread. Three things are therefore secured *before* the begin
//! message becomes visible to the servicer:
//!
//! 1. the submitter's security context, captured synchronously;
//! 2. a completion-ring slot for the outcome the enumeration will owe; and
//! 3. a submission-ring slot for the cancellation it may later need.
//!
//! Once all three are in hand the begin cannot be half-accepted: either it
//! enters the ring and the session owes exactly one terminal, or it is refused
//! and the caller gets its request -- and its captured token -- back.
//!
//! # Why the token is captured by the submitter
//!
//! The directory is opened on a thread-pool worker, whose own identity is
//! whatever the pool last left there. Capturing at submission and carrying the
//! context in the message is what makes the open happen under the identity of
//! whoever asked for it. Capturing later, on the worker, would silently
//! enumerate under the process or worker identity -- the exact defect this layer
//! exists to prevent.
//!
//! A second form takes an already-captured context, so a traversal layer can
//! capture once for a whole tree instead of once per directory.
//!
//! # Why the handle is affine
//!
//! An [`EnumerationHandle`] owns its enumeration's cancellation reservation, so
//! cancelling never has to find room in a ring that ordinary traffic may have
//! filled -- and `Drop` never has to report a failure it has nowhere to report.
//! Dropping the handle therefore cancels. A caller that wants an enumeration to
//! outlive its handle says so with [`EnumerationHandle::detach`], which gives
//! the reservation back rather than spending it.

use windows_impersonation_token_sys::{CaptureError, ImpersonationToken};

use crate::completion::EnumerationId;
use crate::error::{BeginError, BeginFailure};
use crate::request::EnumerationRequest;
use crate::session::{Doorbell, SessionShared};
use crate::submission_ring::{
    BeginMessage, CancelSlot, ControlMessage, SubmitRejection, release_cancel_slot,
};
use std::sync::Arc;

/// A live enumeration's affine handle.
///
/// Holding one is what keeps an enumeration running: dropping it asks the
/// session to stop that enumeration. The handle is not clonable and not
/// copyable, so exactly one owner decides when the enumeration ends.
///
/// Cancellation is asynchronous. It enters the submission ring like every other
/// control operation and takes effect when the servicer reaches it, so entries
/// already queued still arrive and exactly one terminal outcome still follows
/// them.
#[must_use = "dropping the handle cancels the enumeration; use `detach` to let it run"]
pub struct EnumerationHandle {
    enumeration: EnumerationId,
    shared: Arc<SessionShared>,
    doorbell: Arc<Doorbell>,
    /// Taken at admission, spent by exactly one of cancel, detach, or drop.
    cancel: Option<CancelSlot>,
}

impl EnumerationHandle {
    pub(crate) fn new(
        enumeration: EnumerationId,
        shared: Arc<SessionShared>,
        doorbell: Arc<Doorbell>,
        cancel: CancelSlot,
    ) -> Self {
        Self {
            enumeration,
            shared,
            doorbell,
            cancel: Some(cancel),
        }
    }

    /// Which enumeration this handle controls.
    ///
    /// Completion records carry the same identifier, which is how a caller
    /// attributes them when several enumerations share one session.
    #[must_use]
    pub fn id(&self) -> EnumerationId {
        self.enumeration
    }

    /// Ask the session to stop this enumeration.
    ///
    /// Returns immediately. Cancellation cannot preempt a directory query that
    /// is already executing, so entries produced before it is observed are still
    /// delivered, followed by one
    /// [`Cancelled`](crate::TerminalOutcome::Cancelled) terminal.
    pub fn cancel(mut self) {
        self.enqueue_cancel();
    }

    /// Give up control of this enumeration and let it run to completion.
    ///
    /// The cancellation reservation returns to the submission ring, so a
    /// detached enumeration costs the session nothing beyond its terminal slot.
    /// It still reports its outcome; a caller simply loses the ability to stop
    /// it early.
    pub fn detach(mut self) {
        if let Some(slot) = self.cancel.take() {
            release_cancel_slot(&self.shared.submissions, slot);
        }
    }

    /// Spend the reservation on a cancellation, if it has not been spent.
    fn enqueue_cancel(&mut self) {
        let Some(slot) = self.cancel.take() else {
            return;
        };
        let outcome = self.shared.submissions.push_cancel(slot, self.enumeration);
        self.doorbell.ring_if_needed(outcome);
    }
}

impl Drop for EnumerationHandle {
    fn drop(&mut self) {
        // Infallible by construction: the reservation was taken at admission
        // precisely so that this path has nowhere to fail.
        self.enqueue_cancel();
    }
}

impl std::fmt::Debug for EnumerationHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnumerationHandle")
            .field("enumeration", &self.enumeration)
            .field("cancellable", &self.cancel.is_some())
            .finish_non_exhaustive()
    }
}

/// Admit one request, capturing the caller's current security context.
pub(crate) fn try_begin(
    shared: &Arc<SessionShared>,
    doorbell: &Arc<Doorbell>,
    request: EnumerationRequest,
) -> Result<EnumerationHandle, BeginError> {
    // Captured before anything else is claimed, so a capture failure costs no
    // reservations and leaves the rings exactly as they were.
    let token = match ImpersonationToken::capture() {
        Ok(token) => token,
        Err(error) => return Err(BeginError::capture(request, error)),
    };
    try_begin_with_token(shared, doorbell, request, token)
}

/// Admit one request under an already-captured security context.
pub(crate) fn try_begin_with_token(
    shared: &Arc<SessionShared>,
    doorbell: &Arc<Doorbell>,
    request: EnumerationRequest,
    token: ImpersonationToken,
) -> Result<EnumerationHandle, BeginError> {
    if shared.submissions.is_abandoned() {
        return Err(BeginError::rejected(
            BeginFailure::Abandoned,
            request,
            Some(token),
        ));
    }

    let Some(cancel) = shared.submissions.reserve_cancel() else {
        return Err(BeginError::rejected(
            BeginFailure::SubmissionRingFull,
            request,
            Some(token),
        ));
    };
    let enumeration = shared.next_enumeration_id();
    let Some(terminal) = shared.completions.reserve_terminal(enumeration) else {
        release_cancel_slot(&shared.submissions, cancel);
        return Err(BeginError::rejected(
            BeginFailure::CompletionRingFull,
            request,
            Some(token),
        ));
    };

    let message = ControlMessage::Begin(Box::new(BeginMessage {
        enumeration,
        request,
        token,
        terminal,
    }));
    match shared.submissions.try_push(message) {
        Ok(outcome) => {
            doorbell.ring_if_needed(outcome);
            Ok(EnumerationHandle::new(
                enumeration,
                Arc::clone(shared),
                Arc::clone(doorbell),
                cancel,
            ))
        }
        Err((message, rejection)) => {
            // Nothing was accepted, so every claim made above is given back and
            // the caller's request and token are returned intact. Dropping the
            // message releases its terminal reservation.
            release_cancel_slot(&shared.submissions, cancel);
            let (request, token) = match message {
                ControlMessage::Begin(begin) => {
                    let begin = *begin;
                    (begin.request, begin.token)
                }
                _ => unreachable!("the message pushed above is always a begin"),
            };
            let failure = match rejection {
                SubmitRejection::Full => BeginFailure::SubmissionRingFull,
                SubmitRejection::Abandoned => BeginFailure::Abandoned,
            };
            Err(BeginError::rejected(failure, request, Some(token)))
        }
    }
}

/// The capture error behind a [`BeginFailure::TokenCapture`], re-exported so a
/// caller can inspect it without depending on the sibling crate by name.
pub type TokenCaptureError = CaptureError;

#[cfg(test)]
mod tests;
