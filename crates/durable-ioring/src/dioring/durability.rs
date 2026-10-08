// Copyright (c) 2026 Mike Grier
//! One lineage's durability state, without I/O: the writes not yet covered, the seals in progress,
//! the failures not yet resolved, and the high-water mark (DI-D-9, DI-D-12, DI-D-22).
//!
//! Every change returns what it makes due -- flushes to push, and entries to append, in the order
//! they happened -- and the caller does the I/O and appends the entries. So this module decides and
//! the relay acts, as DI-D-18's I/O-free core intends, and these rules are tested without a ring.
//!
//! **How a seal proceeds.** `seal(n)` waits until no write of the lineage at or below `n` is in
//! flight, then names every completed write at or below `n` not yet named, by file, and asks for
//! one flush per file. Each write belongs to exactly one seal -- the first at or above its epoch --
//! because writes at or below a seal are refused once it is made. A seal is finished when every one
//! of its flushes has answered, and the high-water mark advances over finished seals in order (the
//! prefix property), past every epoch no unresolved failure holds.
//!
//! **Failures** (DI-D-12, DI-D-34). Every write is held, in push order, from its push until it is
//! covered successfully, fails, or its epoch is passed by the mark. That record is what a failure's
//! suspect set is frozen from: the held writes whose file the failure reaches, in push order, at the
//! moment it is observed. A failed flush and an import are one transition with the cause as data
//! (DI-1.2 Q4). An epoch passes the mark only when every failure containing it is resolved;
//! abandoning makes its epochs abandoned at once, and a heal takes effect when the first seal made
//! after it finishes with every flush successful.
//!
//! **Time** (DI-D-37, DI-D-38). A failure's observation time is read inside the transition that
//! records it, from the clock the core holds -- `InterruptClock` unless the instance was given
//! another, a mock in these tests -- so stamps are taken in observation order, and a `Steady` clock
//! makes them never decrease in it.
//!
//! **Not yet here**, and left visibly undone rather than approximated: a write to a file a consumer
//! provider serves leaves its seal unfinished, because providers are `DI-3.2.6`'s; and failures are
//! the lineage's own until `DI-3.2.5` makes them the instance's, shared by every lineage with a
//! write in the suspect set.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io;
use std::ops::Bound;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use win_time_sys::{InterruptClock, InterruptTime, TimePoint};

use super::{DomainId, TimeBase};
use crate::contract::EpochId;
use crate::ids::{DioringIds, FailureId, FailureToken, Lineage, OpId};
use crate::types::{
    Cause, Epoch, Failed, FailureInfo, FileKey, Resolution, ResolveError, ResolveRefusal,
    SuspectSet, SuspectWrite,
};

#[cfg(test)]
mod tests;

type V<E> = DioringIds<E>;

/// Who must answer for a file's writes (DI-D-27): the built-in default, by flushing the file,
/// and the consumer's provider, for the domains it serves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Routing {
    /// The file has no declared domain, or one the consumer's provider does not serve.
    pub(crate) default: bool,
    /// The file has a domain the consumer's provider serves.
    pub(crate) provider: bool,
}

/// Which files' writes a failure reaches (DI-D-21).
#[derive(Clone, Debug)]
pub(crate) enum Reach {
    /// Every file.
    All,
    /// Every file whose declared domains intersect these, and every file declared with none.
    Domains(Arc<[DomainId]>),
    /// No file: an import naming a lineage the instance does not have.
    Nothing,
}

impl Reach {
    /// The reach of a failed flush of a file declared with `domains`: a file declared with none is
    /// unknown, so it shares fate with every file.
    pub(crate) fn of_file(domains: &Arc<[DomainId]>) -> Self {
        if domains.is_empty() {
            Reach::All
        } else {
            Reach::Domains(Arc::clone(domains))
        }
    }

    fn reaches(&self, file: &[DomainId]) -> bool {
        match self {
            Reach::All => true,
            Reach::Nothing => false,
            Reach::Domains(domains) => {
                file.is_empty() || file.iter().any(|domain| domains.contains(domain))
            }
        }
    }
}

/// A flush now due: seal `through`'s flush of `file`, through `target`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Flush<E, T> {
    pub(crate) through: E,
    pub(crate) file: FileKey,
    pub(crate) target: T,
}

