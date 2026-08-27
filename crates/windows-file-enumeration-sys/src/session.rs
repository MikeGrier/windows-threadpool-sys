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
//! through the submission ring and is applied by the servicer. The servicer
//! mutates the registry and marks enumerations runnable; it never performs a
//! directory query itself, because a query may block and the servicer is the
//! only thing that can start or stop anything.
//!
//! # A worker reports; the servicer applies
//!
//! Work runs on a second thread-pool object, whose callback claims one runnable
//! enumeration and runs one quantum. That worker delivers entries and its own
//! terminal to the completion ring and then *reports* retirement through the
//! submission ring. It never removes a registry entry.
//!
//! That is not tidiness. If a worker finished its own enumeration it would drop
//! that enumeration's state, and any thread-pool object living in that state
//! would be closed from inside its own callback -- which waits for the callback
//! doing the waiting and then frees the closure still running. Reporting instead
//! of acting removes the hazard, and keeping no thread-pool object per
//! enumeration removes it structurally: abandonment releases entries that own
//! nothing the pool must be drained for.
//!
//! # Who owns the pool objects
//!
//! The shared state owns them, and the *last client handle* releases them on its
//! own thread before letting go of its share of that state. A callback therefore
//! never holds the last reference to anything whose release would wait on that
//! callback: by the time the shared state can be dropped, the pool objects are
//! already closed.

use std::io;
use std::os::windows::io::{BorrowedHandle, OwnedHandle};
#[cfg(test)]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use windows_impersonation_token_sys::ImpersonationToken;
use windows_threadpool_sys::callback_env::CallbackEnviron;
use windows_threadpool_sys::work::ThreadpoolWork;

use crate::admission::{self, EnumerationHandle};
use crate::completion::{Completion, EnumerationId, TerminalOutcome};
use crate::completion_ring::{CompletionRing, MINIMUM_COMPLETION_CAPACITY};
use crate::error::{BeginError, SessionError, SessionFailure};
use crate::registry::{EnumerationState, Registry};
use crate::request::EnumerationRequest;
use crate::submission_ring::{
    AbandonSlot, ControlMessage, PushOutcome, SubmissionRing, release_retire_slot,
};

/// The smallest submission-ring capacity that can carry one enumeration.
///
/// Four, and each one is load-bearing: the session's standing abandon message,
/// one enumeration's reserved cancellation, its reserved retirement report, and
/// one ordinary begin. A smaller ring could be built but could never start
/// anything, which is not a bound worth offering.
pub const MINIMUM_SUBMISSION_CAPACITY: usize = 4;

/// The smallest completion-ring capacity that can carry one enumeration.
///
/// Two: one reserved terminal outcome and one entry. Reservations never consume
/// the last slot, so a ring of one could not hold both.
pub const MINIMUM_COMPLETION_RING_CAPACITY: usize = MINIMUM_COMPLETION_CAPACITY;

/// What one quantum of work decided.
///
/// Only [`Idle`](Self::Idle) is produced today: [`advance`](SessionShared::advance)
/// has no engine behind it until FE-8, which is what decides the other two.
#[derive(Debug)]
#[allow(
    dead_code,
    reason = "FE-8's native engine is what parks and finishes an enumeration"
)]
pub(crate) enum QuantumOutcome {
    /// Nothing to do. The enumeration stays registered and idle.
    Idle,
    /// Stopped for want of completion-ring room; resume on consumer progress.
    Parked,
    /// The enumeration is over, with this outcome.
    Finished(TerminalOutcome),
}

/// The session's thread-pool objects.
///
/// Two, deliberately: the servicer must stay responsive, so it is not the object
/// marked as running long. Only the engine may block on a directory query.
struct SessionWork {
    servicer: ThreadpoolWork,
    engine: ThreadpoolWork,
    /// Set by the state-machine model, which drives both callbacks on its own
    /// thread so a scenario decides exactly when they run.
    #[cfg(test)]
    suppressed: AtomicBool,
}

impl SessionWork {
    #[cfg(test)]
    fn is_suppressed(&self) -> bool {
        self.suppressed.load(Ordering::Acquire)
    }

    #[cfg(not(test))]
    fn is_suppressed(&self) -> bool {
        false
    }

