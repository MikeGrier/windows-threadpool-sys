// Copyright (c) 2026 Mike Grier
//! An instance's durability state, without I/O: each lineage's seals and high-water mark, the
//! writes not yet covered, and the failures not yet resolved (DI-D-9, DI-D-12, DI-D-19, DI-D-22).
//!
//! Every change returns what it makes due -- flushes to push, and entries to append, in the order
//! they happened -- and the caller does the I/O and appends the entries. So this module decides and
//! its caller, the I/O core `DurableIoCore`, acts, as DI-D-18's I/O-free core intends, and these
//! rules are tested without a ring.
//!
//! **Lineages** (DI-D-19). Each lineage has its own seal point, seals, high-water mark and
//! abandoned epochs, keyed by the sequence number its `Lineage` carries; the default lineage's is
//! zero. Writes and failures are the instance's. Minting, ending and retiring are
//! [`lineages`]'s.
//!
//! **How a seal proceeds.** `seal(lineage, n)` waits until no write of the lineage at or below `n`
//! is in flight, then names every completed write of it at or below `n` not yet named, and asks
//! for one flush per file the built-in default answers for, and one request to the consumer's
//! provider naming, per domain it serves, each file's writes (DI-D-27). Each write belongs to
//! exactly one seal -- the first of its lineage at or above its epoch -- because writes at or below
//! a seal are refused once it is made. A write is covered successfully once every part of the seal
//! its file depends on has answered success: its file's flush, if the default answers for it, and
//! each domain of it the provider serves. A seal is finished when every part has answered, and a
//! lineage's high-water mark advances over its finished seals in order (the prefix property), past
//! every epoch no failure holds.
//!
//! **Failures** (DI-D-12, DI-D-34). Every write is held, in push order, from its push until it is
//! covered successfully, fails, or its epoch is passed by its lineage's mark. That record is what a
//! failure's suspect set is frozen from: the held writes of every live lineage whose file the
//! failure reaches, in push order, at the moment it is observed. A failed flush and an import are
//! one transition with the cause as data (DI-1.2 Q4). A failure belongs to every lineage it
//! suspects a write of, and holds each: an epoch passes its lineage's mark only when no failure
//! holds it. Abandoning makes its epochs abandoned at once. A heal takes effect per lineage
//! (DI-D-41, DI-D-46): the failure stops holding lineage L once L's first seal made after the heal
//! that reaches the highest epoch L had been pushed then finishes with every part successful, and
//! is healed once it holds none.
//!
//! **Inconsistencies** (DI-D-46). A state the core cannot be in is reported to the diagnostic sink
//! (DI-D-51) and panicked on where it is found, in every build; see [`inconsistent`].
//!
//! **Markings** (DI-D-36, DI-D-39). A suspect set never gains a write, but its failure records
//! what happens to its writes afterwards: a suspect write that completes with a fault, and one
//! whose own file is then flushed successfully, each add a marking to every unresolved failure
//! suspecting it, appended to the failure's record and reported as an entry of its own. They
//! change nothing the mark, a hold or a resolution depends on. A failure abandoned or healed stops
//! gaining them; its `Abandoned` or `Healed` entry carries the record as it ended.
//!
//! **Time** (DI-D-37, DI-D-38, DI-D-50). A failure's observation time is read inside the transition
//! that records it, from the [`StampClock`] the core holds -- the driver's, interrupt time for the
//! Windows driver, or a mock in these tests -- so stamps are taken in observation order, and a
//! clock that never runs backwards makes them never decrease in it. The core names no time type of
//! its own: its stamps are the clock's `Instant`.
//!
//! **The consumer's provider** (DI-D-27, DI-D-44) is asked through [`Due::calls`], and answers per
//! domain through [`providers`]; a failed or abandoned domain is observed as a failed flush is, with
//! its own reach: the files whose declared domains contain it, and every file declared with none.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::fmt;
use std::ops::Bound;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::clock::{Instant, StampClock};
use crate::contract::EpochId;
use crate::correlation::CorrelationId;
use crate::diagnostics::{
    Diagnostic, DiagnosticKind, DiagnosticSink, FlushOutcome, FlushPart, SealOutcome,
};
use crate::domains::DomainId;
use crate::ids::{DioringIds, FailureId, FailureToken, InstanceId, Lineage, OpId};
use crate::types::{
    Entry, Epoch, Failed, FailureCause, FailureInfo, FlushDomain, IoFault, IoTargetKey, Marking,
    MarkingKind, Resolution, ResolveError, ResolveRefusal, SuspectSet, SuspectWrite,
};