/// An entry a change made due, in the lineage's terms.
#[derive(Debug)]
pub(crate) enum Event<E: EpochId + 'static> {
    Durable(E),
    Blocked {
        through: E,
        by: FailureId,
    },
    Failed(Failed<V<E>>),
    Abandoned {
        failure: FailureId,
        suspect: SuspectSet<V<E>>,
    },
}

/// What a change made due.
#[derive(Debug)]
pub(crate) struct Due<E: EpochId + 'static, T> {
    /// Flushes to push.
    pub(crate) flushes: Vec<Flush<E, T>>,
    /// Entries to append, in the order they happened.
    pub(crate) events: Vec<Event<E>>,
}

impl<E: EpochId + 'static, T> Default for Due<E, T> {
    fn default() -> Self {
        Self {
            flushes: Vec::new(),
            events: Vec::new(),
        }
    }
}

/// An epoch's state as this lineage sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum State {
    Open,
    Pending,
    Blocked(FailureId),
    Durable,
    Abandoned,
}

/// The answer to a seal request.
#[derive(Debug)]
pub(crate) enum Sealing<E: EpochId + 'static, T> {
    /// A new seal, and what it made due at once.
    Submitted(Due<E, T>),
    /// At or below the seal point: the epoch's state, and nothing changes.
    AlreadySealed(State),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    InFlight,
    /// Completed successfully, and not yet named to a seal.
    Completed,
    /// Named to the seal with this sequence number.
    Named(u64),
}

/// A write not yet covered successfully, failed, or passed by the mark.
struct Write<E, T> {
    op: OpId,
    epoch: E,
    file: FileKey,
    target: T,
    routing: Routing,
    domains: Arc<[DomainId]>,
    stage: Stage,
}

/// One file a seal flushes.
struct SealFile {
    file: FileKey,
    domains: Arc<[DomainId]>,
    answered: bool,
}

enum SealStage {
    /// A write at or below the seal is still in flight.
    Waiting,
    /// Its flushes are out. `failed` once one failed; `provider_owed` while a write of it needs a
    /// provider answer no provider is asked for yet.
    Flushing {
        files: Vec<SealFile>,
        failed: bool,
        provider_owed: bool,
    },
    /// Every flush answered.
    Done { succeeded: bool },
}

struct Seal<E> {
    /// Counts seals made, so a heal knows which seals come after it.
    seq: u64,
    through: E,
    stage: SealStage,
    /// Answered on the queue already, by `Failed` or `Blocked`; `Durable` is appended regardless.
    answered: bool,
}

/// Where an unresolved failure stands.
enum Standing {
    /// Open to resolution; the flag is its token's, set while the token is live.
    Open(Arc<AtomicBool>),
    /// Healed, effective when a seal with this sequence number or later finishes successfully.
    /// The heal holds the token, so the inventory does not hand out another.
    Healing { after: u64 },
}

struct Failure<E: EpochId + 'static> {
    id: FailureId,
    cause: Cause<V<E>>,
    suspect: SuspectSet<V<E>>,
    observed: TimePoint<InterruptTime>,
    /// The epochs of its suspect writes.
    epochs: BTreeSet<E>,
    standing: Standing,
}

/// One lineage's durability state, stamping failures with `K`'s readings.
pub(crate) struct Durability<E: EpochId + 'static, T, K = InterruptClock> {
    lineage: Lineage,
    sealed_through: Option<E>,
    durable_through: Option<E>,
    /// The writes not yet covered, keyed by push order.
    writes: BTreeMap<u64, Write<E, T>>,
    in_flight_by_epoch: BTreeMap<E, usize>,
    seals: VecDeque<Seal<E>>,
    next_seal: u64,
    /// The unresolved failures, in observation order.
    failures: Vec<Failure<E>>,
    next_failure: u64,
    /// Every epoch an abandoned failure contained.
    abandoned: BTreeSet<E>,
    /// The first internal inconsistency observed. Recorded rather than asserted, because most
    /// changes arrive on a pool thread, where a panic aborts the process and names no test; the
    /// consumer's next `pop` asserts on it instead, on the consumer's own thread.
    inconsistency: Option<&'static str>,
    clock: K,
}

impl<E: EpochId + 'static, T, K> Durability<E, T, K> {
    pub(crate) fn new(lineage: Lineage, clock: K) -> Self {
        Self {
            lineage,
            sealed_through: None,
            durable_through: None,
            writes: BTreeMap::new(),
            in_flight_by_epoch: BTreeMap::new(),
            seals: VecDeque::new(),
            next_seal: 0,
            failures: Vec::new(),
            next_failure: 0,
            abandoned: BTreeSet::new(),
            inconsistency: None,
            clock,
        }
    }
}

