// Copyright (c) 2026 Mike Grier
//! An instance's durability state, without I/O: each lineage's seals and high-water mark, the
//! writes not yet covered, and the failures not yet resolved (DI-D-9, DI-D-12, DI-D-19, DI-D-22).
//!
//! Every change returns what it makes due -- flushes to push, and entries to append, in the order
//! they happened -- and the caller does the I/O and appends the entries. So this module decides and
//! the relay acts, as DI-D-18's I/O-free core intends, and these rules are tested without a ring.
//!
//! **Lineages** (DI-D-19). Each lineage has its own seal point, seals, high-water mark and
//! abandoned epochs, keyed by the sequence number its `Lineage` carries; the default lineage's is
//! zero. Writes and failures are the instance's. Minting, ending and retiring are
//! [`lineages`]'s.
//!
//! **How a seal proceeds.** `seal(lineage, n)` waits until no write of the lineage at or below `n`
//! is in flight, then names every completed write of it at or below `n` not yet named, by file, and
//! asks for one flush per file. Each write belongs to exactly one seal -- the first of its lineage
//! at or above its epoch -- because writes at or below a seal are refused once it is made. A seal is
//! finished when every one of its flushes has answered, and a lineage's high-water mark advances
//! over its finished seals in order (the prefix property), past every epoch no failure holds.
//!
//! **Failures** (DI-D-12, DI-D-34). Every write is held, in push order, from its push until it is
//! covered successfully, fails, or its epoch is passed by its lineage's mark. That record is what a
//! failure's suspect set is frozen from: the held writes of every live lineage whose file the
//! failure reaches, in push order, at the moment it is observed. A failed flush and an import are
//! one transition with the cause as data (DI-1.2 Q4). A failure belongs to every lineage it
//! suspects a write of, and holds each: an epoch passes its lineage's mark only when no failure
//! holds it. Abandoning makes its epochs abandoned at once. A heal takes effect per lineage
//! (DI-D-41): the failure stops holding lineage L once L's first seal made after the heal finishes
//! with every flush successful, and is healed once it holds none.
//!
//! **Markings** (DI-D-36, DI-D-39). A suspect set never gains a write, but its failure records
//! what happens to its writes afterwards: a suspect write that completes failed or short, and one
//! whose own file is then flushed successfully, each add a marking to every unresolved failure
//! suspecting it, appended to the failure's record and reported as an entry of its own. They
//! change nothing the mark, a hold or a resolution depends on. A failure abandoned or healed stops
//! gaining them; its `Abandoned` or `Healed` entry carries the record as it ended.
//!
//! **Time** (DI-D-37, DI-D-38). A failure's observation time is read inside the transition that
//! records it, from the clock the core holds -- `InterruptClock` unless the instance was given
//! another, a mock in these tests -- so stamps are taken in observation order, and a `Steady` clock
//! makes them never decrease in it.
//!
//! **Not yet here**, and left visibly undone rather than approximated: a write to a file a consumer
//! provider serves leaves its seal unfinished, because providers are `DI-3.2.6`'s.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io;
use std::ops::Bound;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use win_time_sys::{InterruptClock, InterruptTime, TimePoint};

use super::{DomainId, TimeBase};
use crate::contract::EpochId;
use crate::error_code::ErrorCode;
use crate::ids::{DioringIds, FailureId, FailureToken, InstanceId, Lineage, OpId};
use crate::types::{
    Cause, Epoch, Failed, FailureInfo, FileKey, Marking, MarkingKind, Outcome, Resolution,
    ResolveError, ResolveRefusal, SuspectSet, SuspectWrite,
};

mod lineages;
#[cfg(test)]
mod tests;

type V<E> = DioringIds<E>;

/// A lineage's key within its instance: the sequence number its `Lineage` carries.
pub(crate) type Key = u64;

/// The default lineage's key.
pub(crate) const DEFAULT_LINEAGE: Key = 0;

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
            Reach::Domains(domains) => {
                file.is_empty() || file.iter().any(|domain| domains.contains(domain))
            }
        }
    }
}