mod files;
mod gates;
mod lineages;
mod providers;

pub(crate) use gates::GateState;
#[cfg(test)]
mod tests;

type V<E, I> = DioringIds<E, I>;

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
    pub(crate) file: IoTargetKey,
    pub(crate) target: T,
}

/// An entry a change made due, in the core's terms.
#[derive(Debug)]
pub(crate) enum Event<E: EpochId + 'static, I: Instant> {
    Durable {
        lineage: Key,
        through: E,
    },
    Blocked {
        lineage: Key,
        through: E,
        by: FailureId,
    },
    Failed(Failed<V<E, I>>),
    Abandoned {
        failure: FailureId,
        suspect: SuspectSet<V<E, I>>,
        markings: Vec<Marking<V<E, I>>>,
    },
    Healed {
        failure: FailureId,
        suspect: SuspectSet<V<E, I>>,
        markings: Vec<Marking<V<E, I>>>,
    },
    Marked {
        failure: FailureId,
        marking: Marking<V<E, I>>,
    },
    LineageEnded {
        lineage: Key,
        abandoned_through: Option<E>,
    },
    /// A held operation will never be issued: the epoch named was abandoned -- its gate, or its own
    /// (DI-D-43).
    NeverIssued {
        op: OpId,
        abandoned: Gate<E>,
    },
}

/// An operation never issued, with the abandoned epoch that stopped it.
pub(crate) type NeverIssued<E> = (OpId, Gate<E>);

impl<E: EpochId + 'static, I: Instant> Event<E, I> {
    /// The entry this event is, for `instance`'s queue -- or, for an operation never issued, the
    /// operation and the abandoned epoch, which only the holder of the operation can make an entry
    /// of.
    pub(crate) fn into_entry<B, C>(
        self,
        instance: InstanceId,
    ) -> Result<Entry<V<E, I>, B, C>, NeverIssued<E>> {
        let name = |seq| Lineage { instance, seq };
        Ok(match self {
            Event::Durable { lineage, through } => Entry::Durable {
                through: Epoch::new(name(lineage), through),
            },
            Event::Blocked {
                lineage,
                through,
                by,
            } => Entry::Blocked {
                through: Epoch::new(name(lineage), through),
                by,
            },
            Event::Failed(failed) => Entry::Failed(failed),
            Event::Abandoned {
                failure,
                suspect,
                markings,
            } => Entry::Abandoned {
                failure,
                suspect,
                markings,
            },
            Event::Healed {
                failure,
                suspect,
                markings,
            } => Entry::Healed {
                failure,
                suspect,
                markings,
            },
            Event::Marked { failure, marking } => Entry::Marked { failure, marking },
            Event::LineageEnded {
                lineage,
                abandoned_through,
            } => Entry::LineageEnded {
                lineage: name(lineage),
                abandoned_through,
            },
            Event::NeverIssued { op, abandoned } => return Err((op, abandoned)),
        })
    }
}

/// How a write ended, as the core needs it. A target never completes a write short (DI-D-50), so a
/// write either succeeded in full or failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WriteEnd {
    /// Completed in full.
    Succeeded,
    /// Completed with a fault, as the backend reported it.
    Failed(IoFault),
}

/// What a change made due.
#[derive(Debug)]
pub(crate) struct Due<E: EpochId + 'static, T, I: Instant> {
    /// Flushes to push.
    pub(crate) flushes: Vec<Flush<E, T>>,
    /// Entries to append, in the order they happened.
    pub(crate) events: Vec<Event<E, I>>,
    /// Held operations whose gates are now durable, to push, in the order they were released.
    pub(crate) released: Vec<OpId>,
    /// Requests for the consumer's provider, one per seal that needs it, in the order made.
    pub(crate) calls: Vec<ProviderCall>,
}

impl<E: EpochId + 'static, T, I: Instant> Default for Due<E, T, I> {
    fn default() -> Self {
        Self {
            flushes: Vec::new(),
            events: Vec::new(),
            released: Vec::new(),
            calls: Vec::new(),
        }
    }
}

