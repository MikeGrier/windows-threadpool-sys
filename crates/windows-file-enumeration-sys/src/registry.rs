// Copyright (c) 2026 Mike Grier
//! The registry of enumerations a session is carrying.
//!
//! Exactly one authority mutates this: the submission-ring servicer. Begin,
//! cancel, and abandon all arrive through that one ordered path, so the registry
//! never has to reconcile two callers racing to decide whether an enumeration
//! exists.
//!
//! Each entry owns the two things that make its outcome unconditional -- the
//! reserved completion slot its terminal will use, and the impersonation context
//! its directory will be opened under -- plus the flags the engine consults
//! between records.

use std::collections::HashMap;

use windows_impersonation_token_sys::ImpersonationToken;
use windows_threadpool_sys::work::ThreadpoolWork;

use crate::completion::EnumerationId;
use crate::completion_ring::TerminalSlot;
use crate::request::EnumerationRequest;

/// One enumeration the session is carrying.
pub(crate) struct EnumerationState {
    /// What to enumerate, and how.
    ///
    /// Read by the native engine (M6), which opens the path and evaluates the
    /// predicate; the shell only carries it.
    #[allow(dead_code, reason = "the native engine (M6) reads the request")]
    pub(crate) request: EnumerationRequest,
    /// The submitter's captured security context, applied only while the
    /// directory handle is opened.
    #[allow(
        dead_code,
        reason = "the native engine (M6) opens the directory with this"
    )]
    pub(crate) token: ImpersonationToken,
    /// The reserved completion slot this enumeration's terminal will use.
    ///
    /// `None` only while the outcome is being delivered, which is also when the
    /// entry is being removed.
    pub(crate) terminal: Option<TerminalSlot>,
    /// Set once cancellation has been serviced. A running quantum observes it
    /// between records; a quiescent enumeration is finished immediately.
    pub(crate) cancelled: bool,
    /// Whether a quantum is executing right now.
    ///
    /// Cancellation cannot preempt one, so it defers the terminal to whichever
    /// side observes the state last.
    pub(crate) running: bool,
    /// Whether the enumeration is waiting for completion-ring room.
    pub(crate) parked: bool,
    /// The work object that advances this enumeration.
    ///
    /// The native engine (M6) installs it. Until then an admitted enumeration is
    /// registered and cancellable but produces no entries.
    pub(crate) work: Option<ThreadpoolWork>,
}

impl EnumerationState {
    pub(crate) fn new(
        request: EnumerationRequest,
        token: ImpersonationToken,
        terminal: TerminalSlot,
    ) -> Self {
        Self {
            request,
            token,
            terminal: Some(terminal),
            cancelled: false,
            running: false,
            parked: false,
            work: None,
        }
    }

    /// Whether this enumeration can be finished right now.
    ///
    /// A quantum in flight owns the transition instead, because a terminal
    /// delivered underneath a running quantum could be followed by an entry that
    /// quantum had already parsed.
    pub(crate) fn is_quiescent(&self) -> bool {
        !self.running
    }
}

/// Every enumeration a session is carrying, and the identifiers it hands out.
pub(crate) struct Registry {
    entries: HashMap<EnumerationId, EnumerationState>,
    /// Cleared by abandonment. A session that has lost its receiver owes no
    /// outcomes, so it must not take on new work either.
    accepting: bool,
}

impl Registry {
    pub(crate) fn new() -> Self {
        Self {
            entries: HashMap::new(),
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
    /// Used by abandonment, which tears the whole session down at once.
    pub(crate) fn drain_all(&mut self) -> Vec<(EnumerationId, EnumerationState)> {
        self.entries.drain().collect()
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
}
