// Copyright (c) 2026 Mike Grier
//! The conformance oracle ([DI-D-24](../DESIGN-NOTES.md#di-d-24)): the contract's rules over the
//! event stream, as one executable definition every implementation's tests bind to -- dioring's
//! own, the fault-injecting implementation's, and those of layers above. A hand-written second
//! copy of a rule in some other test is a check of the copy, not of the contract.
//!
//! The stream is what the consumer sees: each accepted push, reported to the oracle with
//! [`ConformanceOracle::pushed`]; each seal the instance accepted as new, reported with
//! [`ConformanceOracle::sealed`]; each failure the consumer healed, reported with
//! [`ConformanceOracle::healed`]; and each popped entry, reported with
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
//! - **No write is accepted at or below its lineage's seal** (guarantee 6): such a push is refused,
//!   so an identity returned for one is a violation.
//! - **No write is accepted into an abandoned epoch** (DI-D-35), once its `Abandoned` entry has
//!   been observed.
//! - **`Durable` is reported only for what was sealed** (DI-D-9): `Durable { through: n }` needs a
//!   seal of n's lineage at or above n.
//! - **`Durable` follows the writes it covers** (guarantee 5): it arrives after the completion of
//!   every write of n's lineage tagged at or below n.
//! - **`Durable` waits for every failure holding it** (DI-D-12 (f)): it passes no epoch of its
//!   lineage that a reported failure contains, unless that failure was healed or abandoned.
//! - **A failure is new, and suspects only accepted writes** (DI-D-12 (b)): its identity was never
//!   reported before, and every suspect write was accepted as a write tagged with the epoch
//!   reported, listed in push order.
//! - **Finality** (guarantee 3): no suspect write is of an epoch its lineage was already reported
//!   durable through.
//! - **Failure stamps never decrease** (DI-D-38): each `Failed` was observed at or after the
//!   `Failed` before it in the stream. Equal stamps are legal -- interrupt time's resolution is the
//!   system clock tick -- and so is any gap.
//! - **`Abandoned` and `Blocked` name a live failure**: `Abandoned` one reported and not yet
//!   abandoned, with the suspect set it was reported with; `Blocked { through: n, by }` one
//!   containing an epoch of n's lineage at or below n.
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
//! - **Order between lineages**, which the contract leaves open (DI-D-18).
//! - **A `Durable` repeated or below an earlier one** for the same lineage. It is never false --
//!   the high-water mark did pass it -- and the contract promises an answer per request, not a
//!   rising sequence.
//! - **That every request is answered** (guarantee 4): a seal that stays pending is legal while a
//!   provider has not answered, and the stream cannot show that a provider has not.
//! - **Which writes a failure should have suspected.** That depends on the order the instance
//!   observed things in and on the files' declared flush domains, neither of which the stream
//!   carries; so the members are checked, not the set's extent.
//! - **When a heal takes effect.** It waits for a seal the stream cannot tell apart from others,
//!   so a `Durable` that passes a healed failure is accepted whenever it comes.
//! - **`LineageEnded`**, accepted unexamined until the step that produces it adds its rules.
//! - **What a stamp's value is**: how it relates to wall-clock time, or to any clock outside the
//!   instance. The clock is the instance's to choose (DI-D-38), and a test's mock may run at any
//!   rate, so long as it never runs backwards.
//!
//! # What the stream cannot show
//!
//! Whether the bytes reached the file at the offset given; whether a reported `Durable` is true,
//! which only the device knows; whether the inventory reports a failure with the stamp its `Failed`
//! entry carried, since the inventory is not in the stream; and whether the readiness signal was
//! set when it should have been:
//! the oracle sees entries, not wakes. The second is
//! [`check_readiness`]'s, which runs beside the oracle (DI-D-28).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::io;
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::Duration;

use win_time_sys::{InterruptTime, TimePoint};
use windows_threadpool_sys::wait::ThreadpoolWait;