/// One seal's request for the consumer's provider (DI-D-27), in the core's terms: per domain the
/// provider serves that the seal touched, each file's writes named for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProviderCall {
    pub(crate) lineage: Key,
    /// The seal's sequence number, which its answers name.
    pub(crate) seal: u64,
    /// By domain, ascending; within one, files in the order first written.
    pub(crate) domains: Vec<DomainPart>,
}

/// One domain's part of a provider request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DomainPart {
    pub(crate) domain: DomainId,
    /// Each file, with the writes named for this domain, in push order.
    pub(crate) files: Vec<(IoTargetKey, Vec<OpId>)>,
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
pub(crate) enum Sealing<E: EpochId + 'static, T, I: Instant> {
    /// A new seal, and what it made due at once.
    Submitted(Due<E, T, I>),
    /// At or below the seal point: the epoch's state, and nothing changes.
    AlreadySealed(State),
    /// Refused, and nothing changes: the seal would close this gate cycle (DI-D-43).
    Cycle(Vec<Gate<E>>),
}

/// An epoch of a lineage, in the core's terms: what a gate names, and what a cycle lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Gate<E> {
    pub(crate) lineage: Key,
    pub(crate) epoch: E,
}

/// An operation held for its gate (DI-D-23): a gated write, with what it will be recorded as
/// once issued, or a gated read.
struct HeldOp<E, T> {
    op: OpId,
    write: Option<Accepted<E, T>>,
    gate: Gate<E>,
    /// The gate is durable, and the operation waits only to be pushed.
    ready: bool,
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
    pub(crate) file: IoTargetKey,
    /// What its seal's flush is pushed against.
    pub(crate) target: T,
    pub(crate) routing: Routing,
    pub(crate) domains: Arc<[DomainId]>,
}

/// A write not yet covered successfully, failed, or passed by its lineage's mark.
struct Write<E, T> {
    op: OpId,
    lineage: Key,
    epoch: E,
    file: IoTargetKey,
    target: T,
    routing: Routing,
    domains: Arc<[DomainId]>,
    stage: Stage,
}

/// One file a seal flushes.
struct SealFile {
    file: IoTargetKey,
    domains: Arc<[DomainId]>,
    answered: bool,
    /// Answered with success.
    ok: bool,
}

/// One domain a seal asked the consumer's provider about.
struct SealDomain {
    domain: DomainId,
    answered: bool,
    /// Answered with success.
    ok: bool,
}

enum SealStage {
    /// A write at or below the seal is still in flight.
    Waiting,
    /// Its flushes, and its request to the consumer's provider, are out. `failed` once one part
    /// failed.
    Flushing {
        files: Vec<SealFile>,
        domains: Vec<SealDomain>,
        failed: bool,
    },
    /// Every part answered.
    Done { succeeded: bool },
}

struct Seal<E> {
    /// Counts the lineage's seals, so a heal knows which seals come after it.
    seq: u64,
    through: E,
    stage: SealStage,
    /// Answered on the queue already, by `Failed` or `Blocked`; `Durable` is appended regardless.
    answered: bool,
    /// Its pair's id while the pair is open: taken as the pair ends, at the seal's first answer,
    /// so it ends once (DI-D-87).
    pair: Option<CorrelationId>,
}

/// Where an unresolved failure stands.
enum Standing<E> {
    /// Open to resolution; the flag is its token's, set while the token is live.
    Open(Arc<AtomicBool>),
    /// Healed, and still holding the lineages named here, each until its [`Release`] (DI-D-41,
    /// DI-D-46). Healed once it names none. The heal holds the token, so the inventory does not
    /// hand out another.
    Healing { pending: BTreeMap<Key, Release<E>> },
}

/// What releases a healing failure from one lineage (DI-D-41, DI-D-46): the lineage's first seal
/// made after the heal -- numbered `after` or later -- that reaches `through`, the highest epoch
/// the lineage had been pushed when it was healed, finishing with every part successful. The
/// consumer re-issues before it heals (DI-D-12 (c)), so every re-issued write is at or below
/// `through`, and that seal waits for and commits them. `None` when nothing had been pushed.
#[derive(Clone, Copy, Debug)]
pub(super) struct Release<E> {
    after: u64,
    through: Option<E>,
}

impl<E: Ord + Copy> Release<E> {
    /// Whether seal `seq` through `through`, finished successfully, releases the heal.
    pub(super) fn by(&self, seq: u64, through: E) -> bool {
        seq >= self.after && self.through.is_none_or(|reach| through >= reach)
    }
}