impl<E: EpochId + 'static, T: Clone, K: TimeBase> Durability<E, T, K> {
    /// The first internal inconsistency observed, if any.
    pub(crate) fn inconsistency(&self) -> Option<&'static str> {
        self.inconsistency
    }

    fn inconsistent(&mut self, what: &'static str) {
        self.inconsistency.get_or_insert(what);
    }

    pub(crate) fn sealed_through(&self) -> Option<E> {
        self.sealed_through
    }

    pub(crate) fn durable_through(&self) -> Option<E> {
        self.durable_through
    }

    /// The seal point, if a write tagged `epoch` would land at or below it (guarantee 6).
    pub(crate) fn refuses(&self, epoch: E) -> Option<E> {
        self.sealed_through.filter(|&sealed| epoch <= sealed)
    }

    /// Whether an abandoned failure contained `epoch`, so that no write may join it (DI-D-35).
    pub(crate) fn is_abandoned(&self, epoch: E) -> bool {
        self.abandoned.contains(&epoch)
    }

    /// An epoch's state. Abandonment is decided when the failure is abandoned, so it is reported
    /// from then on, before the mark passes the epoch and whether or not the epoch is sealed.
    pub(crate) fn state(&self, epoch: E) -> State {
        if self.abandoned.contains(&epoch) {
            return State::Abandoned;
        }
        if self.sealed_through.is_none_or(|sealed| epoch > sealed) {
            return State::Open;
        }
        if self.durable_through.is_some_and(|durable| epoch <= durable) {
            return State::Durable;
        }
        let finished = self
            .seals
            .iter()
            .find(|seal| seal.through >= epoch)
            .is_some_and(|seal| matches!(seal.stage, SealStage::Done { .. }));
        match self.blocking(epoch) {
            Some(by) if finished => State::Blocked(by),
            _ => State::Pending,
        }
    }

    /// A write was accepted by the ring.
    pub(crate) fn pushed(
        &mut self,
        op: OpId,
        epoch: E,
        file: FileKey,
        target: T,
        routing: Routing,
        domains: Arc<[DomainId]>,
    ) {
        debug_assert!(
            self.refuses(epoch).is_none(),
            "a write at or below the seal"
        );
        *self.in_flight_by_epoch.entry(epoch).or_insert(0) += 1;
        self.writes.insert(
            op.seq,
            Write {
                op,
                epoch,
                file,
                target,
                routing,
                domains,
                stage: Stage::InFlight,
            },
        );
    }

    /// A write completed. One that failed is reported on its own completion and is neither named
    /// to a flush nor suspected by a later failure (guarantee 1); a failure observed while it was
    /// in flight keeps it, because suspect sets are frozen.
    pub(crate) fn completed(&mut self, op: OpId, succeeded: bool) -> Due<E, T> {
        let Some(write) = self.writes.get_mut(&op.seq) else {
            self.inconsistent("a write completed that was never recorded as pushed");
            return Due::default();
        };
        if write.stage != Stage::InFlight {
            self.inconsistent("a write completed twice");
            return Due::default();
        }
        let epoch = write.epoch;
        if succeeded {
            write.stage = Stage::Completed;
        } else {
            self.writes.remove(&op.seq);
        }
        if let Some(count) = self.in_flight_by_epoch.get_mut(&epoch) {
            *count -= 1;
            if *count == 0 {
                self.in_flight_by_epoch.remove(&epoch);
            }
        }
        self.advance(Due::default())
    }

    /// Seal every epoch at or below `through` (DI-D-9).
    pub(crate) fn seal(&mut self, through: E) -> Sealing<E, T> {
        if self.sealed_through.is_some_and(|sealed| through <= sealed) {
            return Sealing::AlreadySealed(self.state(through));
        }
        self.sealed_through = Some(through);
        self.seals.push_back(Seal {
            seq: self.next_seal,
            through,
            stage: SealStage::Waiting,
            answered: false,
        });
        self.next_seal += 1;
        Sealing::Submitted(self.advance(Due::default()))
    }

    /// Seal `through`'s flush of `file` completed. Success covers the writes it was issued for; a
    /// failure is observed at once, and is the seal's answer.
    pub(crate) fn flushed(
        &mut self,
        through: E,
        file: FileKey,
        result: io::Result<()>,
    ) -> Due<E, T> {
        let mut due = Due::default();
        let Some(seal) = self.seals.iter_mut().find(|seal| seal.through == through) else {
            self.inconsistent("a flush completed for a seal that does not exist");
            return due;
        };
        let SealStage::Flushing { files, failed, .. } = &mut seal.stage else {
            self.inconsistent("a flush completed for a seal not flushing");
            return due;
        };
        let Some(entry) = files.iter_mut().find(|f| f.file == file && !f.answered) else {
            self.inconsistent("a flush completed that the seal did not issue");
            return due;
        };
        entry.answered = true;
        let seq = seal.seq;
        match result {
            Ok(()) => {
                // A write a provider also answers for stays until the provider has (DI-3.2.6).
                self.writes.retain(|_, write| {
                    write.stage != Stage::Named(seq) || write.file != file || write.routing.provider
                });
            }
            Err(error) => {
                let reach = Reach::of_file(&entry.domains);
                *failed = true;
                seal.answered = true;
                let cause = Cause::Flush {
                    file,
                    error: Arc::new(error),
                };
                self.observe(cause, &reach, &mut due);
            }
        }
        self.advance(due)
    }

    /// A failure the consumer learned of outside the instance (DI-D-12 (h)).
    pub(crate) fn import(&mut self, cause: Cause<V<E>>, reach: &Reach) -> (FailureId, Due<E, T>) {
        let mut due = Due::default();
        let id = self.observe(cause, reach, &mut due);
        (id, self.advance(due))
    }

    /// Resolve failures, validated whole and applied together: the tokens of a refused call come
    /// back, and nothing changes.
    pub(crate) fn resolve(
        &mut self,
        items: Vec<(FailureToken, Resolution)>,
    ) -> Result<Due<E, T>, ResolveError<V<E>>> {
        if let Some(foreign) = items
            .iter()
            .map(|(token, _)| token.id)
            .find(|id| id.instance != self.lineage.instance)
        {
            return Err(ResolveError {
                reason: ResolveRefusal::Foreign(foreign),
                returned: items,
            });
        }
        let mut due = Due::default();
        for (token, resolution) in items {
            // A live token's failure is unresolved: resolving it consumed its only other token.
            let Some(index) = self.failures.iter().position(|f| f.id == token.id) else {
                self.inconsistent("a live token for a failure not in the inventory");
                continue;
            };
            match resolution {
                Resolution::Abandon => {
                    let failure = self.failures.remove(index);
                    self.abandoned.extend(failure.epochs.iter().copied());
                    due.events.push(Event::Abandoned {
                        failure: failure.id,
                        suspect: failure.suspect,
                    });
                }
                Resolution::Heal => {
                    self.failures[index].standing = Standing::Healing {
                        after: self.next_seal,
                    };
                }
            }
        }
        Ok(self.advance(due))
    }

    /// A token for `id`, if it is an unresolved failure whose token is not live.
    pub(crate) fn take_token(&mut self, id: FailureId) -> Option<FailureToken> {
        let failure = self.failures.iter_mut().find(|f| f.id == id)?;
        match &failure.standing {
            Standing::Open(live) if !live.load(Ordering::Acquire) => {
                let (token, live) = FailureToken::mint(id);
                failure.standing = Standing::Open(live);
                Some(token)
            }
            _ => None,
        }
    }

    /// The unresolved failures, in observation order.
    pub(crate) fn failures(&self) -> Vec<FailureInfo<V<E>>> {
        self.failures
            .iter()
            .map(|failure| FailureInfo {
                id: failure.id,
                cause: failure.cause.clone(),
                suspect: failure.suspect.clone(),
                observed: failure.observed,
                token_live: match &failure.standing {
                    Standing::Open(live) => live.load(Ordering::Acquire),
                    Standing::Healing { .. } => true,
                },
            })
            .collect()
    }

    /// Record a failure: its suspect set is every held write its reach covers, in push order,
    /// frozen now (DI-D-12 (b)). The one transition every cause takes.
    fn observe(&mut self, cause: Cause<V<E>>, reach: &Reach, due: &mut Due<E, T>) -> FailureId {
        let lineage = self.lineage;
        let suspect: Vec<SuspectWrite<V<E>>> = self
            .writes
            .values()
            .filter(|write| reach.reaches(&write.domains))
            .map(|write| SuspectWrite {
                op: write.op,
                file: write.file,
                epoch: Epoch::new(lineage, write.epoch),
            })
            .collect();
        let epochs = suspect.iter().map(|write| write.epoch.id).collect();
        let id = FailureId {
            instance: lineage.instance,
            seq: self.next_failure,
        };
        self.next_failure += 1;
        let (token, live) = FailureToken::mint(id);
        let suspect = SuspectSet::new(suspect);
        let observed = self.clock.now();
        self.failures.push(Failure {
            id,
            cause: cause.clone(),
            suspect: suspect.clone(),
            observed,
            epochs,
            standing: Standing::Open(live),
        });
        due.events.push(Event::Failed(Failed {
            id,
            token,
            cause,
            suspect,
            observed,
        }));
        id
    }

    /// The first unresolved failure, in observation order, holding an epoch above the mark and at
    /// or below `through`.
    fn blocking(&self, through: E) -> Option<FailureId> {
        if self
            .durable_through
            .is_some_and(|durable| durable >= through)
        {
            return None;
        }
        let lower = self
            .durable_through
            .map_or(Bound::Unbounded, Bound::Excluded);
        self.failures
            .iter()
            .find(|failure| {
                failure
                    .epochs
                    .range((lower, Bound::Included(through)))
                    .next()
                    .is_some()
            })
            .map(|failure| failure.id)
    }

    /// Start every seal whose writes have all completed, finish every seal whose flushes have all
    /// answered, advance the high-water mark, and answer the seals a failure holds.
    fn advance(&mut self, mut due: Due<E, T>) -> Due<E, T> {
        self.start_seals(&mut due);
        self.finish_seals();
        while let Some(seal) = self.seals.front() {
            if !matches!(seal.stage, SealStage::Done { .. })
                || self.blocking(seal.through).is_some()
            {
                break;
            }
            let through = seal.through;
            self.seals.pop_front();
            self.durable_through = Some(through);
            // Only writes whose flush failed are left at or below a finished seal; once the mark
            // passes them, no later failure can reach back to them (guarantee 3).
            self.writes.retain(|_, write| write.epoch > through);
            due.events.push(Event::Durable(through));
        }
        for index in 0..self.seals.len() {
            let seal = &self.seals[index];
            if seal.answered || !matches!(seal.stage, SealStage::Done { succeeded: true }) {
                continue;
            }
            if let Some(by) = self.blocking(seal.through) {
                let through = seal.through;
                self.seals[index].answered = true;
                due.events.push(Event::Blocked { through, by });
            }
        }
        due
    }

    fn start_seals(&mut self, due: &mut Due<E, T>) {
        let oldest_in_flight = self.in_flight_by_epoch.keys().next().copied();
        for index in 0..self.seals.len() {
            if !matches!(self.seals[index].stage, SealStage::Waiting) {
                continue;
            }
            let through = self.seals[index].through;
            if oldest_in_flight.is_some_and(|oldest| oldest <= through) {
                // Seals are in ascending order, so no later one is ready either.
                break;
            }
            let seq = self.seals[index].seq;
            let mut files: Vec<SealFile> = Vec::new();
            let mut provider_owed = false;
            for write in self.writes.values_mut() {
                if write.stage != Stage::Completed || write.epoch > through {
                    continue;
                }
                write.stage = Stage::Named(seq);
                provider_owed |= write.routing.provider;
                if write.routing.default && !files.iter().any(|f| f.file == write.file) {
                    files.push(SealFile {
                        file: write.file,
                        domains: Arc::clone(&write.domains),
                        answered: false,
                    });
                    due.flushes.push(Flush {
                        through,
                        file: write.file,
                        target: write.target.clone(),
                    });
                }
            }
            self.seals[index].stage = SealStage::Flushing {
                files,
                failed: false,
                provider_owed,
            };
        }
    }

    /// Mark every seal whose flushes have all answered as done. One whose flushes all succeeded
    /// makes effective every heal made before it (DI-D-12 (c)).
    fn finish_seals(&mut self) {
        for index in 0..self.seals.len() {
            let SealStage::Flushing {
                files,
                failed,
                provider_owed,
            } = &self.seals[index].stage
            else {
                continue;
            };
            if *provider_owed || files.iter().any(|f| !f.answered) {
                continue;
            }
            let succeeded = !*failed;
            let seq = self.seals[index].seq;
            self.seals[index].stage = SealStage::Done { succeeded };
            if succeeded {
                self.failures
                    .retain(|f| !matches!(f.standing, Standing::Healing { after } if after <= seq));
            }
        }
    }
}
