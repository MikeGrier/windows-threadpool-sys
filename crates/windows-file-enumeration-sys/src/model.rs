// Copyright (c) 2026 Mike Grier
//! A deterministic model of the session, for state-machine testing.
//!
//! # Why a model rather than more unit tests
//!
//! The session's hard properties are not properties of any one call. "Exactly
//! one terminal per enumeration", "no entry after a terminal", "reservations
//! never take the last slot", and "the doorbell is signalled exactly when there
//! is something to observe" are invariants over *sequences*, and a unit test
//! that checks them at one point proves nothing about the next interleaving.
//!
//! This harness applies scripted operations to a real session and re-checks
//! every invariant after each one, so a scenario is written as the interesting
//! order of events and the checking comes for free.
//!
//! # Why it is deterministic
//!
//! The servicer normally runs on a thread-pool work item, whose timing nothing
//! can pin down. [`Op::Service`] drains the submission ring on the calling
//! thread instead, using the same code path the callback uses, so a scenario
//! says exactly when servicing happens. The thread-pool path itself is covered
//! separately, where the assertion is that it eventually runs -- which is the
//! only thing that can honestly be asserted about it.
//!
//! # What stands in for the engine
//!
//! The native engine (M6) is what will produce entries and decide outcomes.
//! Here the scenario plays that part with [`Op::OfferEntry`],
//! [`Op::EnterQuantum`], [`Op::LeaveQuantum`], and [`Op::Complete`], driving the
//! same crate-internal transitions the engine will drive. That is deliberate:
//! the shell's invariants must hold for *any* engine that respects those
//! transitions, so modelling them directly tests the contract rather than one
//! engine's habits.

use std::collections::{HashMap, HashSet, VecDeque};

use wtf_string::Wtf16String;

use crate::admission::EnumerationHandle;
use crate::completion::{Completion, EnumerationId, TerminalOutcome};
use crate::error::BeginFailure;
use crate::request::EnumerationRequest;
use crate::session::{Receiver, Session};
use crate::testing::named_file;

/// One scripted step.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Op {
    /// Admit a new enumeration, expecting it to be accepted.
    Begin,
    /// Admit a new enumeration, expecting it to be refused for `reason`.
    BeginRefused(BeginFailure),
    /// Drain the submission ring on this thread.
    Service,
    /// Cancel the enumeration in `slot` through its handle.
    Cancel(usize),
    /// Drop the handle for `slot`, which also cancels.
    DropHandle(usize),
    /// Detach the handle for `slot`, letting the enumeration run on.
    Detach(usize),
    /// The engine takes a quantum of work for `slot`.
    EnterQuantum(usize),
    /// The engine gives the quantum back; `true` parks for want of room.
    LeaveQuantum(usize, bool),
    /// The engine offers one entry named `name` for `slot`.
    ///
    /// Records whether the ring accepted it, which is what backpressure means
    /// here: a refused entry is still the engine's to retry.
    OfferEntry(usize, &'static str),
    /// The engine finishes `slot` normally.
    Complete(usize),
    /// The receiver takes one record, if any.
    Recv,
    /// The receiver takes everything queued.
    DrainReceiver,
    /// The receiver goes away, abandoning the session.
    DropReceiver,
    /// Drop every session handle, so only outstanding enumerations keep the
    /// stream open.
    DropSession,
}

/// What the model observed for one enumeration.
#[derive(Default)]
struct Observed {
    entries: Vec<String>,
    terminal: Option<&'static str>,
}

/// A session under test, plus everything needed to check its invariants.
pub(crate) struct Model {
    session: Option<Session>,
    receiver: Option<Receiver>,
    handles: Vec<Option<EnumerationHandle>>,
    ids: Vec<EnumerationId>,
    /// Entries the engine successfully handed to the ring, per enumeration, in
    /// the order it handed them over.
    offered: HashMap<EnumerationId, VecDeque<String>>,
    /// Entries and outcomes the receiver actually observed.
    observed: HashMap<EnumerationId, Observed>,
    /// Enumerations whose terminal has already been observed.
    finished: HashSet<EnumerationId>,
    /// Entries the ring refused, which is backpressure rather than loss.
    refused: usize,
    completion_capacity: usize,
}

impl Model {
    /// Build a model over a session with the given bounds.
    pub(crate) fn new(submission_capacity: usize, completion_capacity: usize) -> Self {
        let (session, receiver) =
            Session::new(submission_capacity, completion_capacity).expect("valid bounds");
        // Nothing rings the servicer's doorbell, so the pool never races a
        // scripted step: `Op::Service` is the only thing that drains.
        session.suppress_doorbell();
        // Created up front so every check can compare the doorbell against the
        // ring rather than only from the point a scenario happens to ask.
        receiver.doorbell().expect("an event");
        Self {
            session: Some(session),
            receiver: Some(receiver),
            handles: Vec::new(),
            ids: Vec::new(),
            offered: HashMap::new(),
            observed: HashMap::new(),
            finished: HashSet::new(),
            refused: 0,
            completion_capacity,
        }
    }

    /// Apply a script, checking every invariant after each step.
    pub(crate) fn run(&mut self, script: &[Op]) {
        for (index, op) in script.iter().enumerate() {
            self.apply(*op);
            self.check(index, *op);
        }
    }

    /// How many enumerations the session is carrying.
    pub(crate) fn registered(&self) -> usize {
        self.session.as_ref().map_or(0, Session::enumerations)
    }

    /// The entries the receiver observed for `slot`, in order.
    pub(crate) fn entries(&self, slot: usize) -> Vec<String> {
        self.observed
            .get(&self.ids[slot])
            .map(|observed| observed.entries.clone())
            .unwrap_or_default()
    }

