// Copyright (c) 2026 Mike Grier
//! The session: two bounded rings, one registry, and one drain authority.
//!
//! # Shape
//!
//! A [`Session`] is the producing half and clones freely, so several threads can
//! submit into one session. A [`Receiver`] is the consuming half and does not
//! clone, because the completion ring's ordering guarantees are stated for one
//! observer.
//!
//! Everything a client asks of a session -- start, cancel, abandon -- enters
//! through the submission ring and is applied by the servicer. The servicer runs
//! on a thread-pool work item, mutates the registry, and hands each enumeration
//! to its own work object; it never performs a directory query itself, because a
//! query may block and the servicer is the only thing that can start or stop
//! anything.
//!
//! # Why the doorbell is not inside the shared state
//!
//! The servicer's work object is owned by the client-side handles, not by the
//! state its callback touches, and the callback reaches that state through a
//! `Weak`. If the work object lived in the shared state, a callback holding the
//! last reference would drop the work object *from inside its own callback*,
//! and `WaitForThreadpoolWorkCallbacks` would then wait for the callback that is
//! doing the waiting. Keeping ownership on the handle side means the work object
//! is always dropped by a client thread.

use std::io;
use std::os::windows::io::{BorrowedHandle, OwnedHandle};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use windows_impersonation_token_sys::ImpersonationToken;
use windows_threadpool_sys::work::ThreadpoolWork;

use crate::admission::{self, EnumerationHandle};
use crate::completion::{Completion, EnumerationId, TerminalOutcome};
use crate::completion_ring::{CompletionRing, MINIMUM_COMPLETION_CAPACITY};
use crate::error::{BeginError, SessionError, SessionFailure};
use crate::registry::{EnumerationState, Registry};
use crate::request::EnumerationRequest;
use crate::submission_ring::{AbandonSlot, ControlMessage, PushOutcome, SubmissionRing};

/// The smallest submission-ring capacity that can carry one enumeration.
///
/// Three, and each one is load-bearing: the session's standing abandon message,
/// one enumeration's reserved cancellation, and one ordinary begin. A smaller
/// ring could be built but could never start anything, which is not a bound
/// worth offering.
pub const MINIMUM_SUBMISSION_CAPACITY: usize = 3;

/// The smallest completion-ring capacity that can carry one enumeration.
///
/// Two: one reserved terminal outcome and one entry. Reservations never consume
/// the last slot, so a ring of one could not hold both.
pub const MINIMUM_COMPLETION_RING_CAPACITY: usize = MINIMUM_COMPLETION_CAPACITY;

/// State both halves of a session share.
pub(crate) struct SessionShared {
    pub(crate) completions: Arc<CompletionRing>,
    pub(crate) submissions: SubmissionRing,
    registry: Mutex<Registry>,
    next_id: AtomicU64,
}

impl SessionShared {
    fn registry(&self) -> MutexGuard<'_, Registry> {
        self.registry
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// Allocate the next identifier.
    ///
    /// Monotonic within the session, so an identifier retained past its
    /// enumeration names nothing rather than aliasing a later one.
    pub(crate) fn next_enumeration_id(&self) -> EnumerationId {
        EnumerationId::from_raw(self.next_id.fetch_add(1, Ordering::Relaxed))
    }

    /// Whether this enumeration is still registered.
    #[cfg(test)]
    pub(crate) fn contains(&self, enumeration: EnumerationId) -> bool {
        self.registry().contains(enumeration)
    }

    /// How many enumerations the session is carrying.
    pub(crate) fn registered(&self) -> usize {
        self.registry().len()
    }

    /// Service every queued control message, in order, until the ring is empty.
    ///
    /// The ring clears its own drain flag when it runs out, under the same lock
    /// a producer uses to decide whether to ring the doorbell, so this loop and
    /// the next submission cannot both conclude that the other will do the work.
    pub(crate) fn drain_submissions(&self) {
        while let Some(message) = self.submissions.take_for_service() {
            match message {
                ControlMessage::Begin(begin) => self.service_begin(*begin),
                ControlMessage::Cancel(enumeration) => self.service_cancel(enumeration),
                ControlMessage::Abandon => self.service_abandon(),
            }
        }
    }

