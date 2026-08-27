// Copyright (c) 2026 Mike Grier
//! The registry of enumerations a session is carrying, and the ready set that
//! decides which of them a worker runs next.
//!
//! # One authority removes, one authority claims
//!
//! The submission-ring servicer is the only thing that inserts or removes an
//! entry. A worker never does: it *claims* an enumeration, runs a quantum, and
//! reports. That split is what lets a worker finish an enumeration without ever
//! dropping the state -- and therefore without dropping anything whose release
//! would wait on the worker itself (D-16).
//!
//! Both live behind one lock. They are two views of the same question -- what
//! exists, and what is runnable -- and separating them would only invent a lock
//! order to get wrong.
//!
//! # Claiming is single-flight
//!
//! One enumeration owns one native buffer and one record cursor, so exactly one
//! worker may hold it at a time. [`Registry::claim_next`] enforces that by
//! refusing an enumeration that is already running, rather than trusting callers
//! to submit only as often as is safe.

use std::collections::{HashMap, VecDeque};

use windows_impersonation_token_sys::ImpersonationToken;

use crate::completion::EnumerationId;
use crate::completion_ring::TerminalSlot;
use crate::request::EnumerationRequest;
use crate::submission_ring::RetireSlot;

/// One enumeration the session is carrying.
pub(crate) struct EnumerationState {
    /// What to enumerate, and how.
    ///
    /// Read by the native engine (M6), which opens the path and evaluates the
    /// predicate; the shell only carries it.
    #[allow(dead_code, reason = "FE-8 opens the path this describes")]
    pub(crate) request: EnumerationRequest,
    /// The submitter's captured security context, applied only while the
    /// directory handle is opened.
    #[allow(dead_code, reason = "FE-8 opens the directory with this")]
    pub(crate) token: ImpersonationToken,
    /// The reserved completion slot this enumeration's terminal will use.
    ///
    /// Taken by whichever authority delivers the outcome, which is the worker
    /// for an outcome its own work decided and the servicer for a cancellation
    /// observed while the enumeration was quiescent.
    pub(crate) terminal: Option<TerminalSlot>,
    /// The reserved submission slot a worker spends to report retirement.
    ///
    /// Still present when the servicer removes an entry itself, in which case it
    /// is returned to the ring rather than spent.
    pub(crate) retire: Option<RetireSlot>,
    /// Set once cancellation has been serviced. A running quantum observes it
    /// when it reports; a quiescent enumeration is finished immediately.
    pub(crate) cancelled: bool,
    /// Whether a worker holds this enumeration right now.
    pub(crate) running: bool,
    /// Whether the enumeration is waiting for completion-ring room.
    pub(crate) parked: bool,
    /// Whether the enumeration is already in the ready queue, so a second
    /// schedule does not queue it twice.
    queued: bool,
}

impl EnumerationState {
    pub(crate) fn new(
        request: EnumerationRequest,
        token: ImpersonationToken,
        terminal: TerminalSlot,
        retire: RetireSlot,
    ) -> Self {
        Self {
            request,
            token,
            terminal: Some(terminal),
            retire: Some(retire),
            cancelled: false,
            running: false,
            parked: false,
            queued: false,
        }
    }

    /// Whether this enumeration can be finished by an authority other than the
    /// worker holding it.
    ///
    /// A quantum in flight owns the transition instead, because a terminal
    /// delivered underneath a running quantum could be followed by an entry that
    /// quantum had already parsed.
    pub(crate) fn is_quiescent(&self) -> bool {
        !self.running
    }
}

/// Every enumeration a session is carrying, plus the order they run in.
pub(crate) struct Registry {
    entries: HashMap<EnumerationId, EnumerationState>,
    /// Enumerations with work to do, oldest first.
    ready: VecDeque<EnumerationId>,
    /// Cleared by abandonment. A session that has lost its receiver owes no
    /// outcomes, so it must not take on new work either.
    accepting: bool,
}

impl Registry {
    pub(crate) fn new() -> Self {
        Self {
            entries: HashMap::new(),
            ready: VecDeque::new(),
            accepting: true,
        }
    }

    /// Whether new enumerations may still be registered.
    pub(crate) fn is_accepting(&self) -> bool {
        self.accepting
    }

    /// Refuse further registrations. Abandonment is permanent.
    pub(crate) fn stop_accepting(&mut self) {
        self.accepting = false;
    }

    /// How many enumerations are registered.
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Register one enumeration.
    pub(crate) fn insert(&mut self, enumeration: EnumerationId, state: EnumerationState) {
        self.entries.insert(enumeration, state);
    }

    pub(crate) fn get_mut(&mut self, enumeration: EnumerationId) -> Option<&mut EnumerationState> {
        self.entries.get_mut(&enumeration)
    }

    #[cfg(test)]
    pub(crate) fn contains(&self, enumeration: EnumerationId) -> bool {
        self.entries.contains_key(&enumeration)
    }

    pub(crate) fn remove(&mut self, enumeration: EnumerationId) -> Option<EnumerationState> {
        self.entries.remove(&enumeration)
    }

    /// Take every registered enumeration, leaving the registry empty.
    ///
    /// Used by abandonment, which tears the whole session down at once. The
    /// ready set goes with them: an id left queued would name nothing.
    pub(crate) fn drain_all(&mut self) -> Vec<(EnumerationId, EnumerationState)> {
        self.ready.clear();
        self.entries.drain().collect()
    }

    /// Mark an enumeration runnable.
    ///
    /// Idempotent, and a no-op for an enumeration that is gone or already
    /// running -- a running one is re-queued when its worker reports, not while
    /// it still holds the buffer.
    pub(crate) fn mark_ready(&mut self, enumeration: EnumerationId) {
        let Some(state) = self.entries.get_mut(&enumeration) else {
            return;
        };
        state.parked = false;
        if state.queued || state.running {
            return;
        }
        state.queued = true;
        self.ready.push_back(enumeration);
    }

    /// Claim the next runnable enumeration for one worker.
    ///
    /// Skips ids that have since been removed or claimed, so a stale queue entry
    /// costs a pop rather than a wrong answer.
    pub(crate) fn claim_next(&mut self) -> Option<EnumerationId> {
        while let Some(enumeration) = self.ready.pop_front() {
            let Some(state) = self.entries.get_mut(&enumeration) else {
                continue;
            };
            state.queued = false;
            if state.running {
                continue;
            }
            state.running = true;
            state.parked = false;
            return Some(enumeration);
        }
        None
    }

    /// The enumerations waiting for completion-ring room, in no particular
    /// order: they are all resumed together when room appears.
    pub(crate) fn parked(&self) -> Vec<EnumerationId> {
        self.entries
            .iter()
            .filter(|(_, state)| state.parked)
            .map(|(id, _)| *id)
            .collect()
    }

    /// How many enumerations are waiting to run.
    #[cfg(test)]
    pub(crate) fn ready_len(&self) -> usize {
        self.ready.len()
    }
}
