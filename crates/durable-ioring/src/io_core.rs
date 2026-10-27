// Copyright (c) 2026 Mike Grier
//! [`DurableIoCore`]: dioring as a portable state machine (DI-D-49, DI-D-50 P11, DI-D-52).
//!
//! The core holds no backend and does no I/O. It decides, and says what to do: a consumer
//! operation is checked, then either held for its gate or pushed, as a [`Submission`], to the
//! backend the caller lends it, and recorded only once the push accepts it; everything else returns
//! [`CoreActions`], saying whether entries became poppable, and leaves what the driver is to do in
//! the core's own queues, made at construction (DI-23.12.2): submissions the core originated, taken
//! one at a time by [`DurableIoCore::next_submission`], and requests for the consumer's provider,
//! drained by [`DurableIoCore::drain_requests`]. The caller -- the driver -- pushes those
//! submissions to its backend, submits, hands the requests to its provider, and feeds every
//! completion and every provider answer back. Entries are popped from the core's own queue, in the
//! order they happened.
//!
//! **Refusals** (DI-D-52, DI-D-54). A consumer operation the backend refuses is undone: the push
//! hands the submission back, nothing was recorded, no operation id was spent, and the buffer and
//! context go back to the consumer with the backend's
//! [`RefusalCause`](crate::backend::RefusalCause). One the core will hold for its gate is put to
//! the backend's [`check`](IoBackend::check) first, after every check of the core's own, so it is
//! refused for what an ungated one would be. A submission the core originated -- a commit, or an
//! operation released by its gate -- is recorded when emitted, and the driver hands it back
//! through [`DurableIoCore::refused`] if the backend refuses it: kept, and emitted again with the
//! next actions or at once by [`DurableIoCore::retry`], when the cause is retryable; otherwise
//! ended, as a completion with a [`REFUSED`](crate::backend::REFUSED) fault -- an operation
//! completing as `Failed`, a write nullified as one its target failed is, and a commit failing its
//! seal's flush of its target.
//!
//! **Gates** (DI-D-23, DI-D-43). An operation gated on an epoch not yet durable is held, spending
//! an operation id, and is emitted in the actions of the change that makes the gate durable; one
//! whose gate, or own epoch, is abandoned first completes as `NeverIssued` instead.
//!
//! **The provider** (DI-D-27, DI-D-50 P6). A core built with [`DurableIoCore::with_provider`]
//! routes the domains it names to the consumer's provider: a seal's request comes out through
//! [`DurableIoCore::drain_requests`], and each domain's answer goes back through
//! [`DurableIoCore::provided`]. Making the request, and with what lock released, is the driver's
//! (DI-D-44).
//!
//! The durability rules themselves -- seals, coverage, failures and their suspect sets -- are the
//! durability core's, shared with the Windows driver while both paths exist.

use alloc::collections::VecDeque;
use alloc::sync::Arc;

use fixed_hash_table::HeapMap;

use crate::alloc_tags::{tag, tagged};
use crate::backend::{
    Completion, IoBackend, Refused, Submission, SubmissionData, SubmissionOp, SubmissionToken,
    refusal_fault,
};
use crate::capacity::{
    AtCapacity, Capacity, ConstructionError, Limit, list, ordered, queue, table, within,
    within_queue,
};
use crate::clock::StampClock;
use crate::contract::{EpochId, Identities};
use crate::diagnostics::DiagnosticSink;
use crate::domain_lists::DomainList;
use crate::domains::{DomainId, epoch_state, intern, routing};
use crate::durability::{
    Accepted, Arrival, DEFAULT_LINEAGE, Due, Durability, Event, Gate, GateState, Key, Routing,
    Sealing, WriteEnd,
};
use crate::ids::{DioringIds, InstanceId, Lineage, OpId};
use crate::io_target::IoTargetDescription;
use crate::observer::FailureObserver;
use crate::ordered::Ordered;
use crate::provider::ProviderRequest;
use crate::types::{
    AddTargetCause, AddTargetError, DurabilityRequest, Entry, Epoch, EpochState, FlushDomain,
    HeldOperation, Hold, IoTargetBusy, IoTargetKey, Leftover, OpCompletion, OpKind, Outcome,
    PushError, PushRefusal, ReadOptions, RemoveTargetError, SealError, UnknownLineage,
    WriteOptions,
};

mod failures;
mod lineages;
mod provider;
mod queue;

#[cfg(test)]
mod tests;

/// The identities a core over clock `K` uses.
type V<E, K> = DioringIds<E, <K as StampClock>::Instant>;

/// What a seal answers: the request's answer and what it made due, or why it was refused.
type SealAnswer<E, K> = Result<(DurabilityRequest<V<E, K>>, CoreActions), SealError<V<E, K>>>;

/// An epoch's state, or that its lineage is not this core's live one.
type StateAnswer<E, K> = Result<EpochState<V<E, K>>, UnknownLineage<V<E, K>>>;

/// What the driver does after a call into the core: take every submission the core originated
/// with [`DurableIoCore::next_submission`], pushing each to the backend in the order taken, then
/// submit; drain the provider's requests with [`DurableIoCore::drain_requests`]; and make a
/// delivery if `deliver` says. A submission the backend refuses goes back through
/// [`DurableIoCore::refused`], whose own submissions join the ones still to take.
///
/// The submissions and requests stay in the core, in queues made when it was built, so a call
/// allocates nothing to say what is to be done (DI-23.12.2). The submissions are at most those in
/// flight, which are at most the operations declared; the requests are one for each seal started,
/// and are at most the operations declared while the driver drains them after every call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[must_use = "the submissions must be taken and pushed, the requests drained, and a delivery made when asked"]
pub struct CoreActions {
    /// Entries became poppable where there were none: the driver's readiness should be raised.
    pub deliver: bool,
}