    /// Queue one drain of the submission ring.
    fn submit_servicer(&self) {
        if !self.is_suppressed() {
            self.servicer.submit();
        }
    }

    /// Queue one quantum of enumeration work.
    fn submit_engine(&self) {
        if !self.is_suppressed() {
            self.engine.submit();
        }
    }
}

/// State both halves of a session share.
pub(crate) struct SessionShared {
    pub(crate) completions: Arc<CompletionRing>,
    pub(crate) submissions: SubmissionRing,
    registry: Mutex<Registry>,
    next_id: AtomicU64,
    /// The pool objects, taken and dropped by the last client handle.
    ///
    /// `None` once a session has been torn down, after which nothing further is
    /// scheduled -- which is correct, because nothing is left to observe it.
    work: Mutex<Option<SessionWork>>,
    /// Live [`Session`] and [`Receiver`] handles. Not the `Arc` strong count,
    /// which a callback transiently inflates.
    handles: AtomicUsize,
    /// Quantum outcomes the state-machine model has scripted, standing in for
    /// the native engine.
    #[cfg(test)]
    scripted: Mutex<std::collections::VecDeque<QuantumOutcome>>,
}

impl SessionShared {
    fn registry(&self) -> MutexGuard<'_, Registry> {
        self.registry
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    fn work(&self) -> MutexGuard<'_, Option<SessionWork>> {
        self.work
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

    /// How many enumerations are waiting for a worker.
    #[cfg(test)]
    pub(crate) fn ready(&self) -> usize {
        self.registry().ready_len()
    }

    /// Queue a drain if this submission is the one that must schedule it.
    pub(crate) fn ring_servicer(&self, outcome: PushOutcome) {
        if outcome != PushOutcome::RingDoorbell {
            return;
        }
        if let Some(work) = self.work().as_ref() {
            work.submit_servicer();
        }
    }

    /// Note one more client handle.
    fn acquire_handle(&self) {
        self.handles.fetch_add(1, Ordering::AcqRel);
    }

