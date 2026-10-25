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

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::sync::Arc;
use core::fmt;
use core::ops::Bound;
use fixed_hash_table::HeapMap;

use crate::alloc_tags::{tag, tagged};
use crate::capacity::{Capacity, ConstructionError, list, ordered, table, within};
use crate::clock::{Instant, StampClock};
use crate::contract::EpochId;
use crate::correlation::CorrelationId;
use crate::diagnostics::{
    Diagnostic, DiagnosticKind, DiagnosticSink, FlushOutcome, FlushPart, SealOutcome,
};
use crate::domain_lists::{DomainList, DomainLists};
use crate::domains::DomainId;
use crate::ids::{DioringIds, FailureId, FailureToken, InstanceId, Lineage, OpId, TokenSlots};
use crate::types::{Failed, FailureCause, FlushDomain, IoFault, IoTargetKey, Marking, MarkingKind};

mod empties;
mod exchange;
mod failures;
mod files;
mod gates;
mod kept;
mod lineages;
mod lists;
mod providers;
mod scratch;
mod spans;
mod suspects;
mod tables;
mod window;

use crate::ordered::Ordered;
use empties::{Empty, release_heals};
use kept::Kept;
use scratch::Scratch;
use spans::{Span, SpanFile, Spans};
use suspects::suspects;
use tables::{Counts, Seals};
use window::Window;

pub(crate) use exchange::{DomainPart, Due, Event, Flush, ProviderCall, WriteEnd};
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

/// Which files' writes a failure reaches (DI-D-21). Each but `All` also reaches every file
/// declared with no domain, which is unknown and shares fate with every file.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Reach {
    /// Every file.
    All,
    /// Every file whose declared domains intersect a failed file's: the file's own list, held by
    /// the file, which the failure holds in turn (DI-D-108).
    File(DomainList),
    /// Every file declared with one domain: a failed or abandoned provider part.
    Domain(DomainId),
    /// Every file whose declared domains intersect those an import named: a list of the failure's
    /// own, let go when it is resolved.
    Named(DomainList),
}

impl Reach {
    /// The reach of a failed flush of a file declared with `domains`: a file declared with none is
    /// unknown, so it shares fate with every file.
    pub(crate) fn of_file(domains: DomainList) -> Self {
        if domains.is_empty() {
            Reach::All
        } else {
            Reach::File(domains)
        }
    }

    fn reaches(&self, lists: &DomainLists, file: &[DomainId]) -> bool {
        let named = match *self {
            Reach::All => return true,
            Reach::Domain(domain) => return file.is_empty() || file.contains(&domain),
            Reach::File(list) | Reach::Named(list) => lists.get(list),
        };
        file.is_empty() || file.iter().any(|domain| named.contains(domain))
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
    pub(crate) domains: DomainList,
}

/// A write not yet covered successfully, failed, or passed by its lineage's mark.
struct Write<E, T> {
    op: OpId,
    lineage: Key,
    epoch: E,
    file: IoTargetKey,
    target: T,
    routing: Routing,
    domains: DomainList,
    stage: Stage,
    /// The failure clock when it was recorded: no failure observed before names it.
    born: u64,
    /// The failure clock when its lineage ended, if it has: no failure observed since names it.
    until: Option<u64>,
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
    /// Its flushes, and its request to the consumer's provider, are out: `unanswered` counts the
    /// flushes of its files not yet answered, each recorded in its file's span entry. `failed` once
    /// one part failed.
    Flushing {
        unanswered: usize,
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
    /// The latest empty seal made after it and before any later recorded one (DI-D-99).
    trailing: Option<Empty<E>>,
    /// The write whose place the seal holds until the mark passes it (DI-D-110): one it covers.
    pin: Option<u64>,
}

/// Where an unresolved failure stands.
enum Standing<E> {
    /// Open to resolution; whether its token is live is read from its token slot.
    Open,
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

/// An unresolved failure. Its suspect writes are not listed: they are the ones its rule names
/// ([`suspects()`]), read from the write records.
struct Failure<E: EpochId + 'static, I: Instant> {
    id: FailureId,
    cause: FailureCause<V<E, I>>,
    /// The files whose writes it reaches.
    reach: Reach,
    /// The lineage it was confined to, if one was named.
    only: Option<Key>,
    observed: I,
    standing: Standing<E>,
    /// Its slot in the token slots, held until it is resolved.
    token: u32,
    /// The suspect writes as observation listed them, for the census to compare the rule with.
    #[cfg(test)]
    listed: Vec<u64>,
}

impl<E: EpochId + 'static, I: Instant> Failure<E, I> {
    /// Whether this failure names a write recorded at failure clock `born`, nameable until
    /// `until` if it has stopped being, of `lineage`, to a file declared with `domains` in
    /// `lists`: whether observation would have, which is what freezes the set (DI-D-12 (b)).
    fn names(
        &self,
        lists: &DomainLists,
        born: u64,
        until: Option<u64>,
        lineage: Key,
        domains: DomainList,
    ) -> bool {
        let at = self.id.seq;
        born <= at
            && until.is_none_or(|until| until > at)
            && self.only.is_none_or(|only| only == lineage)
            && self.reach.reaches(lists, lists.get(domains))
    }