/// A flush now due: `lineage`'s seal `through`'s flush of `file`, through `target`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Flush<E, T> {
    pub(crate) lineage: Key,
    pub(crate) through: E,
    pub(crate) file: FileKey,
    pub(crate) target: T,
}

/// An entry a change made due, in the core's terms.
#[derive(Debug)]
pub(crate) enum Event<E: EpochId + 'static> {
    Durable {
        lineage: Key,
        through: E,
    },
    Blocked {
        lineage: Key,
        through: E,
        by: FailureId,
    },
    Failed(Failed<V<E>>),
    Abandoned {
        failure: FailureId,
        suspect: SuspectSet<V<E>>,
        markings: Vec<Marking<V<E>>>,
    },
    Healed {
        failure: FailureId,
        suspect: SuspectSet<V<E>>,
        markings: Vec<Marking<V<E>>>,
    },
    Marked {
        failure: FailureId,
        marking: Marking<V<E>>,
    },
    LineageEnded {
        lineage: Key,
        abandoned_through: Option<E>,
    },
}

/// How a write ended, as the core needs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WriteEnd {
    /// Completed, with the byte count its completion reported.
    Transferred(u32),
    /// Completed as failed, with the completion's code.
    Failed(Option<ErrorCode>),
}

impl WriteEnd {
    /// How a completion's outcome ended the write; `None` for one never issued, which no write
    /// the core records can be.
    pub(crate) fn of<I: crate::contract::Identities>(outcome: &Outcome<I>) -> Option<Self> {
        match outcome {
            Outcome::Transferred(transferred) => Some(Self::Transferred(*transferred)),
            Outcome::Failed(error) => Some(Self::Failed(ErrorCode::of(error))),
            Outcome::NeverIssued { .. } => None,
        }
    }
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

/// An epoch's state as its lineage sees it.
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
    /// Named to its lineage's seal with this sequence number.
    Named(u64),
}

/// A write the ring accepted, as the core records it.
pub(crate) struct Accepted<E, T> {
    pub(crate) op: OpId,
    pub(crate) lineage: Key,
    pub(crate) epoch: E,
    pub(crate) file: FileKey,
    /// What its seal's flush is pushed against.
    pub(crate) target: T,
    pub(crate) routing: Routing,
    pub(crate) domains: Arc<[DomainId]>,
    /// The byte count it asks to write.
    pub(crate) len: u32,
}

/// A write not yet covered successfully, failed, or passed by its lineage's mark.
struct Write<E, T> {
    op: OpId,
    lineage: Key,
    epoch: E,
    file: FileKey,
    target: T,
    routing: Routing,
    domains: Arc<[DomainId]>,
    /// The byte count it asked to write, against which a completion is short.
    len: u32,
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
    /// Counts the lineage's seals, so a heal knows which seals come after it.
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
    /// Healed, and still holding the lineages named here, each until its seal with this sequence
    /// number or later finishes successfully (DI-D-41). Healed once it names none. The heal holds
    /// the token, so the inventory does not hand out another.
    Healing { pending: BTreeMap<Key, u64> },
}

struct Failure<E: EpochId + 'static> {
    id: FailureId,
    cause: Cause<V<E>>,
    suspect: SuspectSet<V<E>>,
    observed: TimePoint<InterruptTime>,
    markings: Vec<Marking<V<E>>>,
    /// The epochs of its suspect writes, by lineage: the lineages it belongs to.
    epochs: BTreeMap<Key, BTreeSet<E>>,
    standing: Standing,
}

impl<E: EpochId + 'static> Failure<E> {
    /// The epochs of `lineage` this failure holds, if it holds the lineage at all.
    fn holds(&self, lineage: Key) -> Option<&BTreeSet<E>> {
        let epochs = self.epochs.get(&lineage)?;
        match &self.standing {
            Standing::Open(_) => Some(epochs),
            Standing::Healing { pending } => pending.contains_key(&lineage).then_some(epochs),
        }
    }
}