struct Failure<E: EpochId + 'static, I: Instant> {
    id: FailureId,
    cause: FailureCause<V<E, I>>,
    suspect: SuspectSet<V<E, I>>,
    observed: I,
    markings: Vec<Marking<V<E, I>>>,
    /// The epochs of its suspect writes, by lineage: the lineages it belongs to.
    epochs: BTreeMap<Key, BTreeSet<E>>,
    standing: Standing<E>,
}

impl<E: EpochId + 'static, I: Instant> Failure<E, I> {
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
    /// This lineage's held writes still waiting for their gates, by epoch and push order: the
    /// edges a cycle search follows (DI-D-43).
    gated: BTreeMap<(E, u64), Gate<E>>,
    /// The operations waiting for one of this lineage's epochs, by that epoch and push order.
    waiting: BTreeSet<(E, u64)>,
    /// This lineage's held writes, ready or not, by epoch: a seal waits for them as it does for
    /// writes in flight (guarantee 5).
    held_by_epoch: BTreeMap<E, usize>,
    /// The highest epoch of any write pushed in this lineage, held or not: where a heal's
    /// [`Release`] must reach.
    pushed_through: Option<E>,
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
            gated: BTreeMap::new(),
            waiting: BTreeSet::new(),
            held_by_epoch: BTreeMap::new(),
            pushed_through: None,
        }
    }

    /// Record seal `through`'s flush of `file` as answered, and as failed if it did: the seal's
    /// sequence number and the file's domains, or what about the answer is inconsistent.
    fn answer(
        &mut self,
        through: E,
        file: IoTargetKey,
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
        entry.ok = !failed_now;
        let domains = Arc::clone(&entry.domains);
        if failed_now {
            *failed = true;
            seal.answered = true;
        }
        Ok((seal.seq, domains))
    }
}

/// An instance's durability state, stamping failures with `K`'s readings.
pub(crate) struct Durability<E: EpochId + 'static, T, I: Instant, K> {
    instance: InstanceId,
    /// Every lineage not yet retired, by key.
    lineages: BTreeMap<Key, LineageState<E>>,
    next_lineage: Key,
    /// The writes not yet covered, keyed by push order.
    writes: BTreeMap<u64, Write<E, T>>,
    /// The unresolved failures, in observation order.
    failures: Vec<Failure<E, I>>,
    next_failure: u64,
    /// The operations held for their gates, by push order.
    held: BTreeMap<u64, HeldOp<E, T>>,
    /// The domains the consumer's provider serves, sorted; empty without one.
    served: Arc<[DomainId]>,
    clock: K,
    /// Where an inconsistency is reported before the panic (DI-D-51), and the seal and flush pairs
    /// (DI-D-80).
    diagnostics: Arc<dyn DiagnosticSink>,
    /// Each part of a seal's flush whose pair is open, kept only while the sink wants the pair, and
    /// removed as the pair ends (DI-D-87).
    flush_pairs: HashMap<(Key, u64, Part), FlushPair<E>>,
    /// The consumer's name for each domain the provider serves, which a flush pair reports.
    served_names: HashMap<DomainId, FlushDomain>,
}

/// An open flush pair: its id, and the seal it is part of.
struct FlushPair<E> {
    id: CorrelationId,
    through: E,
}

/// `part`, as a flush pair reports it, the domain by the name `names` gives it.
fn flush_part(part: Part, names: &HashMap<DomainId, FlushDomain>) -> Option<FlushPart<'_>> {
    match part {
        Part::File(file) => Some(FlushPart::File(file)),
        Part::Domain(domain) => names.get(&domain).map(FlushPart::Domain),
    }
}

/// Report the end of a seal's pair, if it has one open.
fn end_seal<E: fmt::Debug>(
    sink: &dyn DiagnosticSink,
    instance: u64,
    lineage: Key,
    through: &E,
    pair: Option<CorrelationId>,
    outcome: SealOutcome,
) {
    if let Some(id) = pair {
        sink.report(&Diagnostic::SealEnded {
            instance,
            lineage,
            through: format_args!("{through:?}"),
            id,
            outcome,
        });
    }
}

/// Report the end of a flush part's pair.
fn end_flush<E: fmt::Debug>(
    sink: &dyn DiagnosticSink,
    instance: u64,
    lineage: Key,
    pair: &FlushPair<E>,
    part: FlushPart<'_>,
    outcome: FlushOutcome,
) {
    sink.report(&Diagnostic::FlushEnded {
        instance,
        lineage,
        through: format_args!("{:?}", pair.through),
        part,
        id: pair.id,
        outcome,
    });
}