    /// Register an admitted enumeration and hand it to its own work object.
    fn service_begin(&self, begin: crate::submission_ring::BeginMessage) {
        let enumeration = begin.enumeration;
        {
            let mut registry = self.registry();
            if !registry.is_accepting() {
                // Abandoned between admission and servicing. Releasing the
                // message's terminal slot without sending is correct: no
                // receiver remains to owe an outcome to. Done after the registry
                // lock is released, because releasing a reservation takes the
                // completion ring's lock.
                drop(registry);
                drop(begin);
                return;
            }
            registry.insert(
                enumeration,
                EnumerationState::new(begin.request, begin.token, begin.terminal),
            );
        }
        self.schedule(enumeration);
    }

    /// Stop one enumeration.
    ///
    /// A quantum in flight cannot be preempted, so this only records the
    /// intention; whichever side sees the enumeration quiescent delivers the
    /// outcome, which is what keeps exactly one terminal per enumeration.
    fn service_cancel(&self, enumeration: EnumerationId) {
        let finished = {
            let mut registry = self.registry();
            let Some(state) = registry.get_mut(enumeration) else {
                // Already finished. A cancellation that lost the race is not an
                // error: the enumeration is over either way.
                return;
            };
            state.cancelled = true;
            state.parked = false;
            if state.is_quiescent() {
                registry.remove(enumeration)
            } else {
                None
            }
        };
        // Outside the registry lock: delivering a terminal takes the completion
        // ring's lock, and dropping a work object waits for its callbacks.
        if let Some(state) = finished {
            finish(state, TerminalOutcome::Cancelled);
        }
    }

    /// Tear the session down because its receiver is gone.
    ///
    /// No terminal outcomes are delivered, because nothing remains to observe
    /// them; the reserved slots are simply released.
    fn service_abandon(&self) {
        let abandoned = {
            let mut registry = self.registry();
            registry.stop_accepting();
            registry.drain_all()
        };
        // Dropping each state releases its terminal reservation and waits out
        // any callback its work object still has in flight, so it happens
        // outside the registry lock.
        drop(abandoned);
    }

    /// Submit one enumeration's work, if the engine has installed it.
    ///
    /// The native engine (M6) is what installs a work object; until then this
    /// clears the parked flag and has nothing to submit, so an admitted
    /// enumeration is registered and cancellable but produces no entries.
    pub(crate) fn schedule(&self, enumeration: EnumerationId) {
        let mut registry = self.registry();
        let Some(state) = registry.get_mut(enumeration) else {
            return;
        };
        state.parked = false;
        if let Some(work) = state.work.as_ref() {
            work.submit();
        }
    }

    /// Resume every enumeration that stopped for want of completion-ring room.
    ///
    /// Called after a receiver takes a record, which is the only event that can
    /// create that room.
    pub(crate) fn resume_parked(&self) {
        let parked = self.registry().parked();
        for enumeration in parked {
            self.schedule(enumeration);
        }
    }
}

/// Deliver one enumeration's outcome and release everything it held.
///
/// Never call this from inside the enumeration's own work callback: dropping the
/// state drops that work object, whose `Drop` waits for the very callback doing
/// the dropping.
fn finish(mut state: EnumerationState, outcome: TerminalOutcome) {
    let terminal = state
        .terminal
        .take()
        .expect("a registered enumeration always holds its terminal slot");
    terminal.send(outcome);
}

/// The servicer's work object, owned by the client-side handles.
pub(crate) struct Doorbell {
    work: ThreadpoolWork,
}

impl Doorbell {
    /// Queue a drain if this submission is the one that must schedule it.
    pub(crate) fn ring_if_needed(&self, outcome: PushOutcome) {
        if outcome == PushOutcome::RingDoorbell {
            self.work.submit();
        }
    }
}

/// The producing half of a session.
///
/// Clone it to submit from several threads; every clone feeds the same
/// submission ring and the same receiver.
pub struct Session {
    pub(crate) shared: Arc<SessionShared>,
    doorbell: Arc<Doorbell>,
}

