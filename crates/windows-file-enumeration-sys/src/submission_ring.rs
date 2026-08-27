// Copyright (c) 2026 Mike Grier
//! The bounded submission ring: every control operation's one way in.
//!
//! Begin, cancel, and abandon all travel this ring, in order, and are applied by
//! one logical drain authority. That single ingress is what makes the session's
//! state machine tractable: there is no second path by which an enumeration can
//! be started or stopped, so ordering between control operations is decided in
//! exactly one place.
//!
//! # Ordinary traffic can fail; control traffic cannot
//!
//! A begin is ordinary traffic. It is offered to the ring and rejected
//! synchronously if the ring is full, on the caller's own thread, where a caller
//! can back off or shed load.
//!
//! Cancellation is not ordinary traffic. A cancel that could fail because
//! *unrelated* begins had filled the ring would make cancellation unreliable
//! precisely when a session is busiest -- and the affine handle's `Drop` has
//! nowhere to report a failure to anyway. So each accepted enumeration carries a
//! [`CancelSlot`] reserved at admission, and the session carries one standing
//! [`AbandonSlot`] for receiver drop. Both send infallibly.
//!
//! # The doorbell coalesces
//!
//! Producers do not drain. They ring a `ThreadpoolWork` doorbell, and the flag
//! that says a drain is already scheduled or running is what keeps a burst of
//! submissions from queueing a burst of empty drains behind it.

use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard};

use windows_impersonation_token_sys::ImpersonationToken;

use crate::completion::EnumerationId;
use crate::completion_ring::TerminalSlot;
use crate::request::EnumerationRequest;

/// Everything an accepted begin carries into the session.
///
/// The terminal slot travels with it because admission is where the room for
/// the outcome is claimed: by the time the servicer sees this message, the
/// enumeration's ability to report how it ended is already guaranteed.
pub(crate) struct BeginMessage {
    pub(crate) enumeration: EnumerationId,
    pub(crate) request: EnumerationRequest,
    pub(crate) token: ImpersonationToken,
    pub(crate) terminal: TerminalSlot,
}

impl std::fmt::Debug for BeginMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Neither the captured token nor the reserved slot has anything a
        // reader wants here, and the token deliberately hides its handle.
        f.debug_struct("BeginMessage")
            .field("enumeration", &self.enumeration)
            .field("path", &self.request.path().to_string_lossy())
            .finish_non_exhaustive()
    }
}

/// One control operation.
#[derive(Debug)]
pub(crate) enum ControlMessage {
    /// Start an already-admitted enumeration.
    Begin(Box<BeginMessage>),
    /// Stop one enumeration, whether or not it has started.
    Cancel(EnumerationId),
    /// The receiver is gone: reject further starts and stop everything.
    Abandon,
}

/// The bounded ring and the drain flag that serialises its servicing.
pub(crate) struct SubmissionRing {
    state: Mutex<RingState>,
}

struct RingState {
    queue: VecDeque<ControlMessage>,
    capacity: usize,
    /// Slots claimed by cancel and abandon reservations but not yet filled.
    reserved: usize,
    /// Whether a drain is scheduled or running. The coalescing flag.
    draining: bool,
    /// Set once the receiver has abandoned the session; further begins are
    /// refused without needing to consult anything else.
    abandoned: bool,
}

impl RingState {
    /// Slots available to ordinary traffic.
    fn free(&self) -> usize {
        self.capacity - self.queue.len() - self.reserved
    }
}

/// Why an ordinary submission was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum SubmitRejection {
    /// The ring had no room for ordinary traffic.
    Full,
    /// The receiver has abandoned the session.
    Abandoned,
}