impl CoreActions {
    /// Actions with no delivery to make.
    const NONE: Self = Self { deliver: false };
}

/// What a call left the driver to do, taken out of the core's queues into lists of its own: the
/// shape [`CoreActions`] had before DI-23.12.2, for the tests that look at what a call made due.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct Taken<V: Identities, B> {
    pub(crate) submit: Vec<Submission<B>>,
    pub(crate) requests: Vec<ProviderRequest<V>>,
    pub(crate) deliver: bool,
}

#[cfg(test)]
impl<V: Identities, B> Taken<V, B> {
    /// Nothing to do.
    pub(crate) fn is_empty(&self) -> bool {
        self.submit.is_empty() && self.requests.is_empty() && !self.deliver
    }
}

#[cfg(test)]
impl<E: EpochId + 'static, B, C, K: StampClock> DurableIoCore<E, B, C, K> {
    /// Take what `actions` left in the core: every submission to push, and every request.
    pub(crate) fn taken(&mut self, actions: CoreActions) -> Taken<V<E, K>, B> {
        let mut requests = Vec::new();
        self.drain_requests(|request| requests.push(request));
        Taken {
            submit: core::iter::from_fn(|| self.next_submission()).collect(),
            requests,
            deliver: actions.deliver,
        }
    }
}

/// What the core knows of a submission in flight.
enum InFlight<E: EpochId + 'static, I: crate::clock::Instant, C> {
    /// A consumer operation, with what its completion entry carries. `released` when the core
    /// emitted it after its gate, rather than the consumer's push issuing it.
    Op {
        id: OpId,
        kind: OpKind<DioringIds<E, I>>,
        target: IoTargetKey,
        context: C,
        released: bool,
    },
    /// A seal's commit of one target.
    Commit {
        lineage: Key,
        through: E,
        target: IoTargetKey,
    },
}

/// An operation held for its gate (DI-D-23): what its submission will be once released.
struct HeldOp<E: EpochId + 'static, I: crate::clock::Instant, B, C> {
    id: OpId,
    kind: OpKind<DioringIds<E, I>>,
    target: IoTargetKey,
    op: SubmissionOp<B>,
    context: C,
    /// The epoch it waits for, as the consumer named it.
    gate: Epoch<DioringIds<E, I>>,
}

/// A target as the core decides with it.
struct TargetRecord {
    description: IoTargetDescription,
    /// Its flush domains, interned, sorted, without repeats, in a list of the durability core's
    /// (DI-D-108), named by each write's record and let go when the target is removed.
    domains: DomainList,
    /// Who answers for its writes (DI-D-27): the built-in default, the provider, or both.
    routing: Routing,
}

/// dioring's core: every lineage's durability, the targets it decides for, the operations in
/// flight or held for their gates, and the queue of entries for the consumer (DI-D-49). `E` is the
/// epoch-id type, `B` the backend's owned buffer, `C` the consumer's per-operation context, and `K`
/// the clock failures are stamped with.
pub struct DurableIoCore<E: EpochId + 'static, B, C, K: StampClock> {
    instance: InstanceId,
    /// What the consumer declared when the core was built, and what is refused past (DI-D-107).
    capacity: Capacity,
    durability: Durability<E, IoTargetKey, K::Instant, K>,
    targets: HeapMap<IoTargetKey, TargetRecord>,
    /// Every flush domain any target or the provider was declared with, interned.
    domains: HeapMap<FlushDomain, DomainId>,
    /// The domains the consumer's provider serves, sorted; empty without one.
    served: Arc<[DomainId]>,
    /// Each served domain's name, at its id's position in `served`.
    served_names: Vec<FlushDomain>,
    /// Every submission handed out and not yet completed.
    in_flight: HeapMap<SubmissionToken, InFlight<E, K::Instant, C>>,
    /// The operations held for their gates, by push order.
    held: Ordered<u64, HeldOp<E, K::Instant, B, C>>,
    /// Consumer operations in flight, by target, as `remove_target` reports them; a target with
    /// none has no entry.
    per_target: HeapMap<IoTargetKey, usize>,
    /// The completion queue, made for [`Capacity::entries`] (DI-D-112).
    queue: VecDeque<Entry<V<E, K>, B, C>>,
    /// Submissions the core originated that the backend refused for a retryable cause, emitted
    /// again with the next actions.
    unpushed: Vec<Submission<B>>,
    /// Submissions the core originated, for the driver to take and push, in order.
    outgoing: VecDeque<Submission<B>>,
    /// Requests for the consumer's provider, in the order made, for the driver to drain.
    requests: VecDeque<ProviderRequest<V<E, K>>>,
    next_op: u64,
    next_token: u64,
}