    /// Whether this failure still holds `lineage`'s epochs among its suspects': open, or healing
    /// and pending on it.
    fn pending(&self, lineage: Key) -> bool {
        match &self.standing {
            Standing::Open => true,
            Standing::Healing { pending } => pending.contains_key(&lineage),
        }
    }
}

/// One lineage's own state.
struct LineageState<E> {
    /// The consumer's description, kept as it was given and lent by the enumeration (DI-D-108).
    description: Option<String>,
    sealed_through: Option<E>,
    durable_through: Option<E>,
    /// The latest empty seal made with no recorded seal queued before it (DI-D-99).
    leading: Option<Empty<E>>,
    next_seal: u64,
    /// Every epoch an abandoned failure contained.
    abandoned: BTreeSet<E>,
    /// Ended (DI-D-30): it answers nothing more, and is retired once its writes in flight complete
    /// and its flushes out have answered (DI-D-98).
    ended: bool,
    /// The highest epoch of any write pushed in this lineage, held or not: where a heal's
    /// [`Release`] must reach.
    pushed_through: Option<E>,
}

impl<E: EpochId> LineageState<E> {
    fn new(description: Option<String>) -> Self {
        Self {
            description,
            sealed_through: None,
            durable_through: None,
            leading: None,
            next_seal: 0,
            abandoned: BTreeSet::new(),
            ended: false,
            pushed_through: None,
        }
    }
}

/// Record `lineage`'s seal `through`'s flush of `file` as answered, and as failed if it did: the
/// seal's sequence number and the file's domains, or what about the answer is inconsistent.
fn answer<E: EpochId>(
    seals: &mut Seals<E>,
    spans: &mut Spans,
    lineage: Key,
    through: E,
    file: IoTargetKey,
    failed_now: bool,
) -> Result<(u64, DomainList), &'static str> {
    let seal = tables::seals_of_mut(seals, lineage)
        .map(|(_, seal)| seal)
        .find(|seal| seal.through == through)
        .ok_or("a flush completed for a seal that does not exist")?;
    let SealStage::Flushing {
        unanswered, failed, ..
    } = &mut seal.stage
    else {
        return Err("a flush completed for a seal not flushing");
    };
    let entry = spans
        .get_mut(&(lineage, file, Span::Seal(seal.seq)))
        .filter(|entry| entry.flushed && !entry.answered)
        .ok_or("a flush completed that the seal did not issue")?;
    entry.answered = true;
    entry.ok = !failed_now;
    *unanswered -= 1;
    let domains = entry.domains;
    if failed_now {
        *failed = true;
        seal.answered = true;
    }
    Ok((seal.seq, domains))
}