impl SubmissionRing {
    /// Build a ring holding at most `capacity` control messages.
    ///
    /// # Panics
    ///
    /// Panics on a zero capacity; callers validate and report that as an error
    /// before constructing a session.
    pub(crate) fn new(capacity: usize) -> Self {
        assert!(
            capacity > 0,
            "a submission ring must hold at least one message"
        );
        Self {
            state: Mutex::new(RingState {
                queue: VecDeque::new(),
                capacity,
                reserved: 0,
                draining: false,
                abandoned: false,
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, RingState> {
        self.state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// The ring's bound.
    pub(crate) fn capacity(&self) -> usize {
        self.lock().capacity
    }

    /// How many messages are queued right now.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.lock().queue.len()
    }

    /// Whether the receiver has abandoned the session.
    pub(crate) fn is_abandoned(&self) -> bool {
        self.lock().abandoned
    }

    /// Claim the slot an enumeration's future cancellation will use.
    ///
    /// Returns `None` when the ring cannot spare one. Taking it at admission is
    /// what lets cancellation -- including the affine handle's `Drop` -- proceed
    /// without a failure path.
    pub(crate) fn reserve_cancel(&self) -> Option<CancelSlot> {
        let mut state = self.lock();
        if state.free() == 0 {
            return None;
        }
        state.reserved += 1;
        Some(CancelSlot { _private: () })
    }

    /// Claim the session's one standing abandon slot.
    ///
    /// Taken when the session is built rather than when the receiver drops,
    /// because at drop time there is no way to report that there was no room.
    pub(crate) fn reserve_abandon(&self) -> Option<AbandonSlot> {
        let mut state = self.lock();
        if state.free() == 0 {
            return None;
        }
        state.reserved += 1;
        Some(AbandonSlot { _private: () })
    }

    /// Offer an ordinary message.
    ///
    /// # Errors
    ///
    /// Returns the message unchanged alongside the reason, so a rejected begin
    /// gives its request and captured token back to the caller for retry.
    pub(crate) fn try_push(
        &self,
        message: ControlMessage,
    ) -> Result<PushOutcome, (ControlMessage, SubmitRejection)> {
        let mut state = self.lock();
        if state.abandoned {
            return Err((message, SubmitRejection::Abandoned));
        }
        if state.free() == 0 {
            return Err((message, SubmitRejection::Full));
        }
        state.queue.push_back(message);
        Ok(claim_drain(&mut state))
    }

    /// Enqueue a cancellation into its reserved slot. Cannot fail.
    ///
    /// Takes the slot by value, so a reservation is spent exactly once and an
    /// unspent one is still its owner's to return.
    pub(crate) fn push_cancel(&self, _slot: CancelSlot, enumeration: EnumerationId) -> PushOutcome {
        let mut state = self.lock();
        state.reserved -= 1;
        state.queue.push_back(ControlMessage::Cancel(enumeration));
        claim_drain(&mut state)
    }

    /// Enqueue abandonment into the standing slot, and latch the flag that
    /// refuses further begins. Cannot fail.
    pub(crate) fn push_abandon(&self, _slot: AbandonSlot) -> PushOutcome {
        let mut state = self.lock();
        state.reserved -= 1;
        state.abandoned = true;
        state.queue.push_back(ControlMessage::Abandon);
        claim_drain(&mut state)
    }

    /// Take the next message, or clear the drain flag when there is none.
    ///
    /// Clearing the flag here, under the same lock the queue is read with, is
    /// what makes the coalescing sound: a producer either sees the flag still
    /// set (and a drain that has not yet finished will reach its message) or
    /// sees it cleared and schedules a fresh drain.
    pub(crate) fn take_for_service(&self) -> Option<ControlMessage> {
        let mut state = self.lock();
        match state.queue.pop_front() {
            Some(message) => Some(message),
            None => {
                state.draining = false;
                None
            }
        }
    }

    /// Release a cancel reservation whose enumeration ended without using it.
    fn release_cancel(&self) {
        self.lock().reserved -= 1;
    }
}

/// Mark a drain as scheduled if one is not already, reporting whether the
/// caller now owes a doorbell ring.
fn claim_drain(state: &mut RingState) -> PushOutcome {
    if state.draining {
        PushOutcome::DrainAlreadyScheduled
    } else {
        state.draining = true;
        PushOutcome::RingDoorbell
    }
}

/// What a producer must do after a successful enqueue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[must_use = "a queued message is not serviced until the doorbell is rung"]
pub(crate) enum PushOutcome {
    /// Submit the drain work item.
    RingDoorbell,
    /// A drain is already scheduled or running and will reach this message.
    DrainAlreadyScheduled,
}

/// A claimed slot for one enumeration's future cancellation.
///
/// Owned by the enumeration's affine handle, so cancelling -- explicitly or by
/// dropping the handle -- always has somewhere to go. Spending it consumes the
/// value, so a slot cannot be spent twice and an unspent one is unambiguously
/// still its owner's.
pub(crate) struct CancelSlot {
    /// Not a unit struct: an empty tuple would be constructible anywhere, and a
    /// reservation must only ever come from the ring that accounted for it.
    _private: (),
}

/// A claimed slot for the session's one abandon message.
pub(crate) struct AbandonSlot {
    _private: (),
}

/// Return an unspent reservation to the ring.
///
/// The slots cannot do this from their own `Drop`, because they deliberately do
/// not hold a share of the ring: a `CancelSlot` lives in the affine handle and
/// an `AbandonSlot` in the receiver, and giving either one a strong reference
/// back would keep the session alive past the last handle that should own it.
/// The owner therefore returns the slot explicitly.
pub(crate) fn release_cancel_slot(ring: &SubmissionRing, _slot: CancelSlot) {
    ring.release_cancel();
}

#[cfg(test)]
mod tests;