impl Session {
    /// Build a session and its receiver.
    ///
    /// `submission_capacity` bounds outstanding control messages and
    /// `completion_capacity` bounds undelivered entries and outcomes. Both are
    /// hard bounds: the session never allocates past them in response to load.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] if either capacity is below the minimum that can
    /// carry one enumeration, or if the thread pool refused to create the
    /// servicer's work object.
    pub fn new(
        submission_capacity: usize,
        completion_capacity: usize,
    ) -> Result<(Session, Receiver), SessionError> {
        if submission_capacity < MINIMUM_SUBMISSION_CAPACITY {
            return Err(SessionError::new(
                SessionFailure::SubmissionCapacityTooSmall,
            ));
        }
        if completion_capacity < MINIMUM_COMPLETION_RING_CAPACITY {
            return Err(SessionError::new(
                SessionFailure::CompletionCapacityTooSmall,
            ));
        }

        let shared = Arc::new(SessionShared {
            completions: Arc::new(CompletionRing::new(completion_capacity)),
            submissions: SubmissionRing::new(submission_capacity),
            registry: Mutex::new(Registry::new()),
            next_id: AtomicU64::new(1),
        });

        // The callback holds only a `Weak`, so the servicer never keeps the
        // session alive and never becomes the owner that drops its own work
        // object.
        let weak: Weak<SessionShared> = Arc::downgrade(&shared);
        let work = ThreadpoolWork::new(
            move || {
                if let Some(shared) = weak.upgrade() {
                    shared.drain_submissions();
                }
            },
            None,
        )
        .map_err(|error| SessionError::with_source(SessionFailure::WorkObject, error))?;
        let doorbell = Arc::new(Doorbell { work });

        // Claimed now, not at receiver drop: at drop there is nowhere to report
        // that the ring had no room, and abandonment must never be the message
        // that could not be sent. The minimum capacity guarantees room on a
        // freshly built ring.
        let abandon = shared
            .submissions
            .reserve_abandon()
            .expect("a fresh submission ring always has room for the abandon slot");

        let receiver = Receiver {
            shared: Arc::clone(&shared),
            doorbell: Arc::clone(&doorbell),
            abandon: Some(abandon),
        };
        Ok((Session { shared, doorbell }, receiver))
    }

    /// The submission ring's bound.
    #[must_use]
    pub fn submission_capacity(&self) -> usize {
        self.shared.submissions.capacity()
    }

    /// The completion ring's bound.
    #[must_use]
    pub fn completion_capacity(&self) -> usize {
        self.shared.completions.capacity()
    }

    /// How many enumerations this session is currently carrying.
    #[must_use]
    pub fn enumerations(&self) -> usize {
        self.shared.registered()
    }

    /// Whether the receiver has abandoned this session, so no further
    /// enumeration can be started.
    #[must_use]
    pub fn is_abandoned(&self) -> bool {
        self.shared.submissions.is_abandoned()
    }

    /// Start enumerating one directory under the caller's own security context.
    ///
    /// The context is captured synchronously, here, before the request becomes
    /// visible to the session. The directory is opened later on a thread-pool
    /// worker, whose own identity is unrelated, so capturing at submission is
    /// what makes the open happen as whoever asked for it.
    ///
    /// Returns immediately with an affine [`EnumerationHandle`]. Dropping that
    /// handle cancels the enumeration; [`EnumerationHandle::detach`] lets it run
    /// to completion instead.
    ///
    /// # Errors
    ///
    /// Returns [`BeginError`] when the caller's context cannot be captured, when
    /// either ring cannot secure the room this enumeration would need, or when
    /// the receiver has already abandoned the session. Nothing is accepted in
    /// any of those cases, and the request comes back with the error.
    pub fn try_begin(&self, request: EnumerationRequest) -> Result<EnumerationHandle, BeginError> {
        admission::try_begin(&self.shared, &self.doorbell, request)
    }

    /// Start enumerating one directory under an already-captured context.
    ///
    /// This is the form a traversal layer wants: capture once when the traversal
    /// is submitted, then reuse that one context for every directory in the
    /// tree, rather than re-capturing per directory on whatever thread happens
    /// to be submitting.
    ///
    /// # Errors
    ///
    /// As [`try_begin`](Self::try_begin), except that no capture is attempted
    /// and so [`BeginFailure::TokenCapture`](crate::BeginFailure::TokenCapture)
    /// cannot occur.
    pub fn try_begin_with_token(
        &self,
        request: EnumerationRequest,
        token: ImpersonationToken,
    ) -> Result<EnumerationHandle, BeginError> {
        admission::try_begin_with_token(&self.shared, &self.doorbell, request, token)
    }
}