use crate::contract::{DurableRing, EntryOf, Identities};
use crate::types::{Entry, Epoch, OpKind, SuspectWrite};

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
    /// A write was accepted at or below its lineage's seal (guarantee 6).
    WriteAfterSeal {
        /// The write.
        op: V::OpId,
        /// Its epoch.
        epoch: Epoch<V>,
        /// The seal point it is at or below.
        sealed_through: V::EpochId,
    },
    /// A write was accepted into an epoch already reported abandoned (DI-D-35).
    WriteToAbandoned {
        /// The write.
        op: V::OpId,
        /// Its epoch.
        epoch: Epoch<V>,
    },
    /// `Durable` for an epoch no seal of its lineage covers.
    DurableNotSealed {
        /// What was reported.
        through: Epoch<V>,
    },
    /// `Durable` before the completion of a write it covers (guarantee 5).
    DurableBeforeCompletion {
        /// What was reported.
        through: Epoch<V>,
        /// A covered write not yet completed.
        op: V::OpId,
    },
    /// `Durable` past an epoch a failure contains that was neither healed nor abandoned.
    DurableThroughFailure {
        /// What was reported.
        through: Epoch<V>,
        /// The failure.
        failure: V::FailureId,
    },
    /// `Failed` with an identity already reported.
    DuplicateFailure {
        /// The repeated identity.
        failure: V::FailureId,
    },
    /// A suspect write that was not accepted as a write with the epoch reported.
    SuspectNotPushed {
        /// The failure.
        failure: V::FailureId,
        /// The write.
        op: V::OpId,
    },
    /// A `Failed` stamped earlier than the `Failed` before it (DI-D-38).
    StampWentBackwards {
        /// The failure.
        failure: V::FailureId,
        /// Its stamp.
        observed: TimePoint<InterruptTime>,
        /// The previous failure's stamp.
        previous: TimePoint<InterruptTime>,
    },
    /// A suspect set not in push order.
    SuspectsOutOfOrder {
        /// The failure.
        failure: V::FailureId,
    },
    /// A suspect write of an epoch already reported durable (guarantee 3).
    SuspectAlreadyDurable {
        /// The failure.
        failure: V::FailureId,
        /// The write.
        op: V::OpId,
    },
    /// `Abandoned` for a failure not reported, or already abandoned.
    AbandonedUnknown {
        /// The failure.
        failure: V::FailureId,
    },
    /// `Abandoned` with a suspect set other than the one `Failed` reported.
    AbandonedSuspectChanged {
        /// The failure.
        failure: V::FailureId,
    },
    /// `Blocked` by a failure that is not live, or holds no epoch at or below the request.
    BlockedByUnrelated {
        /// What was reported.
        through: Epoch<V>,
        /// The failure named.
        by: V::FailureId,
    },
}

/// A failure reported and not yet abandoned.
struct Reported<V: Identities> {
    suspect: Vec<(V::OpId, Epoch<V>)>,
    healed: bool,
}

/// The contract's rules over one instance's event stream. `C` is the consumer's context type;
/// the oracle keeps the context each push was given, to compare with its completion's.
pub struct ConformanceOracle<V: Identities, C> {
    outstanding: HashMap<V::OpId, Pushed<V, C>>,
    completed: HashSet<V::OpId>,
    /// Each lineage's seal point.
    sealed: HashMap<V::Lineage, V::EpochId>,
    /// Every accepted write's epoch, kept after it completes: a failure may suspect it later.
    writes: HashMap<V::OpId, Epoch<V>>,
    /// The highest `Durable` reported for each lineage.
    durable: HashMap<V::Lineage, V::EpochId>,
    /// The failures reported and not abandoned.
    failures: HashMap<V::FailureId, Reported<V>>,
    /// Every failure identity reported.
    failure_ids: HashSet<V::FailureId>,
    /// The latest `Failed` entry's stamp.
    last_stamp: Option<TimePoint<InterruptTime>>,
    /// The epochs of every abandoned failure, by lineage.
    abandoned: HashMap<V::Lineage, BTreeSet<V::EpochId>>,
}

impl<V: Identities, C: PartialEq> ConformanceOracle<V, C> {
    /// An oracle that has seen nothing.
    pub fn new() -> Self {
        Self {
            outstanding: HashMap::new(),
            completed: HashSet::new(),
            sealed: HashMap::new(),
            writes: HashMap::new(),
            durable: HashMap::new(),
            failures: HashMap::new(),
            failure_ids: HashSet::new(),
            last_stamp: None,
            abandoned: HashMap::new(),
        }
    }

    /// Report a failure the consumer healed: `resolve` accepted `Heal` for it. Abandoning needs no
    /// report; the `Abandoned` entry is the record.
    pub fn healed(&mut self, failure: V::FailureId) {
        if let Some(reported) = self.failures.get_mut(&failure) {
            reported.healed = true;
        }
    }

    /// Report a seal the instance accepted as new: `make_durable_through(through)` answered
    /// `Submitted`. A request answered `AlreadySealed` changes nothing and need not be reported.
    pub fn sealed(&mut self, through: Epoch<V>) {
        let point = self.sealed.entry(through.lineage).or_insert(through.id);
        if through.id > *point {
            *point = through.id;
        }
    }