/// An instance's durability state, stamping failures with `K`'s readings.
pub(crate) struct Durability<E: EpochId + 'static, T, I: Instant, K> {
    instance: InstanceId,
    /// Every lineage not yet retired, by key.
    lineages: Ordered<Key, LineageState<E>>,
    next_lineage: Key,
    /// The writes not yet covered, keyed by push order.
    writes: Ordered<u64, Write<E, T>>,
    /// Every lineage's recorded seals the mark has not passed.
    seals: Seals<E>,
    /// Each lineage's writes in flight, by epoch.
    in_flight_by_epoch: Counts<E>,
    /// Each lineage's held writes, ready or not, by epoch: a seal waits for them as it does for
    /// writes in flight (guarantee 5).
    held_by_epoch: Counts<E>,
    /// Each lineage's held writes still waiting for their gates, by epoch and push order: the edges
    /// a cycle search follows (DI-D-43).
    gated: Ordered<(Key, E, u64), Gate<E>>,
    /// What each (lineage, file, seal span) holding an uncovered write is charged (DI-D-98).
    spans: Spans,
    /// The places of the submit window each operation holds, and who holds them (DI-D-100).
    window: Window,
    /// The unresolved failures, in observation order.
    failures: Vec<Failure<E, I>>,
    /// Writes no longer uncovered that an unresolved failure still suspects, with their markings.
    kept: Kept<E, I>,
    /// Whether each unresolved failure's token is live (DI-D-108), shared with the tokens.
    tokens: Arc<TokenSlots>,
    /// The token slots no unresolved failure holds.
    free_tokens: Vec<u32>,
    /// The flush-domain lists targets and imports by domains name (DI-D-108).
    lists: DomainLists,
    /// The lists `DomainLists::shared` handed a test that pushes writes without targets.
    #[cfg(test)]
    test_lists: Vec<DomainList>,
    /// The failure clock: the number the next failure observed takes. A write record reads it when
    /// it is recorded and when it stops being nameable, which is what a failure's rule compares.
    next_failure: u64,
    /// The operations held for their gates, by push order.
    held: Ordered<u64, HeldOp<E, T>>,
    /// The domains the consumer's provider serves, sorted; empty without one.
    served: Arc<[DomainId]>,
    clock: K,
    /// Where an inconsistency is reported before the panic (DI-D-51), and the seal and flush pairs
    /// (DI-D-80).
    diagnostics: Arc<dyn DiagnosticSink>,
    /// Each part of a seal's flush whose pair is open, kept only while the sink wants the pair, and
    /// removed as the pair ends (DI-D-87).
    flush_pairs: HeapMap<(Key, u64, Part), FlushPair<E>>,
    /// The consumer's name for each domain the provider serves, which a flush pair reports.
    served_names: HeapMap<DomainId, FlushDomain>,
    /// The lists a change makes due into, while they are not lent out ([`scratch`]).
    spare: Due<E, T, I>,
    /// Whether they are lent out.
    due_out: bool,
    /// What a call collects into, reused by every call.
    scratch: Scratch<E>,
    /// What the consumer declared, which sized them.
    capacity: Capacity,
}

/// An open flush pair: its id, and the seal it is part of.
struct FlushPair<E> {
    id: CorrelationId,
    through: E,
}

/// `part`, as a flush pair reports it, the domain by the name `names` gives it.
fn flush_part(part: Part, names: &HeapMap<DomainId, FlushDomain>) -> Option<FlushPart<'_>> {
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
        tagged(tag::DURABILITY_END_SEAL_REPORT, || {
            sink.report(&Diagnostic::SealEnded {
                instance,
                lineage,
                through: format_args!("{through:?}"),
                id,
                outcome,
            });
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
    tagged(tag::DURABILITY_END_FLUSH_REPORT, || {
        sink.report(&Diagnostic::FlushEnded {
            instance,
            lineage,
            through: format_args!("{:?}", pair.through),
            part,
            id: pair.id,
            outcome,
        });
    });
}

/// One part of a seal's flush, as the core keys it while it is out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Part {
    File(IoTargetKey),
    Domain(DomainId),
}

