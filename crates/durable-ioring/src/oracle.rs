// Copyright (c) 2026 Mike Grier
//! The conformance oracle ([DI-D-24](../DESIGN-NOTES.md#di-d-24)): the contract's rules over the
//! event stream, as one executable definition every implementation's tests bind to -- dioring's
//! own, the fault-injecting implementation's, and those of layers above. A hand-written second
//! copy of a rule in some other test is a check of the copy, not of the contract.
//!
//! The stream is what the consumer sees: each accepted push, reported to the oracle with
//! [`ConformanceOracle::pushed`], and each popped entry, reported with
//! [`ConformanceOracle::observe`]. The oracle grows with the implementation (DI-3.2): each step
//! adds the rules it implements, and every rule cites the contract's statement of it.
//!
//! # Checked
//!
//! - **One completion per operation.** Every accepted push completes exactly once: a completion
//!   for an operation no push returned, or a second completion for one, is a violation, and so
//!   is an operation still outstanding when the caller declares the stream finished
//!   ([`ConformanceOracle::finish`]). A push returning an identity an earlier push returned is a
//!   violation too, since completions could no longer be told apart.
//! - **What was pushed comes back.** The completion carries the kind the operation was pushed
//!   with -- a read, or a write with its epoch -- and the consumer's context, unchanged.
//!
//! # Deliberately not checked
//!
//! Each is legal, and asserting otherwise would encode a rule the contract does not make:
//!
//! - **Any order among completions.** Operations complete in whatever order the kernel finishes
//!   them, and DI-D-18 records completions in the order they are observed, not pushed.
//! - **Any outcome.** A transfer shorter than the buffer, including zero, and a failure are both
//!   completions. What the contract promises is that the completion arrives, not that the I/O
//!   succeeded.
//! - **The entries later steps define** -- `Durable`, `Failed`, `Blocked`, `Abandoned` and
//!   `LineageEnded` -- are accepted unexamined until the steps that produce them add their rules.
//!
//! # What the stream cannot show
//!
//! Whether the bytes reached the file at the offset given, and whether the readiness signal was
//! set when it should have been: the oracle sees entries, not wakes. The second is
//! [`check_readiness`]'s, which runs beside the oracle (DI-D-28).

use std::collections::{HashMap, HashSet};
use std::io;
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::Duration;

use windows_threadpool_sys::wait::ThreadpoolWait;

use crate::contract::{DurableRing, EntryOf, Identities};
use crate::types::{Entry, OpKind};

#[cfg(test)]
mod tests;

/// What a push gave the instance, kept until its completion.
struct Pushed<V: Identities, C> {
    kind: OpKind<V>,
    context: C,
}

/// A contract rule the stream broke.
#[derive(Debug)]
pub enum Violation<V: Identities> {
    /// A push returned an identity an earlier push already returned.
    DuplicateIdentity {
        /// The repeated identity.
        op: V::OpId,
    },
    /// A completion for an operation no push returned.
    NeverPushed {
        /// The operation completed.
        op: V::OpId,
    },
    /// A second completion for one operation.
    CompletedTwice {
        /// The operation.
        op: V::OpId,
    },
    /// The completion's kind is not the one the operation was pushed with.
    KindChanged {
        /// The operation.
        op: V::OpId,
        /// The kind it was pushed with.
        pushed: OpKind<V>,
        /// The kind its completion reported.
        completed: OpKind<V>,
    },
    /// The completion's context is not the one the operation was pushed with.
    ContextChanged {
        /// The operation.
        op: V::OpId,
    },
    /// Operations pushed and not completed when the stream was declared finished.
    Outstanding {
        /// The operations still outstanding.
        ops: Vec<V::OpId>,
    },
}

/// The contract's rules over one instance's event stream. `C` is the consumer's context type;
/// the oracle keeps the context each push was given, to compare with its completion's.
pub struct ConformanceOracle<V: Identities, C> {
    outstanding: HashMap<V::OpId, Pushed<V, C>>,
    completed: HashSet<V::OpId>,
}

impl<V: Identities, C: PartialEq> ConformanceOracle<V, C> {
    /// An oracle that has seen nothing.
    pub fn new() -> Self {
        Self {
            outstanding: HashMap::new(),
            completed: HashSet::new(),
        }
    }