/// One part of a seal's flush, as the core keys it while it is out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Part {
    File(IoTargetKey),
    Domain(DomainId),
}

impl<E: EpochId + 'static, T, I: Instant, K> Durability<E, T, I, K> {
    /// An instance's state, with its default lineage.
    pub(crate) fn new(
        instance: InstanceId,
        clock: K,
        diagnostics: Arc<dyn DiagnosticSink>,
    ) -> Self {
        let mut lineages = BTreeMap::new();
        lineages.insert(DEFAULT_LINEAGE, LineageState::new(None));
        Self {
            instance,
            lineages,
            next_lineage: DEFAULT_LINEAGE + 1,
            writes: BTreeMap::new(),
            failures: Vec::new(),
            next_failure: 0,
            held: BTreeMap::new(),
            served: Arc::from([]),
            clock,
            diagnostics,
            flush_pairs: HashMap::new(),
            served_names: HashMap::new(),
        }
    }

    /// This instance's state is inconsistent: see [`inconsistent`].
    #[cold]
    #[track_caller]
    pub(crate) fn inconsistent(&self, what: &'static str, detail: fmt::Arguments<'_>) {
        inconsistent(self.instance, self.diagnostics.as_ref(), what, detail);
    }
}

/// `instance` found its own state inconsistent -- in a state it cannot be in -- which is a defect in
/// dioring, or the kernel breaking its contract, never the consumer's input (DI-D-46). Reported to
/// `diagnostics` if it wants it (DI-D-51), then panicked on where it was found, in every build,
/// with `detail` naming what was in hand: the bookkeeping is wrong, and nothing reported afterwards
/// could be trusted. On a pool thread the panic aborts the process. While the thread is already
/// unwinding it is reported only, because a second panic would abort and lose the first; the caller
/// then leaves the state as it found it.
#[cold]
#[track_caller]
pub fn inconsistent(
    instance: InstanceId,
    diagnostics: &dyn DiagnosticSink,
    what: &'static str,
    detail: fmt::Arguments<'_>,
) {
    if diagnostics.wants(DiagnosticKind::Inconsistency) {
        diagnostics.report(&Diagnostic::Inconsistency {
            instance: instance.number(),
            what,
            detail,
        });
    }
    if !std::thread::panicking() {
        panic!(
            "dioring instance {instance:?} found its own state inconsistent: {what} ({detail}). \
             This is a defect in dioring, or the kernel broke its contract (DI-D-46)."
        );
    }
}