/// One lineage's own state.
struct LineageState<E> {
    description: Option<Arc<str>>,
    sealed_through: Option<E>,
    durable_through: Option<E>,
    in_flight_by_epoch: BTreeMap<E, usize>,
    seals: VecDeque<Seal<E>>,
    next_seal: u64,
    /// Every epoch an abandoned failure contained.
    abandoned: BTreeSet<E>,
    /// Ended (DI-D-30): it answers nothing more, and is retired once its writes in flight complete.
    ended: bool,
}

impl<E: EpochId> LineageState<E> {
    fn new(description: Option<Arc<str>>) -> Self {
        Self {
            description,
            sealed_through: None,
            durable_through: None,
            in_flight_by_epoch: BTreeMap::new(),
            seals: VecDeque::new(),
            next_seal: 0,
            abandoned: BTreeSet::new(),
            ended: false,
        }
    }

    /// Record seal `through`'s flush of `file` as answered, and as failed if it did: the seal's
    /// sequence number and the file's domains, or what about the answer is inconsistent.
    fn answer(
        &mut self,
        through: E,
        file: FileKey,
        failed_now: bool,
    ) -> Result<(u64, Arc<[DomainId]>), &'static str> {
        let seal = self
            .seals
            .iter_mut()
            .find(|seal| seal.through == through)
            .ok_or("a flush completed for a seal that does not exist")?;
        let SealStage::Flushing { files, failed, .. } = &mut seal.stage else {
            return Err("a flush completed for a seal not flushing");
        };
        let entry = files
            .iter_mut()
            .find(|f| f.file == file && !f.answered)
            .ok_or("a flush completed that the seal did not issue")?;
        entry.answered = true;
        let domains = Arc::clone(&entry.domains);
        if failed_now {
            *failed = true;
            seal.answered = true;
        }
        Ok((seal.seq, domains))
    }
}

/// An instance's durability state, stamping failures with `K`'s readings.
pub(crate) struct Durability<E: EpochId + 'static, T, K = InterruptClock> {
    instance: InstanceId,
    /// Every lineage not yet retired, by key.
    lineages: BTreeMap<Key, LineageState<E>>,
    next_lineage: Key,
    /// The writes not yet covered, keyed by push order.
    writes: BTreeMap<u64, Write<E, T>>,
    /// The unresolved failures, in observation order.
    failures: Vec<Failure<E>>,
    next_failure: u64,
    /// The first internal inconsistency observed. Recorded rather than asserted, because most
    /// changes arrive on a pool thread, where a panic aborts the process and names no test; the
    /// consumer's next `pop` asserts on it instead, on the consumer's own thread.
    inconsistency: Option<&'static str>,
    clock: K,
}