impl<E: EpochId + 'static, B, C, K: StampClock> DurableIoCore<E, B, C, K> {
    /// A core with no targets, no provider, and only its default lineage, stamping failures with
    /// `clock` and reporting to `diagnostics`, and refusing past `capacity` (DI-D-107).
    ///
    /// Every table the core keys is made now, at the size `capacity` gives it, and never grows.
    ///
    /// # Errors
    ///
    /// [`ConstructionError::NoLineages`] if `capacity` declares no lineage, the default being one,
    /// and [`ConstructionError::Table`] if a table's size is more than a table can be, or cannot be
    /// allocated.
    pub fn new(
        clock: K,
        diagnostics: Arc<dyn DiagnosticSink>,
        capacity: Capacity,
    ) -> Result<Self, ConstructionError> {
        Self::build(clock, diagnostics, capacity, 0)
    }

    /// A core for a provider serving `served` domains, with the tables made at the sizes `capacity`
    /// and `served` give them.
    pub(super) fn build(
        clock: K,
        diagnostics: Arc<dyn DiagnosticSink>,
        capacity: Capacity,
        served: usize,
    ) -> Result<Self, ConstructionError> {
        if capacity.lineages == 0 {
            return Err(ConstructionError::NoLineages);
        }
        let instance = InstanceId::next();
        let durability = Durability::new(instance, clock, diagnostics, &capacity, served)?;
        let targets = tagged(tag::IO_CORE_NEW_TARGETS, || {
            table("the targets", Some(capacity.targets))
        })?;
        let domains = tagged(tag::IO_CORE_NEW_DOMAINS, || {
            table("the flush domains", Some(capacity.domains))
        })?;
        // An operation in flight holds a place, and a commit in flight belongs to an entry with a
        // completed write in it, which holds another: so the two together never exceed the places,
        // which never exceed the operations declared (DI-D-107).
        let in_flight = tagged(tag::IO_CORE_NEW_IN_FLIGHT, || {
            table("the submissions in flight", Some(capacity.operations))
        })?;
        let per_target = tagged(tag::IO_CORE_NEW_PER_TARGET, || {
            table("the operations in flight by target", Some(capacity.targets))
        })?;
        let held = tagged(tag::IO_CORE_NEW_HELD, || {
            ordered(
                "the operations held for their gates",
                Some(capacity.operations),
            )
        })?;
        // Every submission the core originated is in flight from when it is emitted, and those are
        // at most the operations declared, whether waiting to be taken or refused and kept.
        let operations = Some(capacity.operations);
        let unpushed = tagged(tag::IO_CORE_NEW_UNPUSHED, || {
            list("the submissions refused and kept", operations)
        })?;
        let outgoing = tagged(tag::IO_CORE_NEW_OUTGOING, || {
            queue("the submissions to push", operations)
        })?;
        // One for each seal started, and recorded seals are at most the operations (DI-D-110).
        let requests = tagged(tag::IO_CORE_NEW_REQUESTS, || {
            queue("the provider's requests", operations)
        })?;
        let completions = tagged(tag::IO_CORE_NEW_QUEUE, || {
            queue("the completion queue", capacity.entries(served))
        })?;
        Ok(Self {
            instance,
            capacity,
            durability,
            targets,
            domains,
            served: tagged(tag::IO_CORE_NEW_SERVED, || Arc::from([])),
            served_names: Vec::new(),
            in_flight,
            held,
            per_target,
            queue: completions,
            unpushed,
            outgoing,
            requests,
            next_op: 0,
            next_token: 0,
        })
    }

    /// Give the core the consumer's failure observer (DI-D-113), as it is built: chained onto
    /// the constructor, before any operation is pushed. A core given one after it has pushed an
    /// operation or observed a failure, or given a second, is an inconsistency (DI-D-46): the
    /// observer would have missed what came before.
    #[must_use]
    pub fn observed_by(mut self, observer: Arc<dyn FailureObserver<V<E, K>>>) -> Self {
        if self.next_op > 0 || self.durability.has_observed() {
            self.durability.inconsistent(
                "a failure observer was given to a core already in use",
                format_args!("instance {:?}", self.instance),
            );
        } else if !self.durability.observed_by(observer) {
            self.durability.inconsistent(
                "a failure observer was given twice",
                format_args!("instance {:?}", self.instance),
            );
        }
        self
    }

    /// The default lineage, which every core has and which never ends.
    #[must_use]
    pub fn default_lineage(&self) -> Lineage {
        self.name(DEFAULT_LINEAGE)
    }

    /// Decide for a target under `key`, as `description` states it. The driver attaches the target
    /// itself to the backend under the same key.
    ///
    /// # Errors
    ///
    /// [`AddTargetError`], handing the description back, with [`AddTargetCause::KeyInUse`] if `key`
    /// is in use, and with [`AddTargetCause::AtCapacity`] if the core holds as many targets as it
    /// was built for, or the description's domains, those it has not been given before, would take
    /// it past the domains it was built for (DI-D-107). A refusal changes nothing.
    pub fn add_target(
        &mut self,
        key: IoTargetKey,
        description: IoTargetDescription,
    ) -> Result<(), AddTargetError<IoTargetDescription>> {
        let refuse = |description, cause| {
            Err(AddTargetError {
                key,
                target: description,
                cause,
            })
        };
        if self.targets.contains_key(&key) {
            return refuse(description, AddTargetCause::KeyInUse);
        }
        let declared = &description.flush_domains;
        let fresh = declared
            .iter()
            .enumerate()
            .filter(|(at, domain)| {
                !self.domains.contains_key(*domain) && !declared[..*at].contains(domain)
            })
            .count();
        let limit = if self.targets.len() >= self.capacity.targets {
            Some((Limit::Targets, self.capacity.targets))
        } else if self.domains.len() + fresh > self.capacity.domains {
            Some((Limit::Domains, self.capacity.domains))
        } else {
            None
        };
        if let Some((limit, declared)) = limit {
            return refuse(
                description,
                AddTargetCause::AtCapacity(AtCapacity { limit, declared }),
            );
        }
        let table = &mut self.domains;
        let mut lost = false;
        let claimed =
            self.durability
                .lists_mut()
                .claim(description.flush_domains.iter().filter_map(|name| {
                    let id = intern(table, name);
                    lost |= id.is_none();
                    id
                }));
        let domains = match claimed {
            Ok(list) if !lost => Ok(list),
            Ok(list) => {
                self.durability.lists_mut().release(list);
                Err("the flush domains outnumbered the table made for them")
            }
            Err(what) => Err(what),
        };
        let domains = match domains {
            Ok(domains) => domains,
            Err(what) => {
                // The domains were checked to fit above, and a target holds one list, so this
                // does not happen.
                self.durability
                    .inconsistent(what, format_args!("target {key:?}"));
                return refuse(
                    description,
                    AddTargetCause::AtCapacity(AtCapacity {
                        limit: Limit::Domains,
                        declared: self.capacity.domains,
                    }),
                );
            }
        };
        let routing = routing(self.durability.lists().get(domains), &self.served);
        let record = TargetRecord {
            description,
            domains,
            routing,
        };
        if let Err(full) = self.targets.insert(key, record) {
            // The count was checked above, so this does not happen.
            self.durability.inconsistent(
                "the targets outnumbered the table made for them",
                format_args!("target {key:?}"),
            );
            return refuse(
                full.value.description,
                AddTargetCause::AtCapacity(AtCapacity {
                    limit: Limit::Targets,
                    declared: self.capacity.targets,
                }),
            );
        }
        Ok(())
    }

    /// The description of the target under `key`, if there is one.
    #[must_use]
    pub fn target(&self, key: IoTargetKey) -> Option<&IoTargetDescription> {
        self.targets.get(&key).map(|record| &record.description)
    }

    /// Stop deciding for the target under `key`, handing its description back, unless something
    /// still holds it (DI-D-45). The driver detaches the target from the backend once this
    /// succeeds.
    ///
    /// # Errors
    ///
    /// [`RemoveTargetError::UnknownKey`] for no such target, and [`RemoveTargetError::Busy`]
    /// naming every hold at once: operations in flight, operations held for a gate, uncovered
    /// writes, and the failures suspecting writes to it.
    pub fn remove_target(
        &mut self,
        key: IoTargetKey,
    ) -> Result<IoTargetDescription, RemoveTargetError> {
        if !self.targets.contains_key(&key) {
            return Err(RemoveTargetError::UnknownKey(key));
        }
        let in_flight = self.per_target.get(&key).copied().unwrap_or(0);
        let held_for_gate = self.held.values().filter(|held| held.target == key).count();
        let holds = self.durability.file_holds(key);
        if in_flight == 0 && held_for_gate == 0 && holds.is_empty() {
            let record = self.targets.remove(&key).expect("present, checked above");
            self.durability.lists_mut().release(record.domains);
            return Ok(record.description);
        }
        Err(RemoveTargetError::Busy(IoTargetBusy {
            target: key,
            in_flight,
            held_for_gate,
            uncovered: holds.uncovered,
            failures: holds.failures,
        }))
    }

    /// What holds the target under `key` that an [`IoTargetBusy`] counts without naming, given to
    /// `visit` (DI-D-103): for each lineage gated on, in lineage order, the lowest gate its
    /// operations to the target are held for; for each lineage with an uncovered write to it, the
    /// highest epoch among them; and each failure suspecting a write to it, until its `Abandoned`
    /// or `Healed` entry is popped (DI-D-108), in the order observed. Nothing for a key with no target, or a target nothing holds.
    pub fn target_holds(&self, key: IoTargetKey, mut visit: impl FnMut(Hold<V<E, K>>)) {
        if !self.targets.contains_key(&key) {
            return;
        }
        self.durability.each_lineage(|lineage| {
            let lowest = self
                .held
                .values()
                .filter(|held| held.target == key && held.gate.lineage.seq == lineage)
                .map(|held| held.gate)
                .min_by_key(|gate| gate.id);
            if let Some(gate) = lowest {
                visit(Hold::Gate(gate));
            }
        });
        self.durability
            .file_uncovered(key, |through| visit(Hold::Uncovered(through)));
        self.durability
            .file_failures(key, |failure| visit(Hold::Failure(failure)));
    }

    /// Write `data` to `target` at `offset`, tagged `epoch`: checked, then held for its gate, or
    /// pushed to `backend` as a submission and recorded once the push accepts it. The caller
    /// submits.
    ///
    /// # Errors
    ///
    /// [`PushError`], handing back the buffer (`None` for a registered region) and `context`:
    /// for an unknown target; an epoch whose lineage is not this core's live one, at or below its
    /// seal point, or abandoned; a gate in another core, of a lineage not live, abandoned, or
    /// closing a cycle (DI-D-43) -- checked in that order -- or `PushRefusal::Backend` with the
    /// cause the backend gave: its `check`'s for a write the core would hold, its `push`'s for one
    /// pushed now.
    #[expect(
        clippy::too_many_arguments,
        reason = "one write's whole description, and the backend that issues it"
    )]
    pub fn write<Bk: IoBackend<Buffer = B>>(
        &mut self,
        target: IoTargetKey,
        offset: u64,
        data: SubmissionData<B>,
        epoch: Epoch<V<E, K>>,
        context: C,
        options: WriteOptions<V<E, K>>,
        backend: &mut Bk,
    ) -> Result<OpId, PushError<V<E, K>, B, C>> {
        let op = SubmissionOp::Write {
            offset,
            data,
            caching: options.caching,
        };
        let kind = OpKind::Write { epoch };
        self.issue(target, kind, op, context, options.gate, backend)
    }

    /// Read from `target` at `offset` into `data`: checked, then held for its gate, or pushed to
    /// `backend` and recorded once the push accepts it. The caller submits.
    ///
    /// # Errors
    ///
    /// [`PushError`], handing back the buffer and `context`: for an unknown target; a gate in
    /// another core, of a lineage not live, or abandoned; or `PushRefusal::Backend` with the cause
    /// the backend gave, as for [`write`](Self::write).
    pub fn read<Bk: IoBackend<Buffer = B>>(
        &mut self,
        target: IoTargetKey,
        offset: u64,
        data: SubmissionData<B>,
        context: C,
        options: ReadOptions<V<E, K>>,
        backend: &mut Bk,
    ) -> Result<OpId, PushError<V<E, K>, B, C>> {
        let op = SubmissionOp::Read { offset, data };
        self.issue(target, OpKind::Read, op, context, options.gate, backend)
    }

    /// Seal every epoch of `through`'s lineage at or below it (DI-D-9): the answer, and what it
    /// made due at once.
    ///
    /// # Errors
    ///
    /// [`SealError::UnknownLineage`] for a lineage that is not this core's live one, and
    /// [`SealError::GateCycle`] for a seal that would close a gate cycle (DI-D-43).
    pub fn make_durable_through(&mut self, through: Epoch<V<E, K>>) -> SealAnswer<E, K> {
        if !self.is_live(through.lineage) {
            return Err(SealError::UnknownLineage(through.lineage));
        }
        let was_empty = self.queue.is_empty();
        match self.durability.seal(through.lineage.seq, through.id) {
            Sealing::AlreadySealed(state) => Ok((
                DurabilityRequest::AlreadySealed(epoch_state(state)),
                self.act(Due::default(), was_empty),
            )),
            Sealing::Submitted(due) => Ok((DurabilityRequest::Submitted, self.act(due, was_empty))),
            Sealing::Cycle(cycle) => Err(SealError::GateCycle {
                cycle: self.epochs(cycle),
            }),
        }
    }

    /// Record a completion the backend reported: a consumer operation's entry goes on the queue
    /// before anything it makes due, so a `Durable` it leads to follows it (guarantee 5).
    ///
    /// A completion for no submission in flight -- one never handed out, or completed twice -- is
    /// the backend breaking its contract, and is panicked on (DI-D-46, DI-D-50 P9).
    pub fn complete(&mut self, completion: Completion<B>) -> CoreActions {
        let was_empty = self.queue.is_empty();
        let Completion {
            token,
            result,
            buffer,
        } = completion;
        let Some(flight) = self.in_flight.remove(&token) else {
            self.durability.inconsistent(
                "the backend completed a submission the core has no record of",
                format_args!("token {token:?}, result {result:?}"),
            );
            return self.act(Due::default(), was_empty);
        };
        let due = match flight {
            InFlight::Op {
                id,
                kind,
                target,
                context,
                ..
            } => {
                self.count_down(target, id);
                let write = matches!(kind, OpKind::Write { .. });
                let end = match &result {
                    Ok(_) => WriteEnd::Succeeded,
                    Err(fault) => WriteEnd::Failed(fault.clone()),
                };
                let outcome = match result {
                    Ok(transferred) => Outcome::Transferred(transferred),
                    Err(fault) => Outcome::Failed(fault),
                };
                self.push_queued(Entry::Op(OpCompletion {
                    id,
                    kind,
                    outcome,
                    buffer,
                    context,
                }));
                if write {
                    self.durability.completed(id, end)
                } else {
                    Due::default()
                }
            }
            InFlight::Commit {
                lineage,
                through,
                target,
            } => self
                .durability
                .flushed(lineage, through, target, result.map(|_| ())),
        };
        self.act(due, was_empty)
    }

    /// Take back a submission the core originated that the backend refused (DI-D-54). For a
    /// [retryable](crate::backend::RefusalCause::is_retryable) cause it is kept, and emitted again
    /// with the next actions or by [`retry`](Self::retry); nothing is due. For any other it can
    /// never be issued, so it ends here, as a completion with a [`REFUSED`](crate::backend::REFUSED)
    /// fault would end it: an operation completes as `Failed`, a write nullified as one its target
    /// failed is, and a commit fails its seal's flush of its target. What that makes due is
    /// returned.
    ///
    /// A consumer operation the consumer's push issued never comes back this way -- its push is
    /// refused to the consumer -- so one that does, or a submission never handed out, is panicked
    /// on (DI-D-46).
    pub fn refused(&mut self, refused: Refused<B>) -> CoreActions {
        let Refused { cause, submission } = refused;
        let token = submission.token;
        let ours = matches!(
            self.in_flight.get(&token),
            Some(InFlight::Commit { .. } | InFlight::Op { released: true, .. })
        );
        if !ours {
            self.durability.inconsistent(
                "a refusal handed back that is not one of the core's own submissions",
                format_args!("token {token:?}, cause {cause}"),
            );
            return CoreActions::NONE;
        }
        if cause.is_retryable() {
            if within(&mut self.unpushed, submission).is_err() {
                self.durability.inconsistent(
                    "more submissions were kept than are in flight",
                    format_args!("token {token:?}, cause {cause}"),
                );
            }
            return CoreActions::NONE;
        }
        self.complete(Completion {
            token,
            result: Err(refusal_fault(cause)),
            buffer: owned_op(submission.op),
        })
    }

    /// The submissions refused before, to push again -- once the driver has submitted or drained,
    /// as their causes said -- put with the ones to take.
    pub fn retry(&mut self) -> CoreActions {
        self.emit_unpushed();
        CoreActions::NONE
    }

    /// The next submission the core originated, for the driver to push, in the order emitted.
    pub fn next_submission(&mut self) -> Option<Submission<B>> {
        self.outgoing.pop_front()
    }

    /// Hand each request for the consumer's provider to `visit`, in the order made, emptying the
    /// queue, whose storage is kept (DI-D-103).
    pub fn drain_requests(&mut self, visit: impl FnMut(ProviderRequest<V<E, K>>)) {
        self.requests.drain(..).for_each(visit);
    }

    /// Put the submissions refused before with the ones to take, ahead of anything emitted after.
    fn emit_unpushed(&mut self) {
        for submission in self.unpushed.drain(..) {
            if let Err(submission) = within_queue(&mut self.outgoing, submission) {
                self.durability.inconsistent(
                    "more submissions were to push than are in flight",
                    format_args!("token {:?}", submission.token),
                );
            }
        }
    }

    /// Put `submission`, which the core originated, with the ones for the driver to take.
    fn emit(&mut self, submission: Submission<B>) {
        if let Err(submission) = within_queue(&mut self.outgoing, submission) {
            self.durability.inconsistent(
                "more submissions were to push than are in flight",
                format_args!("token {:?}", submission.token),
            );
        }
    }

    /// How many places of the submit window are held: each operation accepted and not yet popped,
    /// each write still uncovered, each write an unresolved failure suspects, and the write each
    /// seal the mark has not passed holds, counted once however many of those hold it (DI-D-100,
    /// DI-D-110).
    #[must_use]
    pub fn window_places(&self) -> usize {
        self.durability.window_places()
    }

    /// Whether the places the core's tables account for are exactly those the window holds.
    #[cfg(test)]
    fn check_window(&self) {
        let mut operations: alloc::collections::BTreeSet<u64> = alloc::collections::BTreeSet::new();
        for flight in self.in_flight.values() {
            if let InFlight::Op { id, .. } = flight {
                operations.insert(id.seq);
            }
        }
        operations.extend(self.held.keys().copied());
        for queued in &self.queue {
            if let Entry::Op(done) = queued {
                operations.insert(done.id.seq);
            }
        }
        assert_eq!(
            operations,
            self.durability.operation_holders(),
            "the window's places disagree with the operations in flight, held and queued"
        );
    }

    /// The lineage's high-water mark: the highest epoch reported durable, or `None` if none is.
    ///
    /// # Errors
    ///
    /// [`UnknownLineage`] for a lineage that is not this core's live one.
    pub fn durable_through(&self, lineage: Lineage) -> Result<Option<E>, UnknownLineage<V<E, K>>> {
        self.ours(lineage)?;
        self.durability
            .durable_through(lineage.seq)
            .ok_or(UnknownLineage(lineage))
    }

    /// The lineage's seal point: the highest epoch sealed, or `None` if none is.
    ///
    /// # Errors
    ///
    /// [`UnknownLineage`] for a lineage that is not this core's live one.
    pub fn sealed_through(&self, lineage: Lineage) -> Result<Option<E>, UnknownLineage<V<E, K>>> {
        self.ours(lineage)?;
        self.durability
            .sealed_through(lineage.seq)
            .ok_or(UnknownLineage(lineage))
    }

    /// An epoch's state.
    ///
    /// # Errors
    ///
    /// [`UnknownLineage`] for a lineage that is not this core's live one.
    pub fn epoch_state(&self, epoch: Epoch<V<E, K>>) -> StateAnswer<E, K> {
        self.ours(epoch.lineage)?;
        self.durability
            .state(epoch.lineage.seq, epoch.id)
            .map(epoch_state)
            .ok_or(UnknownLineage(epoch.lineage))
    }

    /// End the core, giving `visit` what it still holds (DI-D-103): the operations held for their
    /// gates, which were never issued, by value in push order; then the unresolved failures, in the
    /// order observed; then every live lineage with its final high-water mark, lent for the visit.
    /// What is in flight is the backend's to run down.
    pub fn close(self, mut visit: impl FnMut(Leftover<'_, V<E, K>, B, C>)) {
        let Self {
            held, durability, ..
        } = self;
        for held in held.into_values() {
            visit(Leftover::Held(HeldOperation {
                id: held.id,
                kind: held.kind,
                gate: held.gate,
                buffer: owned_op(held.op),
                context: held.context,
            }));
        }
        durability.failure_infos(|failure| visit(Leftover::Failure(failure)));
        durability.lineages(|lineage| visit(Leftover::Lineage(lineage)));
    }

    /// A lineage key's `Lineage` name.
    fn name(&self, seq: Key) -> Lineage {
        Lineage {
            instance: self.instance,
            seq,
        }
    }

    /// A cycle in the core's terms, as the contract reports it.
    fn epochs(&self, cycle: Vec<Gate<E>>) -> Vec<Epoch<V<E, K>>> {
        tagged(tag::IO_CORE_EPOCHS, || {
            cycle
                .into_iter()
                .map(|gate| Epoch::new(self.name(gate.lineage), gate.epoch))
                .collect()
        })
    }

    /// Refuse another core's lineage, which this one has no answer for.
    fn ours(&self, lineage: Lineage) -> Result<(), UnknownLineage<V<E, K>>> {
        if lineage.instance == self.instance {
            Ok(())
        } else {
            Err(UnknownLineage(lineage))
        }
    }

    /// Whether `lineage` is this core's, and live.
    fn is_live(&self, lineage: Lineage) -> bool {
        lineage.instance == self.instance && self.durability.is_live(lineage.seq)
    }

    /// Refuse, hold, or push one operation: the path writes and reads share. One that will be held
    /// is put to the backend's `check` once the core's own checks pass, so it is refused for what a
    /// push now would be refused for whatever the queues hold (DI-D-54).
    fn issue<Bk: IoBackend<Buffer = B>>(
        &mut self,
        target: IoTargetKey,
        kind: OpKind<V<E, K>>,
        op: SubmissionOp<B>,
        context: C,
        gate: Option<Epoch<V<E, K>>>,
        backend: &mut Bk,
    ) -> Result<OpId, PushError<V<E, K>, B, C>> {
        let epoch = match kind {
            OpKind::Write { epoch } => Some(epoch),
            OpKind::Read => None,
        };
        let waiting = match self.decide(target, epoch, gate) {
            Ok(waiting) => waiting,
            Err(reason) => {
                return Err(PushError {
                    reason,
                    buffer: owned_op(op),
                    context,
                });
            }
        };
        if self.durability.window_places() >= self.capacity.operations {
            return Err(PushError {
                reason: PushRefusal::AtCapacity(AtCapacity {
                    limit: Limit::Operations,
                    declared: self.capacity.operations,
                }),
                buffer: owned_op(op),
                context,
            });
        }
        if let (Some(at), Some(named)) = (waiting, gate) {
            if let Err(cause) = tagged(tag::IO_CORE_ISSUE_BACKEND_CHECK, || {
                backend.check(target, &op)
            }) {
                return Err(PushError {
                    reason: PushRefusal::Backend(cause),
                    buffer: owned_op(op),
                    context,
                });
            }
            let id = self.op_id();
            self.durability.admit(id);
            let accepted = epoch.map(|epoch| self.accepted(id, target, epoch));
            self.durability.hold(id, accepted, at);
            let held = HeldOp {
                id,
                kind,
                target,
                op,
                context,
                gate: named,
            };
            if self.held.insert(id.seq, held).is_err() {
                // The window refused the push before it got here if there was no room for it.
                self.durability.inconsistent(
                    "an operation was held past the operations declared",
                    format_args!("operation {id:?}, gated on {named:?}"),
                );
            }
            #[cfg(test)]
            self.check_window();
            return Ok(id);
        }
        let token = self.token();
        let pushed = tagged(tag::IO_CORE_ISSUE_BACKEND_PUSH, || {
            backend.push(Submission { token, target, op })
        });
        if let Err(refused) = pushed {
            return Err(handed_back(refused, context));
        }
        let id = self.op_id();
        self.durability.admit(id);
        if let Some(epoch) = epoch {
            let accepted = self.accepted(id, target, epoch);
            self.durability.pushed(accepted);
        }
        self.issued(token, id, kind, target, context, false);
        #[cfg(test)]
        self.check_window();
        Ok(id)
    }

    /// Why an operation on `target` -- tagged `epoch` if it is a write, gated on `gate` if it has
    /// one -- must be refused, if it must: the target, then the epoch's lineage, then its seal
    /// (guarantee 6), then whether it was abandoned (DI-D-35), then the gate. Otherwise whether it
    /// waits: the gate's epoch in the core's terms, or `None` to push it now. Mutable for the cycle
    /// search's scratch, which is the durability core's; nothing the core decides with changes.
    fn decide(
        &mut self,
        target: IoTargetKey,
        epoch: Option<Epoch<V<E, K>>>,
        gate: Option<Epoch<V<E, K>>>,
    ) -> Result<Option<Gate<E>>, PushRefusal<V<E, K>>> {
        if !self.targets.contains_key(&target) {
            return Err(PushRefusal::UnknownFile(target));
        }
        if let Some(epoch) = epoch {
            if !self.is_live(epoch.lineage) {
                return Err(PushRefusal::UnknownLineage(epoch.lineage));
            }
            if let Some(sealed_through) = self.durability.refuses(epoch.lineage.seq, epoch.id) {
                return Err(PushRefusal::Sealed {
                    epoch,
                    sealed_through,
                });
            }
            if self.durability.is_abandoned(epoch.lineage.seq, epoch.id) {
                return Err(PushRefusal::EpochAbandoned { epoch });
            }
        }
        let Some(gate) = gate else {
            return Ok(None);
        };
        if gate.lineage.instance != self.instance {
            return Err(PushRefusal::UnknownLineage(gate.lineage));
        }
        let at = Gate {
            lineage: gate.lineage.seq,
            epoch: gate.id,
        };
        match self.durability.gate_state(at) {
            GateState::Durable => Ok(None),
            GateState::Waiting => {
                let write = epoch.map(|epoch| Gate {
                    lineage: epoch.lineage.seq,
                    epoch: epoch.id,
                });
                match write.and_then(|write| self.durability.write_cycle(write, at)) {
                    None => Ok(Some(at)),
                    Some(cycle) => Err(PushRefusal::GateCycle {
                        cycle: self.epochs(cycle),
                    }),
                }
            }
            GateState::Abandoned => Err(PushRefusal::GateAbandoned { gate }),
            GateState::Unknown => Err(PushRefusal::UnknownLineage(gate.lineage)),
        }
    }

    /// A write to `target` as the durability core records it.
    fn accepted(
        &self,
        id: OpId,
        target: IoTargetKey,
        epoch: Epoch<V<E, K>>,
    ) -> Accepted<E, IoTargetKey> {
        let record = &self.targets[&target];
        Accepted {
            op: id,
            lineage: epoch.lineage.seq,
            epoch: epoch.id,
            file: target,
            target,
            routing: record.routing,
            domains: record.domains,
        }
    }

    /// A token no submission in flight has.
    fn token(&mut self) -> SubmissionToken {
        let token = SubmissionToken(self.next_token);
        self.next_token += 1;
        token
    }

    /// The next operation id. Only an accepted push spends one -- held, or taken by the backend --
    /// so accepted pushes' ids are consecutive.
    fn op_id(&mut self) -> OpId {
        let id = OpId {
            instance: self.instance,
            seq: self.next_op,
        };
        self.next_op += 1;
        id
    }

    /// Record a consumer operation now in flight.
    fn issued(
        &mut self,
        token: SubmissionToken,
        id: OpId,
        kind: OpKind<V<E, K>>,
        target: IoTargetKey,
        context: C,
        released: bool,
    ) {
        let recorded = self.in_flight.insert(
            token,
            InFlight::Op {
                id,
                kind,
                target,
                context,
                released,
            },
        );
        if recorded.is_err() {
            self.durability.inconsistent(
                "an operation was put in flight past the submissions the operations declared allow",
                format_args!("operation {id:?} on {target:?}"),
            );
        }
        match self.per_target.entry(target).or_insert(0) {
            Ok(count) => *count += 1,
            Err(_) => self.durability.inconsistent(
                "an operation was put in flight for more targets than were declared",
                format_args!("operation {id:?} on {target:?}"),
            ),
        }
    }

    /// A consumer operation on `target` completed.
    fn count_down(&mut self, target: IoTargetKey, id: OpId) {
        match self.per_target.get_mut(&target) {
            Some(1) => {
                self.per_target.remove(&target);
            }
            Some(count) => *count -= 1,
            None => self.durability.inconsistent(
                "a completion for a target with nothing in flight",
                format_args!("operation {id:?} on {target:?}"),
            ),
        }
    }

    /// Act on what a change made due: append its entries in order; emit, after any refused before,
    /// every operation its gate released and a commit for every flush now due; and make a request
    /// of the provider for every seal that needs one. `was_empty` is whether the queue was empty
    /// when the change began.
    fn act(&mut self, mut due: Due<E, IoTargetKey, K::Instant>, was_empty: bool) -> CoreActions {
        for event in due.events.drain(..) {
            match event.into_entry(self.instance) {
                Arrival::Entry(entry) => self.enqueue(entry),
                Arrival::Other(Event::NeverIssued { op, abandoned }) => {
                    let abandoned = Epoch::new(self.name(abandoned.lineage), abandoned.epoch);
                    self.never_issued(op, abandoned);
                }
                Arrival::Other(Event::Superseded { lineage, through }) => {
                    self.withdraw_blocked(Epoch::new(self.name(lineage), through));
                }
                Arrival::Other(other) => self
                    .durability
                    .inconsistent("an entry the queue cannot take", format_args!("{other:?}")),
            }
        }
        self.emit_unpushed();
        for id in due.released.drain(..) {
            self.release(id);
        }
        for flush in due.flushes.drain(..) {
            let token = self.token();
            let recorded = self.in_flight.insert(
                token,
                InFlight::Commit {
                    lineage: flush.lineage,
                    through: flush.through,
                    target: flush.file,
                },
            );
            if recorded.is_err() {
                self.durability.inconsistent(
                    "a commit was put in flight past the submissions the operations declared allow",
                    format_args!("commit of {:?}", flush.file),
                );
            }
            self.emit(Submission {
                token,
                target: flush.file,
                op: SubmissionOp::Commit,
            });
        }
        for call in due.calls.drain(..) {
            let request = self.request(call);
            if within_queue(&mut self.requests, request).is_err() {
                self.durability.inconsistent(
                    "more provider requests were made than seals can be recorded",
                    format_args!("instance {:?}", self.instance),
                );
            }
        }
        self.durability.recycle(due);
        #[cfg(test)]
        {
            self.check_window();
            self.check_queue();
            assert!(
                self.durability.due_home(),
                "a change's lists were not given back"
            );
        }
        CoreActions {
            deliver: was_empty && !self.queue.is_empty(),
        }
    }

    /// A held operation will never be issued: its completion goes on the queue, with its buffer
    /// and context.
    fn never_issued(&mut self, op: OpId, abandoned: Epoch<V<E, K>>) {
        let Some(held) = self.held.remove(&op.seq) else {
            self.durability.inconsistent(
                "an operation never issued that the core does not hold",
                format_args!("operation {op:?}, abandoned {abandoned:?}"),
            );
            return;
        };
        self.push_queued(Entry::Op(OpCompletion {
            id: held.id,
            kind: held.kind,
            outcome: Outcome::NeverIssued { abandoned },
            buffer: owned_op(held.op),
            context: held.context,
        }));
    }

    /// A held operation's gate is durable: emit it, recorded as in flight from now.
    fn release(&mut self, id: OpId) {
        let Some(held) = self.held.remove(&id.seq) else {
            self.durability.inconsistent(
                "a release of an operation the core does not hold",
                format_args!("operation {id:?}"),
            );
            return;
        };
        let token = self.token();
        self.durability.issued(id);
        self.issued(token, id, held.kind, held.target, held.context, true);
        self.emit(Submission {
            token,
            target: held.target,
            op: held.op,
        });
    }
}

/// The buffer `data` owns, if it owns one.
fn owned<B>(data: SubmissionData<B>) -> Option<B> {
    match data {
        SubmissionData::Owned(buffer) => Some(buffer),
        SubmissionData::Registered(_) => None,
    }
}

/// The buffer an operation owns, if it owns one.
fn owned_op<B>(op: SubmissionOp<B>) -> Option<B> {
    match op {
        SubmissionOp::Write { data, .. } | SubmissionOp::Read { data, .. } => owned(data),
        SubmissionOp::Commit => None,
    }
}

/// A consumer operation the backend refused, handed back with its cause.
fn handed_back<V: Identities, B, C>(refused: Refused<B>, context: C) -> PushError<V, B, C> {
    let Refused { cause, submission } = refused;
    PushError {
        reason: PushRefusal::Backend(cause),
        buffer: owned_op(submission.op),
        context,
    }
}