impl<E: EpochId + 'static, T: Clone, I: Instant, K: StampClock<Instant = I>>
    Durability<E, T, I, K>
{
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
        self.in_flight(write, false);
    }

    /// Record `write` as in flight. One that was held may be at or below a seal made while it was
    /// held; any other is above the seal point, which refused it otherwise (guarantee 6).
    fn in_flight(&mut self, write: Accepted<E, T>, was_held: bool) {
        let Accepted {
            op,
            lineage,
            epoch,
            file,
            target,
            routing,
            domains,
        } = write;
        let Some(state) = self.lineages.get_mut(&lineage).filter(|state| !state.ended) else {
            self.inconsistent(
                "a write was pushed to a lineage that is not live",
                format_args!("write {op:?} to {file:?}, lineage {lineage}, epoch {epoch:?}"),
            );
            return;
        };
        debug_assert!(
            was_held || state.sealed_through.is_none_or(|sealed| epoch > sealed),
            "a write at or below the seal"
        );
        *state.in_flight_by_epoch.entry(epoch).or_insert(0) += 1;
        state.pushed_through = state.pushed_through.max(Some(epoch));
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
                stage: Stage::InFlight,
            },
        );
    }

    /// A write completed. One that failed is reported on its own completion and is neither named
    /// to a flush nor suspected by a later failure (guarantee 1); a failure observed while it was
    /// in flight keeps it, because suspect sets are frozen, and marks it nullified. A write of an
    /// ended lineage is let go either way.
    pub(crate) fn completed(&mut self, op: OpId, end: WriteEnd) -> Due<E, T, I> {
        let mut due = Due::default();
        let Some(write) = self.writes.get_mut(&op.seq) else {
            self.inconsistent(
                "a write completed that was never recorded as pushed",
                format_args!("write {op:?}, ending {end:?}"),
            );
            return due;
        };
        if write.stage != Stage::InFlight {
            let (file, lineage, epoch, stage) =
                (write.file, write.lineage, write.epoch, write.stage);
            self.inconsistent(
                "a write completed twice",
                format_args!(
                    "write {op:?} to {file:?}, lineage {lineage}, epoch {epoch:?}, already \
                     {stage:?}, ending {end:?}"
                ),
            );
            return due;
        }
        let (lineage, epoch) = (write.lineage, write.epoch);
        let ended = self.lineages.get(&lineage).is_none_or(|state| state.ended);
        match end {
            WriteEnd::Succeeded => {
                write.stage = Stage::Completed;
                if ended {
                    self.writes.remove(&op.seq);
                }
            }
            WriteEnd::Failed(fault) => {
                self.writes.remove(&op.seq);
                self.mark(op, MarkingKind::Nullified { fault }, &mut due);
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
    pub(crate) fn seal(&mut self, lineage: Key, through: E) -> Sealing<E, T, I> {
        let Some(state) = self.lineages.get_mut(&lineage).filter(|state| !state.ended) else {
            self.inconsistent(
                "a seal of a lineage that is not live",
                format_args!("lineage {lineage}, through {through:?}"),
            );
            return Sealing::Submitted(Due::default());
        };
        if state.sealed_through.is_some_and(|sealed| through <= sealed) {
            let answer = self.state(lineage, through).unwrap_or(State::Pending);
            return Sealing::AlreadySealed(answer);
        }
        if let Some(cycle) = self.seal_cycle(lineage, through) {
            return Sealing::Cycle(cycle);
        }
        let state = self
            .lineages
            .get_mut(&lineage)
            .expect("live, checked above");
        state.sealed_through = Some(through);
        let seq = state.next_seal;
        state.next_seal += 1;
        let pair = CorrelationId::for_pair(&*self.diagnostics, DiagnosticKind::Seal);
        if let Some(id) = pair {
            self.diagnostics.report(&Diagnostic::SealBegan {
                instance: self.instance.number(),
                lineage,
                through: format_args!("{through:?}"),
                id,
            });
        }
        state.seals.push_back(Seal {
            seq,
            through,
            stage: SealStage::Waiting,
            answered: false,
            pair,
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
        file: IoTargetKey,
        result: Result<(), IoFault>,
    ) -> Due<E, T, I> {
        let mut due = Due::default();
        let answered = match self.lineages.get_mut(&lineage) {
            Some(state) if !state.ended => state.answer(through, file, result.is_err()),
            _ => return due,
        };
        let (seq, domains) = match answered {
            Ok(answered) => answered,
            Err(what) => {
                self.inconsistent(
                    what,
                    format_args!(
                        "lineage {lineage}, seal through {through:?}, file {file:?}, {result:?}"
                    ),
                );
                return due;
            }
        };
        let outcome = if result.is_ok() {
            FlushOutcome::Succeeded
        } else {
            FlushOutcome::Failed
        };
        self.part_answered(lineage, seq, Part::File(file), outcome);
        match result {
            Ok(()) => self.cover(lineage, seq, &mut due),
            Err(fault) => {
                let cause = FailureCause::Commit {
                    target: file,
                    fault,
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
        cause: FailureCause<V<E, I>>,
        reach: &Reach,
        only: Option<Key>,
    ) -> (FailureId, Due<E, T, I>) {
        let mut due = Due::default();
        let id = self.observe(cause, reach, only, &mut due);
        (id, self.advance(due))
    }

    /// Resolve failures, validated whole and applied together: the tokens of a refused call come
    /// back, and nothing changes.
    pub(crate) fn resolve(
        &mut self,
        items: Vec<(FailureToken, Resolution)>,
    ) -> Result<Due<E, T, I>, ResolveError<V<E, I>>> {
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
                self.inconsistent(
                    "a live token for a failure not in the inventory",
                    format_args!("failure {:?}, {resolution:?}", token.id),
                );
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
                    self.abandon_held(&failure.epochs, &mut due);
                }
                Resolution::Heal => {
                    let lineages = &self.lineages;
                    let pending = self.failures[index]
                        .epochs
                        .keys()
                        .filter_map(|key| {
                            lineages.get(key).filter(|state| !state.ended).map(|state| {
                                let release = Release {
                                    after: state.next_seal,
                                    through: state.pushed_through,
                                };
                                (*key, release)
                            })
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
    pub(crate) fn failures(&self) -> Vec<FailureInfo<V<E, I>>> {
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
        cause: FailureCause<V<E, I>>,
        reach: &Reach,
        only: Option<Key>,
        due: &mut Due<E, T, I>,
    ) -> FailureId {
        let instance = self.instance;
        let lineages = &self.lineages;
        let suspect: Vec<SuspectWrite<V<E, I>>> = self
            .writes
            .values()
            .filter(|write| only.is_none_or(|lineage| write.lineage == lineage))
            .filter(|write| lineages.get(&write.lineage).is_some_and(|s| !s.ended))
            .filter(|write| reach.reaches(&write.domains))
            .map(|write| SuspectWrite {
                op: write.op,
                target: write.file,
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
    fn mark(&mut self, write: OpId, kind: MarkingKind, due: &mut Due<E, T, I>) {
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
                kind: kind.clone(),
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
    fn release_healed(&mut self, due: &mut Due<E, T, I>) {
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
    fn advance(&mut self, mut due: Due<E, T, I>) -> Due<E, T, I> {
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
    fn advance_mark(&mut self, lineage: Key, due: &mut Due<E, T, I>) {
        while let Some(seal) = self.lineages.get(&lineage).and_then(|s| s.seals.front()) {
            if !matches!(seal.stage, SealStage::Done { .. })
                || self.blocking(lineage, seal.through).is_some()
            {
                break;
            }
            let through = seal.through;
            let state = self.lineages.get_mut(&lineage).expect("found above");
            let popped = state.seals.pop_front().expect("found above");
            state.durable_through = Some(through);
            end_seal(
                &*self.diagnostics,
                self.instance.number(),
                lineage,
                &through,
                popped.pair,
                SealOutcome::Durable,
            );
            // Only writes whose flush failed are left at or below a finished seal; once the mark
            // passes them, no later failure can reach back to them (guarantee 3).
            self.writes
                .retain(|_, write| write.lineage != lineage || write.epoch > through);
            due.events.push(Event::Durable { lineage, through });
            self.release_through(lineage, through, due);
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
                let pair = state.seals[index].pair.take();
                end_seal(
                    &*self.diagnostics,
                    self.instance.number(),
                    lineage,
                    &through,
                    pair,
                    SealOutcome::Blocked,
                );
                due.events.push(Event::Blocked {
                    lineage,
                    through,
                    by,
                });
            }
        }
    }

    fn start_seals(&mut self, lineage: Key, due: &mut Due<E, T, I>) {
        let Some(state) = self.lineages.get_mut(&lineage) else {
            return;
        };
        // A held write is outstanding as one in flight is: the seal waits for it (guarantee 5).
        let in_flight = state.in_flight_by_epoch.keys().next().copied();
        let held = state.held_by_epoch.keys().next().copied();
        let oldest_in_flight = match (in_flight, held) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
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
            let mut parts: Vec<DomainPart> = Vec::new();
            for write in self.writes.values_mut() {
                if write.lineage != lineage
                    || write.stage != Stage::Completed
                    || write.epoch > through
                {
                    continue;
                }
                write.stage = Stage::Named(seq);
                if write.routing.provider {
                    providers::name(
                        &mut parts,
                        &self.served,
                        write.file,
                        &write.domains,
                        write.op,
                    );
                }
                if write.routing.default && !files.iter().any(|f| f.file == write.file) {
                    files.push(SealFile {
                        file: write.file,
                        domains: Arc::clone(&write.domains),
                        answered: false,
                        ok: false,
                    });
                    due.flushes.push(Flush {
                        lineage,
                        through,
                        file: write.file,
                        target: write.target.clone(),
                    });
                }
            }
            parts.sort_unstable_by_key(|part| part.domain);
            let domains = parts
                .iter()
                .map(|part| SealDomain {
                    domain: part.domain,
                    answered: false,
                    ok: false,
                })
                .collect();
            let issued = files
                .iter()
                .map(|file| Part::File(file.file))
                .chain(parts.iter().map(|part| Part::Domain(part.domain)));
            for part in issued {
                let Some(id) = CorrelationId::for_pair(&*self.diagnostics, DiagnosticKind::Flush)
                else {
                    break;
                };
                let Some(shown) = flush_part(part, &self.served_names) else {
                    continue;
                };
                self.diagnostics.report(&Diagnostic::FlushBegan {
                    instance: self.instance.number(),
                    lineage,
                    through: format_args!("{through:?}"),
                    part: shown,
                    id,
                });
                self.flush_pairs
                    .insert((lineage, seq, part), FlushPair { id, through });
            }
            if !parts.is_empty() {
                due.calls.push(ProviderCall {
                    lineage,
                    seal: seq,
                    domains: parts,
                });
            }
            state.seals[index].stage = SealStage::Flushing {
                files,
                domains,
                failed: false,
            };
        }
    }

    /// Mark every seal of `lineage` whose flushes have all answered as done. One whose flushes all
    /// succeeded releases, in this lineage, every heal it is the [`Release`] of (DI-D-41, DI-D-46);
    /// a failure is healed once that has happened in every lineage it held.
    fn finish_seals(&mut self, lineage: Key) {
        let Some(state) = self.lineages.get_mut(&lineage) else {
            return;
        };
        for seal in &mut state.seals {
            let SealStage::Flushing {
                files,
                domains,
                failed,
            } = &seal.stage
            else {
                continue;
            };
            if files.iter().any(|f| !f.answered) || domains.iter().any(|d| !d.answered) {
                continue;
            }
            let succeeded = !*failed;
            let (seq, through) = (seal.seq, seal.through);
            seal.stage = SealStage::Done { succeeded };
            if succeeded {
                for failure in &mut self.failures {
                    if let Standing::Healing { pending } = &mut failure.standing
                        && pending
                            .get(&lineage)
                            .is_some_and(|release| release.by(seq, through))
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

    /// Part `part` of `lineage`'s seal `seq` answered with `outcome`: end its pair, and, if it
    /// failed, the seal's, whose answer the failure is.
    pub(super) fn part_answered(
        &mut self,
        lineage: Key,
        seq: u64,
        part: Part,
        outcome: FlushOutcome,
    ) {
        if let Some(pair) = self.flush_pairs.remove(&(lineage, seq, part))
            && let Some(shown) = flush_part(part, &self.served_names)
        {
            end_flush(
                &*self.diagnostics,
                self.instance.number(),
                lineage,
                &pair,
                shown,
                outcome,
            );
        }
        if outcome == FlushOutcome::Succeeded {
            return;
        }
        if let Some(seal) = self
            .lineages
            .get_mut(&lineage)
            .and_then(|state| state.seals.iter_mut().find(|seal| seal.seq == seq))
        {
            let pair = seal.pair.take();
            end_seal(
                &*self.diagnostics,
                self.instance.number(),
                lineage,
                &seal.through,
                pair,
                SealOutcome::Failed,
            );
        }
    }
}

impl<E: EpochId + 'static, T, I: Instant, K> Durability<E, T, I, K> {
    /// End every pair of `lineage` still open, or of every lineage: its seals' unanswered, and its
    /// flushes' abandoned. For a lineage that ends, and for the instance's state as it is dropped.
    pub(super) fn end_pairs(&mut self, only: Option<Key>) {
        let instance = self.instance.number();
        let mut flushes: Vec<((Key, u64, Part), FlushPair<E>)> = self
            .flush_pairs
            .extract_if(|(key, ..), _| only.is_none_or(|only| *key == only))
            .collect();
        flushes.sort_unstable_by_key(|((key, seq, _), pair)| (*key, *seq, pair.id));
        for ((lineage, _, part), pair) in &flushes {
            if let Some(shown) = flush_part(*part, &self.served_names) {
                end_flush(
                    &*self.diagnostics,
                    instance,
                    *lineage,
                    pair,
                    shown,
                    FlushOutcome::Abandoned,
                );
            }
        }
        for (&lineage, state) in &mut self.lineages {
            if only.is_some_and(|only| only != lineage) {
                continue;
            }
            for seal in &mut state.seals {
                let pair = seal.pair.take();
                end_seal(
                    &*self.diagnostics,
                    instance,
                    lineage,
                    &seal.through,
                    pair,
                    SealOutcome::Ended,
                );
            }
        }
    }
}

impl<E: EpochId + 'static, T, I: Instant, K> Drop for Durability<E, T, I, K> {
    /// Every pair still open ends as its instance does (DI-D-87): a seal unanswered, a flush
    /// abandoned.
    fn drop(&mut self) {
        self.end_pairs(None);
    }
}