    /// The terminal the receiver observed for `slot`, if any.
    pub(crate) fn terminal(&self, slot: usize) -> Option<&'static str> {
        self.observed
            .get(&self.ids[slot])
            .and_then(|observed| observed.terminal)
    }

    /// How many offered entries the ring refused for want of room.
    pub(crate) fn refused(&self) -> usize {
        self.refused
    }

    fn session(&self) -> &Session {
        self.session.as_ref().expect("the session is still held")
    }

    fn apply(&mut self, op: Op) {
        match op {
            Op::Begin => {
                let handle = self
                    .session()
                    .try_begin(request())
                    .expect("the script expects room");
                self.ids.push(handle.id());
                self.handles.push(Some(handle));
            }
            Op::BeginRefused(expected) => {
                let error = self
                    .session()
                    .try_begin(request())
                    .expect_err("the script expects a refusal");
                assert_eq!(error.failure(), expected);
            }
            Op::Service => self.session().shared.drain_submissions(),
            Op::Cancel(slot) => {
                if let Some(handle) = self.handles[slot].take() {
                    handle.cancel();
                }
            }
            Op::DropHandle(slot) => {
                self.handles[slot] = None;
            }
            Op::Detach(slot) => {
                if let Some(handle) = self.handles[slot].take() {
                    handle.detach();
                }
            }
            Op::EnterQuantum(slot) => {
                let id = self.ids[slot];
                self.session().shared.enter_quantum(id);
            }
            Op::LeaveQuantum(slot, parked) => {
                let id = self.ids[slot];
                self.session().shared.leave_quantum(id, parked);
            }
            Op::OfferEntry(slot, name) => {
                let id = self.ids[slot];
                let record = Completion::Entry {
                    enumeration: id,
                    entry: named_file(name),
                };
                match self.session().shared.completions.try_send_entry(record) {
                    Ok(()) => self
                        .offered
                        .entry(id)
                        .or_default()
                        .push_back(name.to_string()),
                    Err(_) => self.refused += 1,
                }
            }
            Op::Complete(slot) => {
                let id = self.ids[slot];
                self.session()
                    .shared
                    .complete(id, TerminalOutcome::Completed);
            }
            Op::Recv => {
                let record = self
                    .receiver
                    .as_ref()
                    .and_then(|receiver| receiver.try_recv());
                if let Some(record) = record {
                    self.observe(record);
                }
            }
            Op::DrainReceiver => {
                while let Some(record) = self
                    .receiver
                    .as_ref()
                    .and_then(|receiver| receiver.try_recv())
                {
                    self.observe(record);
                }
            }
            Op::DropReceiver => {
                self.receiver = None;
            }
            Op::DropSession => {
                self.session = None;
            }
        }
    }

    /// Record one observed completion, checking the ordering rules as it goes.
    fn observe(&mut self, record: Completion) {
        let id = record.enumeration();
        let observed = self.observed.entry(id).or_default();
        match record {
            Completion::Entry { entry, .. } => {
                assert!(
                    observed.terminal.is_none(),
                    "{id} produced an entry after its terminal"
                );
                observed.entries.push(entry.name().to_string_lossy());
            }
            Completion::Terminal { outcome, .. } => {
                assert!(
                    observed.terminal.is_none(),
                    "{id} produced a second terminal"
                );
                observed.terminal = Some(match outcome {
                    TerminalOutcome::Completed => "completed",
                    TerminalOutcome::Cancelled => "cancelled",
                    TerminalOutcome::Failed(_) => "failed",
                });
                assert!(self.finished.insert(id), "{id} finished twice");
            }
        }
    }

    /// Every invariant the session promises, checked after one step.
    fn check(&self, index: usize, op: Op) {
        let context = format!("after step {index} ({op:?})");

        let Some(session) = self.session.as_ref() else {
            // With the session gone there is no ring to inspect through it; the
            // ordering checks in `observe` still apply to anything drained.
            return;
        };
        let ring = &session.shared.completions;

        let queued = ring.len();
        let reserved = ring.reserved();
        assert!(
            queued + reserved <= self.completion_capacity,
            "{context}: {queued} queued plus {reserved} reserved exceeds the bound"
        );
        assert!(
            reserved < self.completion_capacity,
            "{context}: reservations took every slot, leaving no room for an entry"
        );

        // The doorbell's whole contract in one line.
        if let Ok(handle) = self.receiver.as_ref().map_or_else(
            || Err(std::io::Error::other("no receiver")),
            |receiver| {
                receiver
                    .doorbell()
                    .map(|handle| handle.try_clone_to_owned())
            },
        ) {
            let handle = handle.expect("the doorbell can be duplicated");
            assert_eq!(
                is_signalled(&handle),
                ring.is_pending(),
                "{context}: the doorbell disagrees with what the receiver can observe"
            );
        }

        // Per-enumeration delivery is a prefix of what was offered, in order.
        for (id, offered) in &self.offered {
            let Some(observed) = self.observed.get(id) else {
                continue;
            };
            let expected: Vec<&String> = offered.iter().take(observed.entries.len()).collect();
            let actual: Vec<&String> = observed.entries.iter().collect();
            assert_eq!(expected, actual, "{context}: {id} delivered out of order");
        }
    }
}

fn request() -> EnumerationRequest {
    EnumerationRequest::new(&Wtf16String::from(r"C:\Windows")).expect("a resolvable path")
}

/// Whether a waitable handle is currently signalled.
fn is_signalled(handle: &std::os::windows::io::OwnedHandle) -> bool {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::WaitForSingleObject;

    // SAFETY: the handle is a live event duplicated from the ring under test.
    let result = unsafe { WaitForSingleObject(handle.as_raw_handle() as HANDLE, 0) };
    result == WAIT_OBJECT_0
}

#[cfg(test)]
mod tests;