    /// Release one client handle, tearing the session's pool objects down when
    /// it was the last.
    ///
    /// Runs on whichever thread dropped the handle -- always a client thread,
    /// never a callback -- which is exactly the precondition for waiting out
    /// in-flight callbacks.
    fn release_handle(&self) {
        if self.handles.fetch_sub(1, Ordering::AcqRel) != 1 {
            return;
        }
        // Taken under the lock and dropped outside it, so a callback blocked on
        // this lock cannot be what the drop is waiting for.
        let work = self.work().take();
        drop(work);
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
                ControlMessage::Retire(enumeration) => self.service_retire(enumeration),
                ControlMessage::Abandon => self.service_abandon(),
            }
        }
    }

    /// Register an admitted enumeration and make it runnable.
    fn service_begin(&self, begin: crate::submission_ring::BeginMessage) {
        let enumeration = begin.enumeration;
        {
            let mut registry = self.registry();
            if !registry.is_accepting() {
                // Abandoned between admission and servicing. Releasing the
                // message's slots without spending them is correct: no receiver
                // remains to owe an outcome to. Done after the registry lock is
                // released, because releasing takes the other rings' locks.
                drop(registry);
                self.retire_state(
                    EnumerationState::new(begin.request, begin.token, begin.terminal, begin.retire),
                    None,
                );
                return;
            }
            registry.insert(
                enumeration,
                EnumerationState::new(begin.request, begin.token, begin.terminal, begin.retire),
            );
        }
        self.schedule(enumeration);
    }

    /// Stop one enumeration.
    ///
    /// A quantum in flight cannot be preempted, so this only records the
    /// intention; the worker holding it applies the outcome when it reports.
    /// Only a quiescent enumeration is finished here, which is what keeps
    /// exactly one terminal per enumeration.
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
        if let Some(state) = finished {
            self.retire_state(state, Some(TerminalOutcome::Cancelled));
        }
    }

    /// Release an enumeration whose worker has reported itself finished.
    ///
    /// The terminal was already delivered by that worker, so nothing is owed
    /// here; this returns what the entry still holds.
    fn service_retire(&self, enumeration: EnumerationId) {
        let state = self.registry().remove(enumeration);
        if let Some(state) = state {
            self.retire_state(state, None);
        }
    }

    /// Tear the session down because its receiver is gone.
    ///
    /// No terminal outcomes are delivered, because nothing remains to observe
    /// them; the reserved slots are simply released. Nothing here waits on a
    /// worker, because a registry entry owns no thread-pool object.
    fn service_abandon(&self) {
        let abandoned = {
            let mut registry = self.registry();
            registry.stop_accepting();
            registry.drain_all()
        };
        for (_, state) in abandoned {
            self.retire_state(state, None);
        }
    }

    /// Release everything a removed entry still holds, delivering `outcome` if
    /// one is still owed.
    ///
    /// An unspent slot of either kind returns to its ring rather than being
    /// leaked; a terminal slot dropped without an outcome releases its
    /// completion-ring reservation.
    fn retire_state(&self, mut state: EnumerationState, outcome: Option<TerminalOutcome>) {
        if let Some(retire) = state.retire.take() {
            release_retire_slot(&self.submissions, retire);
        }
        match (state.terminal.take(), outcome) {
            (Some(terminal), Some(outcome)) => terminal.send(outcome),
            (slot, _) => drop(slot),
        }
    }

    /// Make one enumeration runnable and ask for a worker.
    pub(crate) fn schedule(&self, enumeration: EnumerationId) {
        self.registry().mark_ready(enumeration);
        if let Some(work) = self.work().as_ref() {
            work.submit_engine();
        }
    }

    /// Run one quantum for one runnable enumeration.
    ///
    /// This is the engine callback's whole body. Claiming is single-flight, so
    /// an enumeration already held by another worker is skipped rather than run
    /// twice over the same buffer and cursor.
    pub(crate) fn run_engine_quantum(&self) {
        let Some(enumeration) = self.claim_next() else {
            return;
        };
        let outcome = self.advance(enumeration);
        self.report_quantum(enumeration, outcome);
    }

    /// Claim the next runnable enumeration for this worker.
    pub(crate) fn claim_next(&self) -> Option<EnumerationId> {
        self.registry().claim_next()
    }

    /// Advance one enumeration by one bounded quantum.
    ///
    /// The native engine lands here in FE-8. Until then an enumeration that is
    /// not cancelled simply has nothing to do, so it is claimed, found idle, and
    /// released.
    fn advance(&self, enumeration: EnumerationId) -> QuantumOutcome {
        #[cfg(test)]
        if let Some(scripted) = self
            .scripted
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .pop_front()
        {
            return scripted;
        }
        let _ = enumeration;
        QuantumOutcome::Idle
    }

    /// Hand the claim back and apply whatever the quantum decided.
    pub(crate) fn report_quantum(&self, enumeration: EnumerationId, outcome: QuantumOutcome) {
        let finish = {
            let mut registry = self.registry();
            let Some(state) = registry.get_mut(enumeration) else {
                // Removed while this worker held it, which is what abandonment
                // does. Nothing is owed and nothing is left to release.
                return;
            };
            state.running = false;
            match outcome {
                // The worker reached a real conclusion, which wins over a
                // cancellation that arrived while it was doing so.
                QuantumOutcome::Finished(outcome) => Some(outcome),
                _ if state.cancelled => Some(TerminalOutcome::Cancelled),
                QuantumOutcome::Parked => {
                    state.parked = true;
                    None
                }
                QuantumOutcome::Idle => None,
            }
        };
        if let Some(outcome) = finish {
            self.finish_from_worker(enumeration, outcome);
        }
    }

    /// Deliver a worker's terminal and report the enumeration for retirement.
    ///
    /// The worker owns the terminal slot, so delivery cannot fail. Removing the
    /// entry is the servicer's job, which is why this reports rather than
    /// removes.
    fn finish_from_worker(&self, enumeration: EnumerationId, outcome: TerminalOutcome) {
        let (terminal, retire) = {
            let mut registry = self.registry();
            match registry.get_mut(enumeration) {
                Some(state) => (state.terminal.take(), state.retire.take()),
                None => return,
            }
        };
        if let Some(terminal) = terminal {
            terminal.send(outcome);
        }
        if let Some(retire) = retire {
            let pushed = self.submissions.push_retire(retire, enumeration);
            self.ring_servicer(pushed);
        }
    }

    /// Resume every enumeration that stopped for want of completion-ring room.
    ///
    /// Called after a receiver takes a record, which is the only event that can
    /// create that room.
    pub(crate) fn resume_parked(&self) {
        let parked = {
            let registry = self.registry();
            registry.parked()
        };
        for enumeration in parked {
            self.schedule(enumeration);
        }
    }

    /// Push a scripted quantum outcome for the state-machine model.
    #[cfg(test)]
    pub(crate) fn script_quantum(&self, outcome: QuantumOutcome) {
        self.scripted
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push_back(outcome);
    }
}