    /// Report an accepted push: the identity it returned, what it was, and the context it was
    /// given (a copy; the instance has the original).
    ///
    /// # Errors
    ///
    /// [`Violation::DuplicateIdentity`] if an earlier push returned `op`.
    pub fn pushed(&mut self, op: V::OpId, kind: OpKind<V>, context: C) -> Result<(), Violation<V>> {
        if self.completed.contains(&op) || self.outstanding.contains_key(&op) {
            return Err(Violation::DuplicateIdentity { op });
        }
        self.outstanding.insert(op, Pushed { kind, context });
        Ok(())
    }

    /// Report a popped entry.
    ///
    /// # Errors
    ///
    /// The first rule the entry breaks.
    pub fn observe<B>(&mut self, entry: &Entry<V, B, C>) -> Result<(), Violation<V>> {
        let Entry::Op(completion) = entry else {
            return Ok(());
        };
        let op = completion.id;
        let Some(pushed) = self.outstanding.remove(&op) else {
            return Err(if self.completed.contains(&op) {
                Violation::CompletedTwice { op }
            } else {
                Violation::NeverPushed { op }
            });
        };
        self.completed.insert(op);
        if pushed.kind != completion.kind {
            return Err(Violation::KindChanged {
                op,
                pushed: pushed.kind,
                completed: completion.kind,
            });
        }
        if pushed.context != completion.context {
            return Err(Violation::ContextChanged { op });
        }
        Ok(())
    }

    /// How many pushed operations have not completed.
    pub fn outstanding(&self) -> usize {
        self.outstanding.len()
    }

    /// Declare the stream finished: every pushed operation should have completed.
    ///
    /// # Errors
    ///
    /// [`Violation::Outstanding`], naming the operations that did not.
    pub fn finish(self) -> Result<(), Violation<V>> {
        if self.outstanding.is_empty() {
            Ok(())
        } else {
            Err(Violation::Outstanding {
                ops: self.outstanding.into_keys().collect(),
            })
        }
    }
}

impl<V: Identities, C: PartialEq> Default for ConformanceOracle<V, C> {
    fn default() -> Self {
        Self::new()
    }
}

/// Why [`check_readiness`] failed.
#[derive(Debug)]
pub enum ReadinessFailure {
    /// The first entry into an empty queue did not set the signal within the bound.
    NotSetForFirstEntry,
    /// An entry into the queue, emptied again after the first, did not set the signal within the
    /// bound: the signal was set once and not on a later empty-to-non-empty transition.
    NotSetAgain,
    /// Taking the signal, popping, or waiting failed.
    Io(io::Error),
}

impl From<io::Error> for ReadinessFailure {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// The readiness signal's harness check (DI-D-28): push into an empty queue and require the
/// signal set; drain; push again and require it set again. The oracle sees entries, not wakes, so
/// this runs beside it.
///
/// `push` must start exactly one operation whose completion becomes an entry. **Nothing may be in
/// flight when this is called**: it drains the queue and withdraws a stale signal first, and an
/// entry landing in between would have its wake withdrawn with it.
///
/// Returns every entry it popped, in order, so the caller can report them to its oracle.
///
/// # Errors
///
/// The first part of the check that failed.
pub fn check_readiness<D, P>(
    ring: &mut D,
    mut push: P,
    bound: Duration,
) -> Result<Vec<EntryOf<D>>, ReadinessFailure>
where
    D: DurableRing,
    P: FnMut(&mut D),
{
    let mut entries = Vec::new();
    let signal = ring.readiness()?;
    drain(ring, &mut entries)?;
    signal.reset()?;

    let (woken, wakes) = mpsc::channel();
    let woken = Mutex::new(woken);
    let wait = ThreadpoolWait::new(
        signal.into(),
        move |_| {
            if let Ok(woken) = woken.lock() {
                let _ = woken.send(());
            }
        },
        None,
    )?;

    let mut steps = || -> Result<(), ReadinessFailure> {
        wait.arm(None);
        push(ring);
        wakes
            .recv_timeout(bound)
            .map_err(|_| ReadinessFailure::NotSetForFirstEntry)?;
        drain(ring, &mut entries)?;

        wait.arm(None);
        push(ring);
        wakes
            .recv_timeout(bound)
            .map_err(|_| ReadinessFailure::NotSetAgain)?;
        drain(ring, &mut entries)?;
        Ok(())
    };
    let outcome = steps();
    // Discharges the drain the wait owes, on every path.
    wait.stop_and_drain();
    outcome.map(|()| entries)
}

/// Pop until the queue is empty.
fn drain<D: DurableRing>(ring: &mut D, into: &mut Vec<EntryOf<D>>) -> io::Result<()> {
    while let Some(entry) = ring.pop()? {
        into.push(entry);
    }
    Ok(())
}