impl Clone for Session {
    fn clone(&self) -> Self {
        self.shared.completions.add_session();
        Self {
            shared: Arc::clone(&self.shared),
            doorbell: Arc::clone(&self.doorbell),
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // One fewer producer. When the last one goes and nothing is still
        // enumerating, the receiver learns the stream has ended rather than
        // blocking on a record that can never arrive.
        self.shared.completions.remove_session();
    }
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("submission_capacity", &self.submission_capacity())
            .field("completion_capacity", &self.completion_capacity())
            .field("enumerations", &self.enumerations())
            .finish_non_exhaustive()
    }
}

/// The consuming half of a session: the only way to observe completions.
///
/// Not clonable. The ordering the completion ring promises -- every entry of one
/// enumeration before its terminal -- is a statement about one observer, and two
/// receivers racing on the same ring would each see an arbitrary subsequence of
/// it.
/// Dropping the receiver abandons the session: the session stops accepting
/// enumerations and releases the ones it is carrying, without delivering any
/// terminal outcome, because no observer remains to owe one to.
pub struct Receiver {
    shared: Arc<SessionShared>,
    doorbell: Arc<Doorbell>,
    /// The standing abandon reservation, claimed when the session was built so
    /// that `Drop` has nowhere to fail.
    abandon: Option<AbandonSlot>,
}

impl Receiver {
    /// Take the next record if one is already queued.
    #[must_use]
    pub fn try_recv(&self) -> Option<Completion> {
        let record = self.shared.completions.try_take();
        if record.is_some() {
            // Taking a record is the only thing that creates room, so it is
            // also the only thing that can un-park a backpressured enumeration.
            self.shared.resume_parked();
        }
        record
    }

    /// Block until a record is available, or until the stream ends.
    ///
    /// Returns `None` only when nothing is queued, no session handle remains,
    /// and no enumeration is still outstanding.
    #[must_use]
    pub fn recv(&self) -> Option<Completion> {
        let record = self.shared.completions.take_blocking(None);
        if record.is_some() {
            self.shared.resume_parked();
        }
        record
    }

    /// Block for at most `timeout`.
    ///
    /// Returns `None` on timeout as well as at the end of the stream; a caller
    /// that must tell them apart can check [`is_disconnected`](Self::is_disconnected).
    #[must_use]
    pub fn recv_timeout(&self, timeout: Duration) -> Option<Completion> {
        let record = self.shared.completions.take_blocking(Some(timeout));
        if record.is_some() {
            self.shared.resume_parked();
        }
        record
    }

    /// A manual-reset event, signalled exactly while this receiver has something
    /// to take.
    ///
    /// This is what lets a client integrate with its own thread pool instead of
    /// dedicating a thread to [`recv`](Self::recv): wait on the handle, then
    /// drain with [`try_recv`](Self::try_recv) until it yields `None`. It stays
    /// signalled once the stream has ended, so a waiter learns about that too.
    ///
    /// The event is created on the first call, so a client that never asks for
    /// it pays for no kernel object. The borrow is deliberate: the event belongs
    /// to the ring and must not be closed by a caller.
    ///
    /// # Errors
    ///
    /// Returns the error from `CreateEventW` on the first call.
    pub fn doorbell(&self) -> io::Result<BorrowedHandle<'_>> {
        self.shared.completions.doorbell()
    }

    /// A duplicate of [`doorbell`](Self::doorbell) that the caller owns, as
    /// arming a `ThreadpoolWait` requires.
    ///
    /// # Errors
    ///
    /// Returns the error from `CreateEventW` or `DuplicateHandle`.
    pub fn doorbell_owned(&self) -> io::Result<OwnedHandle> {
        self.shared.completions.doorbell_owned()
    }

    /// Whether the stream has ended.
    #[must_use]
    pub fn is_disconnected(&self) -> bool {
        self.shared.completions.is_closed()
    }

    /// How many records are queued right now.
    #[must_use]
    pub fn len(&self) -> usize {
        self.shared.completions.len()
    }

    /// Whether nothing is queued right now.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The completion ring's bound.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.shared.completions.capacity()
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        // Infallible by construction: the slot was claimed when the session was
        // built precisely so this path cannot fail. Ringing the doorbell here is
        // what makes abandonment asynchronous -- `Drop` never blocks on the
        // teardown it starts.
        if let Some(slot) = self.abandon.take() {
            let outcome = self.shared.submissions.push_abandon(slot);
            self.doorbell.ring_if_needed(outcome);
        }
    }
}

impl std::fmt::Debug for Receiver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Receiver")
            .field("queued", &self.len())
            .field("capacity", &self.capacity())
            .field("disconnected", &self.is_disconnected())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