/// The producing half of a session.
///
/// Clone it to submit from several threads; every clone feeds the same
/// submission ring and the same receiver.
pub struct Session {
    pub(crate) shared: Arc<SessionShared>,
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
    /// carry one enumeration, or if the thread pool refused to create either of
    /// the session's work objects.
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
            work: Mutex::new(None),
            // The session and its receiver.
            handles: AtomicUsize::new(2),
            #[cfg(test)]
            scripted: Mutex::new(std::collections::VecDeque::new()),
        });

        // Both callbacks hold only a `Weak`, so neither keeps the session alive
        // and neither can become the owner that closes its own work object.
        let servicer = {
            let weak: Weak<SessionShared> = Arc::downgrade(&shared);
            ThreadpoolWork::new(
                move || {
                    if let Some(shared) = weak.upgrade() {
                        shared.drain_submissions();
                    }
                },
                None,
            )
            .map_err(|error| SessionError::with_source(SessionFailure::WorkObject, error))?
        };
        let engine = {
            let weak: Weak<SessionShared> = Arc::downgrade(&shared);
            // Any quantum may perform a synchronous directory query, so the pool
            // is told these callbacks can block. That accounting belongs to the
            // engine alone: the servicer must stay responsive.
            let mut environment = CallbackEnviron::new();
            environment.set_runs_long();
            ThreadpoolWork::new(
                move || {
                    if let Some(shared) = weak.upgrade() {
                        shared.run_engine_quantum();
                    }
                },
                Some(&mut environment),
            )
            .map_err(|error| SessionError::with_source(SessionFailure::WorkObject, error))?
        };
        *shared.work() = Some(SessionWork {
            servicer,
            engine,
            #[cfg(test)]
            suppressed: AtomicBool::new(false),
        });

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
            abandon: Some(abandon),
        };
        Ok((Session { shared }, receiver))
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

    /// Leave servicing entirely to explicit drains, for the state-machine
    /// model, which must decide when each step happens.
    #[cfg(test)]
    pub(crate) fn suppress_pool(&self) {
        if let Some(work) = self.shared.work().as_ref() {
            work.suppressed.store(true, Ordering::Release);
        }
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
        admission::try_begin(&self.shared, request)
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
        admission::try_begin_with_token(&self.shared, request, token)
    }
}

impl Clone for Session {
    fn clone(&self) -> Self {
        self.shared.completions.add_session();
        self.shared.acquire_handle();
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // One fewer producer. When the last one goes and nothing is still
        // enumerating, the receiver learns the stream has ended rather than
        // blocking on a record that can never arrive.
        self.shared.completions.remove_session();
        self.shared.release_handle();
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
///
/// Dropping the receiver abandons the session: the session stops accepting
/// enumerations and releases the ones it is carrying, without delivering any
/// terminal outcome, because no observer remains to owe one to.
pub struct Receiver {
    shared: Arc<SessionShared>,
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
    /// that must tell them apart can check
    /// [`is_disconnected`](Self::is_disconnected).
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
        // built precisely so this path cannot fail. Ringing rather than draining
        // is what makes abandonment asynchronous -- `Drop` never blocks on the
        // teardown it starts, unless it is also the last handle, in which case
        // releasing the pool objects necessarily waits out their callbacks.
        if let Some(slot) = self.abandon.take() {
            let pushed = self.shared.submissions.push_abandon(slot);
            self.shared.ring_servicer(pushed);
        }
        self.shared.release_handle();
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