    /// Report an accepted push: the identity it returned, what it was, and the context it was
    /// given (a copy; the instance has the original).
    ///
    /// # Errors
    ///
    /// [`Violation::DuplicateIdentity`] if an earlier push returned `op`,
    /// [`Violation::WriteAfterSeal`] for a write at or below its lineage's seal, and
    /// [`Violation::WriteToAbandoned`] for a write into an epoch reported abandoned.
    pub fn pushed(&mut self, op: V::OpId, kind: OpKind<V>, context: C) -> Result<(), Violation<V>> {
        if self.completed.contains(&op) || self.outstanding.contains_key(&op) {
            return Err(Violation::DuplicateIdentity { op });
        }
        if let OpKind::Write { epoch } = kind
            && let Some(&sealed_through) = self.sealed.get(&epoch.lineage)
            && epoch.id <= sealed_through
        {
            return Err(Violation::WriteAfterSeal {
                op,
                epoch,
                sealed_through,
            });
        }
        if let OpKind::Write { epoch } = kind {
            if self
                .abandoned
                .get(&epoch.lineage)
                .is_some_and(|ids| ids.contains(&epoch.id))
            {
                return Err(Violation::WriteToAbandoned { op, epoch });
            }
            self.writes.insert(op, epoch);
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
        let completion = match entry {
            Entry::Op(completion) => completion,
            Entry::Durable { through } => return self.durable(*through),
            Entry::Failed(failed) => {
                return self.failed(failed.id, failed.suspect.writes(), failed.observed);
            }
            Entry::Abandoned { failure, suspect } => {
                return self.abandoned(*failure, suspect.writes());
            }
            Entry::Blocked { through, by } => return self.blocked(*through, *by),
            Entry::LineageEnded { .. } => return Ok(()),
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

    fn durable(&mut self, through: Epoch<V>) -> Result<(), Violation<V>> {
        if self
            .sealed
            .get(&through.lineage)
            .is_none_or(|&sealed| through.id > sealed)
        {
            return Err(Violation::DurableNotSealed { through });
        }
        let uncompleted = self
            .outstanding
            .iter()
            .find_map(|(op, pushed)| match pushed.kind {
                OpKind::Write { epoch }
                    if epoch.lineage == through.lineage && epoch.id <= through.id =>
                {
                    Some(*op)
                }
                _ => None,
            });
        if let Some(op) = uncompleted {
            return Err(Violation::DurableBeforeCompletion { through, op });
        }
        if let Some(failure) = self.failures.iter().find_map(|(id, reported)| {
            (!reported.healed && holds(reported, through)).then_some(*id)
        }) {
            return Err(Violation::DurableThroughFailure { through, failure });
        }
        let point = self.durable.entry(through.lineage).or_insert(through.id);
        if through.id > *point {
            *point = through.id;
        }
        Ok(())
    }

    fn failed(
        &mut self,
        failure: V::FailureId,
        suspect: &[SuspectWrite<V>],
        observed: TimePoint<InterruptTime>,
    ) -> Result<(), Violation<V>> {
        if !self.failure_ids.insert(failure) {
            return Err(Violation::DuplicateFailure { failure });
        }
        if let Some(previous) = self.last_stamp.replace(observed)
            && observed < previous
        {
            return Err(Violation::StampWentBackwards {
                failure,
                observed,
                previous,
            });
        }
        for write in suspect {
            if self.writes.get(&write.op) != Some(&write.epoch) {
                return Err(Violation::SuspectNotPushed {
                    failure,
                    op: write.op,
                });
            }
            if self
                .durable
                .get(&write.epoch.lineage)
                .is_some_and(|&durable| write.epoch.id <= durable)
            {
                return Err(Violation::SuspectAlreadyDurable {
                    failure,
                    op: write.op,
                });
            }
        }
        if suspect.windows(2).any(|pair| pair[0].op >= pair[1].op) {
            return Err(Violation::SuspectsOutOfOrder { failure });
        }
        self.failures.insert(
            failure,
            Reported {
                suspect: suspect.iter().map(|w| (w.op, w.epoch)).collect(),
                healed: false,
            },
        );
        Ok(())
    }

    fn abandoned(
        &mut self,
        failure: V::FailureId,
        suspect: &[SuspectWrite<V>],
    ) -> Result<(), Violation<V>> {
        let Some(reported) = self.failures.remove(&failure) else {
            return Err(Violation::AbandonedUnknown { failure });
        };
        if !reported
            .suspect
            .iter()
            .copied()
            .eq(suspect.iter().map(|w| (w.op, w.epoch)))
        {
            return Err(Violation::AbandonedSuspectChanged { failure });
        }
        for (_, epoch) in reported.suspect {
            self.abandoned
                .entry(epoch.lineage)
                .or_default()
                .insert(epoch.id);
        }
        Ok(())
    }

    fn blocked(&self, through: Epoch<V>, by: V::FailureId) -> Result<(), Violation<V>> {
        match self.failures.get(&by) {
            Some(reported) if holds(reported, through) => Ok(()),
            _ => Err(Violation::BlockedByUnrelated { through, by }),
        }
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

/// Whether a reported failure contains an epoch of `through`'s lineage at or below it.
fn holds<V: Identities>(reported: &Reported<V>, through: Epoch<V>) -> bool {
    reported
        .suspect
        .iter()
        .any(|(_, epoch)| epoch.lineage == through.lineage && epoch.id <= through.id)
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