impl<E: EpochId + 'static, T, K> Durability<E, T, K> {
    /// An instance's state, with its default lineage.
    pub(crate) fn new(instance: InstanceId, clock: K) -> Self {
        let mut lineages = BTreeMap::new();
        lineages.insert(DEFAULT_LINEAGE, LineageState::new(None));
        Self {
            instance,
            lineages,
            next_lineage: DEFAULT_LINEAGE + 1,
            writes: BTreeMap::new(),
            failures: Vec::new(),
            next_failure: 0,
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

    /// A lineage's `Lineage` name.
    fn name(&self, lineage: Key) -> Lineage {
        Lineage {
            instance: self.instance,
            seq: lineage,
        }
    }

    /// The lineage's state, if it is live: minted, and neither ended nor retired.
    fn live(&self, lineage: Key) -> Option<&LineageState<E>> {
        self.lineages.get(&lineage).filter(|state| !state.ended)
    }

    /// The lineage's seal point; `None` for a lineage not live.
    pub(crate) fn sealed_through(&self, lineage: Key) -> Option<Option<E>> {
        self.live(lineage).map(|state| state.sealed_through)
    }

    /// The lineage's high-water mark; `None` for a lineage not live.
    pub(crate) fn durable_through(&self, lineage: Key) -> Option<Option<E>> {
        self.live(lineage).map(|state| state.durable_through)
    }

    /// The seal point, if a write to `lineage` tagged `epoch` would land at or below it
    /// (guarantee 6).
    pub(crate) fn refuses(&self, lineage: Key, epoch: E) -> Option<E> {
        self.live(lineage)?
            .sealed_through
            .filter(|&sealed| epoch <= sealed)
    }

    /// Whether an abandoned failure contained `lineage`'s `epoch`, so that no write may join it
    /// (DI-D-35).
    pub(crate) fn is_abandoned(&self, lineage: Key, epoch: E) -> bool {
        self.live(lineage)
            .is_some_and(|state| state.abandoned.contains(&epoch))
    }

    /// An epoch's state; `None` for a lineage not live. Abandonment is decided when the failure is
    /// abandoned, so it is reported from then on, before the mark passes the epoch and whether or
    /// not the epoch is sealed.
    pub(crate) fn state(&self, lineage: Key, epoch: E) -> Option<State> {
        let state = self.live(lineage)?;
        if state.abandoned.contains(&epoch) {
            return Some(State::Abandoned);
        }
        if state.sealed_through.is_none_or(|sealed| epoch > sealed) {
            return Some(State::Open);
        }
        if state
            .durable_through
            .is_some_and(|durable| epoch <= durable)
        {
            return Some(State::Durable);
        }
        let finished = state
            .seals
            .iter()
            .find(|seal| seal.through >= epoch)
            .is_some_and(|seal| matches!(seal.stage, SealStage::Done { .. }));
        Some(match self.blocking(lineage, epoch) {
            Some(by) if finished => State::Blocked(by),
            _ => State::Pending,
        })
    }

    /// A write was accepted by the ring.
    pub(crate) fn pushed(&mut self, write: Accepted<E, T>) {
        let Accepted {
            op,
            lineage,
            epoch,
            file,
            target,
            routing,
            domains,
            len,
        } = write;
        let Some(state) = self.lineages.get_mut(&lineage).filter(|state| !state.ended) else {
            self.inconsistent("a write was pushed to a lineage that is not live");
            return;
        };
        debug_assert!(
            state.sealed_through.is_none_or(|sealed| epoch > sealed),
            "a write at or below the seal"
        );
        *state.in_flight_by_epoch.entry(epoch).or_insert(0) += 1;
        self.writes.insert(
            op.seq,
            Write {
                op,
                lineage,
                epoch,
                file,
                target,
                routing,
                domains,
                len,
                stage: Stage::InFlight,
            },
        );
    }

    /// A write completed. One that failed is reported on its own completion and is neither named
    /// to a flush nor suspected by a later failure (guarantee 1); a failure observed while it was
    /// in flight keeps it, because suspect sets are frozen, and marks it nullified. One that
    /// completed short marks each failure suspecting it, and stays held. A write of an ended
    /// lineage is let go either way.
    pub(crate) fn completed(&mut self, op: OpId, end: WriteEnd) -> Due<E, T> {
        let mut due = Due::default();
        let Some(write) = self.writes.get_mut(&op.seq) else {
            self.inconsistent("a write completed that was never recorded as pushed");
            return due;
        };
        if write.stage != Stage::InFlight {
            self.inconsistent("a write completed twice");
            return due;
        }
        let (lineage, epoch, len) = (write.lineage, write.epoch, write.len);
        let ended = self.lineages.get(&lineage).is_none_or(|state| state.ended);
        match end {
            WriteEnd::Transferred(transferred) => {
                write.stage = Stage::Completed;
                if ended {
                    self.writes.remove(&op.seq);
                }
                if transferred < len {
                    self.mark(op, MarkingKind::Short { transferred }, &mut due);
                }
            }
            WriteEnd::Failed(code) => {
                self.writes.remove(&op.seq);
                self.mark(op, MarkingKind::Nullified { code }, &mut due);
            }
        }
        if let Some(state) = self.lineages.get_mut(&lineage)
            && let Some(count) = state.in_flight_by_epoch.get_mut(&epoch)
        {
            *count -= 1;
            if *count == 0 {
                state.in_flight_by_epoch.remove(&epoch);
            }
        }
        if ended {
            self.retire_if_drained(lineage);
        }
        self.advance(due)
    }

    /// Seal every epoch of `lineage` at or below `through` (DI-D-9). The lineage must be live.
    pub(crate) fn seal(&mut self, lineage: Key, through: E) -> Sealing<E, T> {
        let Some(state) = self.lineages.get_mut(&lineage).filter(|state| !state.ended) else {
            self.inconsistent("a seal of a lineage that is not live");
            return Sealing::Submitted(Due::default());
        };
        if state.sealed_through.is_some_and(|sealed| through <= sealed) {
            let answer = self.state(lineage, through).unwrap_or(State::Pending);
            return Sealing::AlreadySealed(answer);
        }
        state.sealed_through = Some(through);
        let seq = state.next_seal;
        state.next_seal += 1;
        state.seals.push_back(Seal {
            seq,
            through,
            stage: SealStage::Waiting,
            answered: false,
        });
        Sealing::Submitted(self.advance(Due::default()))
    }

    /// `lineage`'s seal `through`'s flush of `file` completed. Success covers the writes it was
    /// issued for; a failure is observed at once, and is the seal's answer. A flush of an ended
    /// lineage reports nothing (DI-D-30).
    pub(crate) fn flushed(
        &mut self,
        lineage: Key,
        through: E,
        file: FileKey,
        result: io::Result<()>,
    ) -> Due<E, T> {
        let mut due = Due::default();
        let answered = match self.lineages.get_mut(&lineage) {
            Some(state) if !state.ended => state.answer(through, file, result.is_err()),
            _ => return due,
        };
        let (seq, domains) = match answered {
            Ok(answered) => answered,
            Err(what) => {
                self.inconsistent(what);
                return due;
            }
        };
        match result {
            Ok(()) => {
                // A write a provider also answers for stays until the provider has (DI-3.2.6).
                let covered = |write: &Write<E, T>| {
                    write.lineage == lineage
                        && write.stage == Stage::Named(seq)
                        && write.file == file
                        && !write.routing.provider
                };
                let ops: Vec<OpId> = self
                    .writes
                    .values()
                    .filter(|write| covered(write))
                    .map(|write| write.op)
                    .collect();
                self.writes.retain(|_, write| !covered(write));
                for op in ops {
                    self.mark(op, MarkingKind::Covered, &mut due);
                }
            }
            Err(error) => {
                let cause = Cause::Flush {
                    file,
                    error: Arc::new(error),
                };
                self.observe(cause, &Reach::of_file(&domains), None, &mut due);
            }
        }
        self.advance(due)
    }

    /// A failure the consumer learned of outside the instance (DI-D-12 (h)), reaching the writes
    /// `reach` covers -- of lineage `only`, if one is named.
    pub(crate) fn import(
        &mut self,
        cause: Cause<V<E>>,
        reach: &Reach,
        only: Option<Key>,
    ) -> (FailureId, Due<E, T>) {
        let mut due = Due::default();
        let id = self.observe(cause, reach, only, &mut due);
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
            .find(|id| id.instance != self.instance)
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
                    for (lineage, epochs) in &failure.epochs {
                        if let Some(state) = self.lineages.get_mut(lineage) {
                            state.abandoned.extend(epochs.iter().copied());
                        }
                    }
                    due.events.push(Event::Abandoned {
                        failure: failure.id,
                        suspect: failure.suspect,
                        markings: failure.markings,
                    });
                }
                Resolution::Heal => {
                    let lineages = &self.lineages;
                    let pending = self.failures[index]
                        .epochs
                        .keys()
                        .filter_map(|key| {
                            lineages
                                .get(key)
                                .filter(|state| !state.ended)
                                .map(|state| (*key, state.next_seal))
                        })
                        .collect();
                    self.failures[index].standing = Standing::Healing { pending };
                }
            }
        }
        self.release_healed(&mut due);
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
                markings: failure.markings.clone(),
                token_live: match &failure.standing {
                    Standing::Open(live) => live.load(Ordering::Acquire),
                    Standing::Healing { .. } => true,
                },
            })
            .collect()
    }

    /// Record a failure: its suspect set is every held write of a live lineage -- of `only`, if
    /// named -- whose file its reach covers, in push order, frozen now (DI-D-12 (b)). The one
    /// transition every cause takes.
    fn observe(
        &mut self,
        cause: Cause<V<E>>,
        reach: &Reach,
        only: Option<Key>,
        due: &mut Due<E, T>,
    ) -> FailureId {
        let instance = self.instance;
        let lineages = &self.lineages;
        let suspect: Vec<SuspectWrite<V<E>>> = self
            .writes
            .values()
            .filter(|write| only.is_none_or(|lineage| write.lineage == lineage))
            .filter(|write| lineages.get(&write.lineage).is_some_and(|s| !s.ended))
            .filter(|write| reach.reaches(&write.domains))
            .map(|write| SuspectWrite {
                op: write.op,
                file: write.file,
                epoch: Epoch::new(
                    Lineage {
                        instance,
                        seq: write.lineage,
                    },
                    write.epoch,
                ),
            })
            .collect();
        let mut epochs: BTreeMap<Key, BTreeSet<E>> = BTreeMap::new();
        for write in &suspect {
            epochs
                .entry(write.epoch.lineage.seq)
                .or_default()
                .insert(write.epoch.id);
        }
        let id = FailureId {
            instance,
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
            markings: Vec::new(),
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

    /// Mark every unresolved failure that suspects `write` (DI-D-36), with one reading of the
    /// clock, in observation order.
    fn mark(&mut self, write: OpId, kind: MarkingKind, due: &mut Due<E, T>) {
        let mut observed = None;
        for failure in &mut self.failures {
            let suspects = failure
                .suspect
                .writes()
                .binary_search_by_key(&write.seq, |suspect| suspect.op.seq)
                .is_ok_and(|at| failure.suspect.writes()[at].op == write);
            if !suspects {
                continue;
            }
            let marking = Marking {
                write,
                observed: *observed.get_or_insert_with(|| self.clock.now()),
                kind,
            };
            failure.markings.push(marking.clone());
            due.events.push(Event::Marked {
                failure: failure.id,
                marking,
            });
        }
    }

    /// Resolve as healed every healing failure that holds no lineage any more, each reported by its
    /// `Healed` entry (DI-D-41).
    fn release_healed(&mut self, due: &mut Due<E, T>) {
        let mut at = 0;
        while at < self.failures.len() {
            if matches!(&self.failures[at].standing, Standing::Healing { pending } if pending.is_empty())
            {
                let failure = self.failures.remove(at);
                due.events.push(Event::Healed {
                    failure: failure.id,
                    suspect: failure.suspect,
                    markings: failure.markings,
                });
            } else {
                at += 1;
            }
        }
    }

    /// The first unresolved failure, in observation order, holding an epoch of `lineage` above its
    /// mark and at or below `through`.
    fn blocking(&self, lineage: Key, through: E) -> Option<FailureId> {
        let durable = self.lineages.get(&lineage)?.durable_through;
        if durable.is_some_and(|durable| durable >= through) {
            return None;
        }
        let lower = durable.map_or(Bound::Unbounded, Bound::Excluded);
        self.failures
            .iter()
            .find(|failure| {
                failure.holds(lineage).is_some_and(|epochs| {
                    epochs
                        .range((lower, Bound::Included(through)))
                        .next()
                        .is_some()
                })
            })
            .map(|failure| failure.id)
    }

    /// For every live lineage: start every seal whose writes have all completed, and finish every
    /// seal whose flushes have all answered. Then report the heals that took effect, advance each
    /// lineage's high-water mark, and answer the seals a failure holds.
    fn advance(&mut self, mut due: Due<E, T>) -> Due<E, T> {
        let live: Vec<Key> = self
            .lineages
            .iter()
            .filter(|(_, state)| !state.ended)
            .map(|(key, _)| *key)
            .collect();
        for &lineage in &live {
            self.start_seals(lineage, &mut due);
            self.finish_seals(lineage);
        }
        self.release_healed(&mut due);
        for &lineage in &live {
            self.advance_mark(lineage, &mut due);
        }
        due
    }

    /// Advance `lineage`'s high-water mark over its finished seals, in order, and answer the seals
    /// a failure holds.
    fn advance_mark(&mut self, lineage: Key, due: &mut Due<E, T>) {
        while let Some(seal) = self.lineages.get(&lineage).and_then(|s| s.seals.front()) {
            if !matches!(seal.stage, SealStage::Done { .. })
                || self.blocking(lineage, seal.through).is_some()
            {
                break;
            }
            let through = seal.through;
            let state = self.lineages.get_mut(&lineage).expect("found above");
            state.seals.pop_front();
            state.durable_through = Some(through);
            // Only writes whose flush failed are left at or below a finished seal; once the mark
            // passes them, no later failure can reach back to them (guarantee 3).
            self.writes
                .retain(|_, write| write.lineage != lineage || write.epoch > through);
            due.events.push(Event::Durable { lineage, through });
        }
        let count = self.lineages.get(&lineage).map_or(0, |s| s.seals.len());
        for index in 0..count {
            let seal = &self.lineages[&lineage].seals[index];
            if seal.answered || !matches!(seal.stage, SealStage::Done { succeeded: true }) {
                continue;
            }
            let through = seal.through;
            if let Some(by) = self.blocking(lineage, through) {
                let state = self.lineages.get_mut(&lineage).expect("counted above");
                state.seals[index].answered = true;
                due.events.push(Event::Blocked {
                    lineage,
                    through,
                    by,
                });
            }
        }
    }

    fn start_seals(&mut self, lineage: Key, due: &mut Due<E, T>) {
        let Some(state) = self.lineages.get_mut(&lineage) else {
            return;
        };
        let oldest_in_flight = state.in_flight_by_epoch.keys().next().copied();
        for index in 0..state.seals.len() {
            if !matches!(state.seals[index].stage, SealStage::Waiting) {
                continue;
            }
            let through = state.seals[index].through;
            if oldest_in_flight.is_some_and(|oldest| oldest <= through) {
                // Seals are in ascending order, so no later one is ready either.
                break;
            }
            let seq = state.seals[index].seq;
            let mut files: Vec<SealFile> = Vec::new();
            let mut provider_owed = false;
            for write in self.writes.values_mut() {
                if write.lineage != lineage
                    || write.stage != Stage::Completed
                    || write.epoch > through
                {
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
                        lineage,
                        through,
                        file: write.file,
                        target: write.target.clone(),
                    });
                }
            }
            state.seals[index].stage = SealStage::Flushing {
                files,
                failed: false,
                provider_owed,
            };
        }
    }

    /// Mark every seal of `lineage` whose flushes have all answered as done. One whose flushes all
    /// succeeded makes every heal made before it effective in this lineage (DI-D-41); a failure is
    /// healed once that has happened in every lineage it held.
    fn finish_seals(&mut self, lineage: Key) {
        let Some(state) = self.lineages.get_mut(&lineage) else {
            return;
        };
        for seal in &mut state.seals {
            let SealStage::Flushing {
                files,
                failed,
                provider_owed,
            } = &seal.stage
            else {
                continue;
            };
            if *provider_owed || files.iter().any(|f| !f.answered) {
                continue;
            }
            let succeeded = !*failed;
            let seq = seal.seq;
            seal.stage = SealStage::Done { succeeded };
            if succeeded {
                for failure in &mut self.failures {
                    if let Standing::Healing { pending } = &mut failure.standing
                        && pending.get(&lineage).is_some_and(|&after| after <= seq)
                    {
                        pending.remove(&lineage);
                    }
                }
            }
        }
    }

    /// Retire an ended lineage whose writes in flight have all completed.
    fn retire_if_drained(&mut self, lineage: Key) {
        if self
            .lineages
            .get(&lineage)
            .is_some_and(|state| state.ended && state.in_flight_by_epoch.is_empty())
        {
            self.lineages.remove(&lineage);
        }
    }
}