impl<E: EpochId + 'static, T, I: Instant, K> Durability<E, T, I, K> {
    /// An instance's state, with its default lineage, its tables made at the sizes `capacity`
    /// gives them (DI-D-107) for a provider that serves `served` domains.
    ///
    /// # Errors
    ///
    /// [`ConstructionError::Table`] if a table's size is more than a table can be, or cannot be
    /// allocated.
    pub(crate) fn new(
        instance: InstanceId,
        clock: K,
        diagnostics: Arc<dyn DiagnosticSink>,
        capacity: &Capacity,
        served: usize,
    ) -> Result<Self, ConstructionError> {
        let operations = Some(capacity.operations);
        let mut lineages = tagged(tag::DURABILITY_NEW_LINEAGES, || {
            ordered("the lineages", Some(capacity.lineages))
        })?;
        if lineages
            .insert(DEFAULT_LINEAGE, LineageState::new(None))
            .is_err()
        {
            return Err(ConstructionError::NoLineages);
        }
        let writes = tagged(tag::DURABILITY_NEW_WRITES, || {
            ordered("the uncovered writes", operations)
        })?;
        let seals = tagged(tag::DURABILITY_NEW_SEALS, || {
            ordered("the recorded seals", operations)
        })?;
        let in_flight_by_epoch = tagged(tag::DURABILITY_NEW_IN_FLIGHT_BY_EPOCH, || {
            ordered("the writes in flight by epoch", operations)
        })?;
        let held_by_epoch = tagged(tag::DURABILITY_NEW_HELD_BY_EPOCH, || {
            ordered("the held writes by epoch", operations)
        })?;
        let gated = tagged(tag::DURABILITY_NEW_GATED, || {
            ordered("the held writes by gate", operations)
        })?;
        let held = tagged(tag::DURABILITY_NEW_HELD, || {
            ordered("the operations held for their gates", operations)
        })?;
        let spans = tagged(tag::DURABILITY_NEW_SPANS, || {
            table("the span entries", Some(capacity.operations))
        })?;
        let window = tagged(tag::DURABILITY_NEW_WINDOW, || {
            Window::new(capacity.operations)
        })?;
        let flush_pairs = tagged(tag::DURABILITY_NEW_FLUSH_PAIRS, || {
            table("the open flush pairs", capacity.flush_parts(served))
        })?;
        let served_names = tagged(tag::DURABILITY_NEW_SERVED_NAMES, || {
            table("the served domains' names", Some(served))
        })?;
        let failures = tagged(tag::DURABILITY_NEW_FAILURES, || {
            list("the unresolved failures", capacity.failures(served))
        })?;
        let kept = tagged(tag::DURABILITY_NEW_KEPT, || Kept::new(capacity.operations))?;
        let tokens = tagged(tag::DURABILITY_NEW_TOKENS, || {
            TokenSlots::new(capacity.failures(served)).map(Arc::new)
        })?;
        let mut free_tokens = tagged(tag::DURABILITY_NEW_FREE_TOKENS, || {
            list("the free token slots", Some(tokens.len()))
        })?;
        free_tokens.extend(
            (0..tokens.len())
                .rev()
                .filter_map(|at| u32::try_from(at).ok()),
        );
        let lists = tagged(tag::DURABILITY_NEW_LISTS, || {
            DomainLists::new(
                capacity.targets.checked_add(capacity.imports),
                capacity.domains,
            )
        })?;
        let spare = tagged(tag::DURABILITY_NEW_DUE, || Due::reserved(capacity))?;
        let scratch = Scratch::new(capacity, served)?;
        Ok(Self {
            instance,
            lineages,
            next_lineage: DEFAULT_LINEAGE + 1,
            writes,
            seals,
            in_flight_by_epoch,
            held_by_epoch,
            gated,
            spans,
            window,
            failures,
            kept,
            tokens,
            free_tokens,
            lists,
            #[cfg(test)]
            test_lists: Vec::new(),
            next_failure: 0,
            held,
            served: tagged(tag::DURABILITY_NEW_SERVED, || Arc::from([])),
            clock,
            diagnostics,
            flush_pairs,
            served_names,
            spare,
            due_out: false,
            scratch,
            capacity: *capacity,
        })
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
    if tagged(tag::DURABILITY_INCONSISTENT_WANTS, || {
        diagnostics.wants(DiagnosticKind::Inconsistency)
    }) {
        tagged(tag::DURABILITY_INCONSISTENT_REPORT, || {
            diagnostics.report(&Diagnostic::Inconsistency {
                instance: instance.number(),
                what,
                detail,
            });
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
            .seal_points(&self.seals, lineage)
            .find(|seal| seal.through >= epoch)
            .is_some_and(|seal| seal.finished);
        Some(match self.blocking(lineage, epoch) {
            Some(by) if finished => State::Blocked(by),
            _ => State::Pending,
        })
    }

    /// A write was accepted by the ring.
    pub(crate) fn pushed(&mut self, write: Accepted<E, T>) {
        self.in_flight(write, false);
        #[cfg(test)]
        assert!(
            self.spans_agree(),
            "the span entries disagree with the uncovered writes"
        );
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
        state.pushed_through = state.pushed_through.max(Some(epoch));
        if tables::count_up(&mut self.in_flight_by_epoch, lineage, epoch).is_err() {
            self.inconsistent(
                "an epoch in flight was counted past the operations declared",
                format_args!("write {op:?} to {file:?}, lineage {lineage}, epoch {epoch:?}"),
            );
            return;
        }
        match self
            .spans
            .entry((lineage, file, Span::Unsealed))
            .or_insert_with(|| SpanFile::new(domains))
        {
            Ok(entry) => entry.writes += 1,
            Err(_) => {
                self.inconsistent(
                    "a span entry was made past the operations declared",
                    format_args!("write {op:?} to {file:?}, lineage {lineage}"),
                );
                return;
            }
        }
        self.hold_uncovered(op);
        let write = Write {
            op,
            lineage,
            epoch,
            file,
            target,
            routing,
            domains,
            stage: Stage::InFlight,
            born: self.next_failure,
            until: None,
        };
        if self.writes.insert(op.seq, write).is_err() {
            self.inconsistent(
                "an uncovered write was recorded past the operations declared",
                format_args!("write {op:?} to {file:?}, lineage {lineage}, epoch {epoch:?}"),
            );
        }
    }

    /// A write completed. One that failed is reported on its own completion and is neither named
    /// to a flush nor suspected by a later failure (guarantee 1); a failure observed while it was
    /// in flight keeps it, because suspect sets are frozen, and marks it nullified. A write of an
    /// ended lineage is let go either way.
    pub(crate) fn completed(&mut self, op: OpId, end: WriteEnd) -> Due<E, T, I> {
        let mut due = self.fresh();
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
                    self.let_go_one(op.seq);
                }
            }
            WriteEnd::Failed(fault) => {
                self.let_go_one(op.seq);
                self.mark(op, MarkingKind::Nullified { fault }, &mut due);
            }
        }
        tables::count_down(&mut self.in_flight_by_epoch, lineage, epoch);
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
        let pin = self.pinned_by(lineage, through);
        let state = self
            .lineages
            .get_mut(&lineage)
            .expect("live, checked above");
        state.sealed_through = Some(through);
        let seq = state.next_seal;
        state.next_seal += 1;
        let pair = CorrelationId::for_pair(&*self.diagnostics, DiagnosticKind::Seal);
        if let Some(id) = pair {
            tagged(tag::DURABILITY_SEAL_REPORT, || {
                self.diagnostics.report(&Diagnostic::SealBegan {
                    instance: self.instance.number(),
                    lineage,
                    through: format_args!("{through:?}"),
                    id,
                });
            });
        }
        let Some(op) = pin else {
            self.record_empty(lineage, Empty::new(seq, through, pair));
            let due = self.fresh();
            return Sealing::Submitted(self.advance(due));
        };
        let recorded = self.seals.insert(
            (lineage, seq),
            Seal {
                seq,
                through,
                stage: SealStage::Waiting,
                answered: false,
                pair,
                trailing: None,
                pin: Some(op),
            },
        );
        if recorded.is_err() {
            self.inconsistent(
                "a seal was recorded past the operations declared",
                format_args!("lineage {lineage}, through {through:?}"),
            );
        } else {
            self.hold_for_seal(op);
        }
        let due = self.fresh();
        Sealing::Submitted(self.advance(due))
    }

    /// `lineage`'s seal `through`'s flush of `file` completed. Success covers the writes it was
    /// issued for; a failure is observed at once, and is the seal's answer. A flush of an ended
    /// lineage reports nothing (DI-D-30), and lets go the writes it was issued for (DI-D-98).
    pub(crate) fn flushed(
        &mut self,
        lineage: Key,
        through: E,
        file: IoTargetKey,
        result: Result<(), IoFault>,
    ) -> Due<E, T, I> {
        let mut due = self.fresh();
        let answered = match self.lineages.get_mut(&lineage) {
            Some(state) if !state.ended => answer(
                &mut self.seals,
                &mut self.spans,
                lineage,
                through,
                file,
                result.is_err(),
            ),
            Some(_) => {
                let answered = answer(
                    &mut self.seals,
                    &mut self.spans,
                    lineage,
                    through,
                    file,
                    result.is_err(),
                );
                if let Ok((seq, _)) = answered {
                    self.let_go(|write| {
                        write.lineage == lineage
                            && write.file == file
                            && write.stage == Stage::Named(seq)
                    });
                    let window = &mut self.window;
                    let mut lost = None;
                    self.seals.retain(|key, seal| {
                        let keep = key.0 != lineage
                            || matches!(seal.stage, SealStage::Flushing { unanswered, .. } if unanswered > 0);
                        if !keep && let Err(what) = window::unpin(window, seal.pin) {
                            lost.get_or_insert(what);
                        }
                        keep
                    });
                    if let Some(what) = lost {
                        self.inconsistent(
                            what,
                            format_args!("lineage {lineage}, through {through:?}"),
                        );
                    }
                    self.retire_if_drained(lineage);
                }
                return self.advance(due);
            }
            None => return due,
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
                self.observe(cause, &Reach::of_file(domains), None, &mut due);
            }
        }
        self.advance(due)
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
        // What observation named before the rule replaced the list, for the census (DI-D-109).
        #[cfg(test)]
        let listed = {
            let lineages = &self.lineages;
            self.writes
                .values()
                .filter(|write| only.is_none_or(|lineage| write.lineage == lineage))
                .filter(|write| lineages.get(&write.lineage).is_some_and(|s| !s.ended))
                .filter(|write| reach.reaches(&self.lists, self.lists.get(write.domains)))
                .map(|write| write.op.seq)
                .collect()
        };
        let id = FailureId {
            instance: self.instance,
            seq: self.next_failure,
        };
        self.next_failure += 1;
        let minted = self.free_tokens.pop().and_then(|slot| {
            FailureToken::mint(id, &self.tokens, slot, true).map(|token| (slot, token))
        });
        let Some((slot, token)) = minted else {
            self.inconsistent(
                "no token slot was free for a failure within the bound",
                format_args!("failure {id:?}"),
            );
            return id;
        };
        let observed = tagged(tag::DURABILITY_OBSERVE_CLOCK, || self.clock.now());
        let recorded_cause = tagged(tag::DURABILITY_OBSERVE_CAUSE, || cause.clone());
        let failure = Failure {
            id,
            cause: recorded_cause,
            reach: *reach,
            only,
            observed,
            standing: Standing::Open,
            token: slot,
            #[cfg(test)]
            listed,
        };
        if within(&mut self.failures, failure).is_err() {
            self.free_tokens.push(slot);
            self.release_reach(*reach);
            self.inconsistent(
                "more failures were unresolved than the flush parts and the imports allow",
                format_args!("failure {id:?}"),
            );
            return id;
        }
        let at = self.failures.len() - 1;
        self.hold_suspects(at);
        let suspect = self.suspect_set(&self.failures[at], tag::DURABILITY_OBSERVE_SUSPECT);
        tagged(tag::DURABILITY_OBSERVE_EVENT, || {
            due.events.push(Event::Failed(Failed {
                id,
                token,
                cause,
                suspect,
                observed,
            }));
        });
        id
    }

    /// Mark `write`, which has just stopped being uncovered, if an unresolved failure suspects it
    /// (DI-D-36): one reading of the clock, kept once with the write's record, and a `Marked` for
    /// each failure suspecting it, in observation order.
    fn mark(&mut self, write: OpId, kind: MarkingKind, due: &mut Due<E, T, I>) {
        if !self
            .failures
            .iter()
            .any(|failure| self.suspects(failure, write))
        {
            return;
        }
        let observed = tagged(tag::DURABILITY_MARK_CLOCK, || self.clock.now());
        if let Err(what) = self.kept.mark(write, observed, kind.clone()) {
            self.inconsistent(what, format_args!("write {write:?}, {kind:?}"));
        }
        for failure in self
            .failures
            .iter()
            .filter(|failure| self.suspects(failure, write))
        {
            let marking = Marking {
                write,
                observed,
                kind: kind.clone(),
            };
            tagged(tag::DURABILITY_MARK_EVENT, || {
                due.events.push(Event::Marked {
                    failure: failure.id,
                    marking,
                });
            });
        }
    }

    /// Every marking `failure` has gained, in the order observed: its suspect writes', read from
    /// their records (DI-D-108).
    fn markings<'a>(
        &'a self,
        failure: &'a Failure<E, I>,
    ) -> impl Iterator<Item = Marking<V<E, I>>> + 'a {
        self.kept
            .markings(move |write| self.suspects(failure, write))
    }

    /// Resolve as healed every healing failure that holds no lineage any more, each reported by its
    /// `Healed` entry (DI-D-41).
    fn release_healed(&mut self, due: &mut Due<E, T, I>) {
        let mut at = 0;
        while at < self.failures.len() {
            if matches!(&self.failures[at].standing, Standing::Healing { pending } if pending.is_empty())
            {
                let failure = self.failures.remove(at);
                self.free_tokens.push(failure.token);
                let markings = tagged(tag::DURABILITY_RELEASE_HEALED_MARKINGS, || {
                    self.markings(&failure).collect()
                });
                let suspect = self.suspect_set(&failure, tag::DURABILITY_RELEASE_HEALED_SUSPECT);
                tagged(tag::DURABILITY_RELEASE_HEALED_EVENT, || {
                    due.events.push(Event::Healed {
                        failure: failure.id,
                        suspect,
                        markings,
                    });
                });
                self.release_suspects(&failure);
                self.release_reach(failure.reach);
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
            .find(|failure| self.holds_within(failure, lineage, (lower, Bound::Included(through))))
            .map(|failure| failure.id)
    }

    /// For every live lineage: start every seal whose writes have all completed, and finish every
    /// seal whose flushes have all answered. Then report the heals that took effect, advance each
    /// lineage's high-water mark, and answer the seals a failure holds.
    fn advance(&mut self, mut due: Due<E, T, I>) -> Due<E, T, I> {
        let mut live = core::mem::take(&mut self.scratch.live);
        let mut lost = None;
        for (&key, state) in self.lineages.iter() {
            if !state.ended && within(&mut live, key).is_err() {
                lost.get_or_insert(key);
            }
        }
        if let Some(key) = lost {
            self.inconsistent(
                "more live lineages than the lineages declared",
                format_args!("lineage {key}"),
            );
        }
        for &lineage in &live {
            self.start_seals(lineage, &mut due);
            self.finish_empties(lineage);
            self.finish_seals(lineage);
        }
        self.release_healed(&mut due);
        for &lineage in &live {
            self.advance_mark(lineage, &mut due);
        }
        live.clear();
        self.scratch.live = live;
        #[cfg(test)]
        assert!(
            self.spans_agree(),
            "the span entries disagree with the uncovered writes"
        );
        #[cfg(test)]
        assert!(
            self.lists_agree(),
            "something names a flush-domain list whose row is free"
        );
        #[cfg(test)]
        assert!(
            self.window_agrees(),
            "the window's places disagree with the uncovered writes, the failures and the seals, \
             or a failure's rule with what observation listed"
        );
        due
    }

    fn start_seals(&mut self, lineage: Key, due: &mut Due<E, T, I>) {
        // A held write is outstanding as one in flight is: the seal waits for it (guarantee 5).
        let oldest_in_flight = self.oldest_outstanding(lineage);
        let mut lost = None;
        let mut full = None;
        let mut pairs_full = false;
        let mut unlisted = false;
        for (_, seal) in tables::seals_of_mut(&mut self.seals, lineage) {
            if !matches!(seal.stage, SealStage::Waiting) {
                continue;
            }
            let through = seal.through;
            if oldest_in_flight.is_some_and(|oldest| oldest <= through) {
                // Seals are in ascending order, so no later one is ready either.
                break;
            }
            let seq = seal.seq;
            // This seal's flushes are the ones pushed from here on, one per file it names.
            let first_flush = due.flushes.len();
            let mut unanswered = 0;
            let mut parts: Vec<DomainPart> = Vec::new();
            for write in self.writes.values_mut() {
                if write.lineage != lineage
                    || write.stage != Stage::Completed
                    || write.epoch > through
                {
                    continue;
                }
                // The write moves from its lineage's unsealed span to the seal's.
                if !spans::leave(&mut self.spans, (lineage, write.file, Span::Unsealed)) {
                    lost.get_or_insert(write.op);
                }
                write.stage = Stage::Named(seq);
                if write.routing.provider {
                    providers::name(
                        &mut parts,
                        &self.served,
                        write.file,
                        self.lists.get(write.domains),
                        write.op,
                    );
                }
                let domains = write.domains;
                let Ok(entry) = self
                    .spans
                    .entry((lineage, write.file, Span::Seal(seq)))
                    .or_insert_with(|| SpanFile::new(domains))
                else {
                    full.get_or_insert(write.op);
                    continue;
                };
                entry.writes += 1;
                if write.routing.default && !entry.flushed {
                    entry.flushed = true;
                    unanswered += 1;
                    let flush = Flush {
                        lineage,
                        through,
                        file: write.file,
                        target: write.target.clone(),
                    };
                    if within(&mut due.flushes, flush).is_err() {
                        unlisted = true;
                    }
                }
            }
            parts.sort_unstable_by_key(|part| part.domain);
            let domains = tagged(tag::DURABILITY_START_SEALS_DOMAINS, || {
                parts
                    .iter()
                    .map(|part| SealDomain {
                        domain: part.domain,
                        answered: false,
                        ok: false,
                    })
                    .collect()
            });
            let issued = due.flushes[first_flush..]
                .iter()
                .map(|flush| Part::File(flush.file))
                .chain(parts.iter().map(|part| Part::Domain(part.domain)));
            for part in issued {
                let Some(id) = CorrelationId::for_pair(&*self.diagnostics, DiagnosticKind::Flush)
                else {
                    break;
                };
                let Some(shown) = flush_part(part, &self.served_names) else {
                    continue;
                };
                tagged(tag::DURABILITY_START_SEALS_REPORT, || {
                    self.diagnostics.report(&Diagnostic::FlushBegan {
                        instance: self.instance.number(),
                        lineage,
                        through: format_args!("{through:?}"),
                        part: shown,
                        id,
                    });
                });
                if self
                    .flush_pairs
                    .insert((lineage, seq, part), FlushPair { id, through })
                    .is_err()
                {
                    pairs_full = true;
                }
            }
            if !parts.is_empty() {
                let call = ProviderCall {
                    lineage,
                    seal: seq,
                    domains: parts,
                };
                if within(&mut due.calls, call).is_err() {
                    unlisted = true;
                }
            }
            seal.stage = SealStage::Flushing {
                unanswered,
                domains,
                failed: false,
            };
        }
        if let Some(op) = lost {
            self.inconsistent(
                "a write named to a seal was in no unsealed span",
                format_args!("write {op:?}, lineage {lineage}"),
            );
        }
        if let Some(op) = full {
            self.inconsistent(
                "a span entry was made past the operations declared",
                format_args!("write {op:?}, lineage {lineage}"),
            );
        }
        if pairs_full {
            self.inconsistent(
                "a flush pair was opened past what the operations declared allow",
                format_args!("lineage {lineage}"),
            );
        }
        if unlisted {
            self.inconsistent(
                "a change made more flushes or provider requests due than the operations declared",
                format_args!("lineage {lineage}"),
            );
        }
    }

    /// Mark every seal of `lineage` whose flushes have all answered as done. One whose flushes all
    /// succeeded releases, in this lineage, every heal it is the [`Release`] of (DI-D-41, DI-D-46);
    /// a failure is healed once that has happened in every lineage it held.
    fn finish_seals(&mut self, lineage: Key) {
        for (_, seal) in tables::seals_of_mut(&mut self.seals, lineage) {
            let SealStage::Flushing {
                unanswered,
                domains,
                failed,
            } = &seal.stage
            else {
                continue;
            };
            if *unanswered > 0 || domains.iter().any(|d| !d.answered) {
                continue;
            }
            let succeeded = !*failed;
            let (seq, through) = (seal.seq, seal.through);
            seal.stage = SealStage::Done { succeeded };
            if succeeded {
                release_heals(&mut self.failures, lineage, seq, through);
            }
        }
    }

    /// Retire an ended lineage whose writes in flight have all completed, and whose flushes out
    /// have all answered.
    fn retire_if_drained(&mut self, lineage: Key) {
        let in_flight = tables::lowest(&self.in_flight_by_epoch, lineage).is_some();
        let sealing = tables::seals_of(&self.seals, lineage).next().is_some();
        if self
            .lineages
            .get(&lineage)
            .is_some_and(|state| state.ended && !in_flight && !sealing)
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
        if let Some(seal) = self.seals.get_mut(&(lineage, seq)) {
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
        // Made for as many pairs as the table holds, so every pair taken fits.
        let mut flushes = core::mem::take(&mut self.scratch.pairs);
        let mut unlisted = false;
        self.flush_pairs.retain(|key, pair| {
            if only.is_some_and(|only| key.0 != only) {
                return true;
            }
            let taken = FlushPair {
                id: pair.id,
                through: pair.through,
            };
            let fits = within(&mut flushes, (*key, taken)).is_ok();
            unlisted |= !fits;
            !fits
        });
        if unlisted {
            self.inconsistent(
                "more flush pairs were open than the flush parts declared",
                format_args!("lineage {only:?}"),
            );
        }
        flushes.sort_unstable_by_key(|((key, seq, _), pair)| (*key, *seq, pair.id));
        for ((lineage, _, part), pair) in flushes.drain(..) {
            if let Some(shown) = flush_part(part, &self.served_names) {
                end_flush(
                    &*self.diagnostics,
                    instance,
                    lineage,
                    &pair,
                    shown,
                    FlushOutcome::Abandoned,
                );
            }
        }
        self.scratch.pairs = flushes;
        for (&lineage, state) in self.lineages.iter_mut() {
            if only.is_some_and(|only| only != lineage) {
                continue;
            }
            for (through, pair) in state.take_pairs(&mut self.seals, lineage) {
                end_seal(
                    &*self.diagnostics,
                    instance,
                    lineage,
                    &through,
                    Some(pair),
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
