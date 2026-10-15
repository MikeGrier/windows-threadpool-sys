// Copyright (c) 2026 Mike Grier
//! [`DurableIoCore`]: dioring as a portable state machine (DI-D-49, DI-D-50 P11, DI-D-52).
//!
//! The core holds no backend and does no I/O. It decides, and says what to do: a consumer
//! operation is checked, then either held for its gate or pushed, as a [`Submission`], to the
//! backend the caller lends it, and recorded only once the push accepts it; everything else returns
//! [`CoreActions`] -- submissions the core originated, requests for the consumer's provider, and
//! whether entries became poppable. The caller -- the driver -- pushes those submissions to its
//! backend, submits, hands the requests to its provider, and feeds every completion and every
//! provider answer back. Entries are popped from the core's own queue, in the order they happened.
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
//! routes the domains it names to the consumer's provider: a seal's request comes out in
//! [`CoreActions::requests`], and each domain's answer goes back through
//! [`DurableIoCore::provided`]. Making the request, and with what lock released, is the driver's
//! (DI-D-44).
//!
//! The durability rules themselves -- seals, coverage, failures and their suspect sets -- are the
//! durability core's, shared with the Windows driver while both paths exist.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;

use crate::backend::{
    Completion, DuplicateTarget, IoBackend, Refused, Submission, SubmissionData, SubmissionOp,
    SubmissionToken, refusal_fault,
};
use crate::clock::StampClock;
use crate::contract::{EpochId, Identities};
use crate::diagnostics::DiagnosticSink;
use crate::domains::{DomainId, epoch_state, intern_all, routing};
use crate::durability::{
    Accepted, DEFAULT_LINEAGE, Due, Durability, Gate, GateState, Key, Routing, Sealing, WriteEnd,
};
use crate::ids::{DioringIds, InstanceId, Lineage, OpId};
use crate::io_target::IoTargetDescription;
use crate::provider::ProviderRequest;
use crate::types::{
    DurabilityRequest, Entry, Epoch, EpochState, FlushDomain, HeldOperation, IoTargetBusy,
    IoTargetKey, Leftovers, OpCompletion, OpKind, Outcome, PushError, PushRefusal, ReadOptions,
    RemoveTargetError, SealError, UnknownLineage, WriteOptions,
};

mod failures;
mod lineages;
mod provider;

pub use provider::DuplicateDomain;

#[cfg(test)]
mod tests;

/// The identities a core over clock `K` uses.
type V<E, K> = DioringIds<E, <K as StampClock>::Instant>;

/// What a core over clock `K` asks of its driver.
type Actions<E, B, K> = CoreActions<V<E, K>, B>;

/// What a seal answers: the request's answer and what it made due, or why it was refused.
type SealAnswer<E, B, K> =
    Result<(DurabilityRequest<V<E, K>>, Actions<E, B, K>), SealError<V<E, K>>>;

/// An epoch's state, or that its lineage is not this core's live one.
type StateAnswer<E, K> = Result<EpochState<V<E, K>>, UnknownLineage<V<E, K>>>;

/// What the driver does after a call into the core.
#[derive(Debug)]
#[must_use = "the submissions must be pushed and submitted, the requests made, and a delivery made when asked"]
pub struct CoreActions<V: Identities, B> {
    /// Submissions the core originated -- commits, operations their gates released, and any
    /// refused before -- to push to the backend in order, then submit. One the backend refuses
    /// goes back through [`DurableIoCore::refused`], whose own actions are acted on in turn; the
    /// rest may still be pushed.
    pub submit: Vec<Submission<B>>,
    /// Requests for the consumer's provider, in the order made; each domain's answer goes back
    /// through [`DurableIoCore::provided`].
    pub requests: Vec<ProviderRequest<V>>,
    /// Entries became poppable where there were none: the driver's readiness should be raised.
    pub deliver: bool,
}

impl<V: Identities, B> CoreActions<V, B> {
    /// Nothing to do.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.submit.is_empty() && self.requests.is_empty() && !self.deliver
    }

    /// Actions with nothing to do.
    fn none() -> Self {
        Self {
            submit: Vec::new(),
            requests: Vec::new(),
            deliver: false,
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
    /// Its flush domains, interned, sorted, without repeats; shared with each write's record.
    domains: Arc<[DomainId]>,
    /// Who answers for its writes (DI-D-27): the built-in default, the provider, or both.
    routing: Routing,
}

/// dioring's core: every lineage's durability, the targets it decides for, the operations in
/// flight or held for their gates, and the queue of entries for the consumer (DI-D-49). `E` is the
/// epoch-id type, `B` the backend's owned buffer, `C` the consumer's per-operation context, and `K`
/// the clock failures are stamped with.
pub struct DurableIoCore<E: EpochId + 'static, B, C, K: StampClock> {
    instance: InstanceId,
    durability: Durability<E, IoTargetKey, K::Instant, K>,
    targets: HashMap<IoTargetKey, TargetRecord>,
    /// Every flush domain any target or the provider was declared with, interned.
    domains: HashMap<FlushDomain, DomainId>,
    /// The domains the consumer's provider serves, sorted; empty without one.
    served: Arc<[DomainId]>,
    /// Each served domain's name, at its id's position in `served`.
    served_names: Vec<FlushDomain>,
    /// Every submission handed out and not yet completed.
    in_flight: HashMap<SubmissionToken, InFlight<E, K::Instant, C>>,
    /// The operations held for their gates, by push order.
    held: BTreeMap<u64, HeldOp<E, K::Instant, B, C>>,
    /// Consumer operations in flight, by target, as `remove_target` reports them; a target with
    /// none has no entry.
    per_target: HashMap<IoTargetKey, usize>,
    queue: VecDeque<Entry<V<E, K>, B, C>>,
    /// Submissions the core originated that the backend refused for a retryable cause, emitted
    /// again with the next actions.
    unpushed: Vec<Submission<B>>,
    next_op: u64,
    next_token: u64,
}

impl<E: EpochId + 'static, B, C, K: StampClock> DurableIoCore<E, B, C, K> {
    /// A core with no targets, no provider, and only its default lineage, stamping failures with
    /// `clock` and reporting to `diagnostics`.
    pub fn new(clock: K, diagnostics: Arc<dyn DiagnosticSink>) -> Self {
        let instance = InstanceId::next();
        Self {
            instance,
            durability: Durability::new(instance, clock, diagnostics),
            targets: HashMap::new(),
            domains: HashMap::new(),
            served: Arc::from([]),
            served_names: Vec::new(),
            in_flight: HashMap::new(),
            held: BTreeMap::new(),
            per_target: HashMap::new(),
            queue: VecDeque::new(),
            unpushed: Vec::new(),
            next_op: 0,
            next_token: 0,
        }
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
    /// [`DuplicateTarget`], handing the description back, if `key` is in use.
    pub fn add_target(
        &mut self,
        key: IoTargetKey,
        description: IoTargetDescription,
    ) -> Result<(), DuplicateTarget<IoTargetDescription>> {
        if self.targets.contains_key(&key) {
            return Err(DuplicateTarget {
                key,
                target: description,
            });
        }
        let domains = intern_all(&mut self.domains, description.flush_domains.iter().cloned());
        let routing = routing(&domains, &self.served);
        self.targets.insert(
            key,
            TargetRecord {
                description,
                domains,
                routing,
            },
        );
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
    ) -> Result<IoTargetDescription, RemoveTargetError<V<E, K>>> {
        if !self.targets.contains_key(&key) {
            return Err(RemoveTargetError::UnknownKey(key));
        }
        let in_flight = self.per_target.get(&key).copied().unwrap_or(0);
        let mut held_for_gate = 0;
        let mut lowest: BTreeMap<u64, Epoch<V<E, K>>> = BTreeMap::new();
        for held in self.held.values().filter(|held| held.target == key) {
            held_for_gate += 1;
            lowest
                .entry(held.gate.lineage.seq)
                .and_modify(|low| {
                    if held.gate.id < low.id {
                        *low = held.gate;
                    }
                })
                .or_insert(held.gate);
        }
        let holds = self.durability.file_holds(key);
        if in_flight == 0 && held_for_gate == 0 && holds.is_empty() {
            let record = self.targets.remove(&key).expect("present, checked above");
            return Ok(record.description);
        }
        Err(RemoveTargetError::Busy(IoTargetBusy {
            target: key,
            in_flight,
            held_for_gate,
            lowest_gates: lowest.into_values().collect(),
            uncovered: holds.uncovered,
            uncovered_through: holds.uncovered_through,
            failures: holds.failures,
        }))
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
    pub fn make_durable_through(&mut self, through: Epoch<V<E, K>>) -> SealAnswer<E, B, K> {
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
    pub fn complete(&mut self, completion: Completion<B>) -> Actions<E, B, K> {
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
                self.queue.push_back(Entry::Op(OpCompletion {
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
    pub fn refused(&mut self, refused: Refused<B>) -> Actions<E, B, K> {
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
            return CoreActions::none();
        }
        if cause.is_retryable() {
            self.unpushed.push(submission);
            return CoreActions::none();
        }
        self.complete(Completion {
            token,
            result: Err(refusal_fault(cause)),
            buffer: owned_op(submission.op),
        })
    }

    /// The submissions refused before, to push again -- once the driver has submitted or drained,
    /// as their causes said.
    pub fn retry(&mut self) -> Actions<E, B, K> {
        CoreActions {
            submit: std::mem::take(&mut self.unpushed),
            requests: Vec::new(),
            deliver: false,
        }
    }

    /// The next entry, if there is one.
    pub fn pop(&mut self) -> Option<Entry<V<E, K>, B, C>> {
        self.queue.pop_front()
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

    /// End the core, handing back what it still holds: the operations held for their gates, which
    /// were never issued, in push order; the unresolved failures; and every lineage with its final
    /// high-water mark. What is in flight is the backend's to run down.
    pub fn close(self) -> Leftovers<V<E, K>, B, C> {
        let failures = self.durability.failures();
        let lineages = self.durability.lineages();
        let held = self
            .held
            .into_values()
            .map(|held| HeldOperation {
                id: held.id,
                kind: held.kind,
                gate: held.gate,
                buffer: owned_op(held.op),
                context: held.context,
            })
            .collect();
        Leftovers {
            held,
            failures,
            lineages,
        }
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
        cycle
            .into_iter()
            .map(|gate| Epoch::new(self.name(gate.lineage), gate.epoch))
            .collect()
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
        if let (Some(at), Some(named)) = (waiting, gate) {
            if let Err(cause) = backend.check(target, &op) {
                return Err(PushError {
                    reason: PushRefusal::Backend(cause),
                    buffer: owned_op(op),
                    context,
                });
            }
            let id = self.op_id();
            let accepted = epoch.map(|epoch| self.accepted(id, target, epoch));
            self.durability.hold(id, accepted, at);
            self.held.insert(
                id.seq,
                HeldOp {
                    id,
                    kind,
                    target,
                    op,
                    context,
                    gate: named,
                },
            );
            return Ok(id);
        }
        let token = self.token();
        if let Err(refused) = backend.push(Submission { token, target, op }) {
            return Err(handed_back(refused, context));
        }
        let id = self.op_id();
        if let Some(epoch) = epoch {
            let accepted = self.accepted(id, target, epoch);
            self.durability.pushed(accepted);
        }
        self.issued(token, id, kind, target, context, false);
        Ok(id)
    }

    /// Why an operation on `target` -- tagged `epoch` if it is a write, gated on `gate` if it has
    /// one -- must be refused, if it must: the target, then the epoch's lineage, then its seal
    /// (guarantee 6), then whether it was abandoned (DI-D-35), then the gate. Otherwise whether it
    /// waits: the gate's epoch in the core's terms, or `None` to push it now.
    fn decide(
        &self,
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
            domains: Arc::clone(&record.domains),
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
        self.in_flight.insert(
            token,
            InFlight::Op {
                id,
                kind,
                target,
                context,
                released,
            },
        );
        *self.per_target.entry(target).or_insert(0) += 1;
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
    fn act(&mut self, due: Due<E, IoTargetKey, K::Instant>, was_empty: bool) -> Actions<E, B, K> {
        let Due {
            flushes,
            events,
            released,
            calls,
        } = due;
        for event in events {
            match event.into_entry(self.instance) {
                Ok(entry) => self.queue.push_back(entry),
                Err((op, abandoned)) => {
                    let abandoned = Epoch::new(self.name(abandoned.lineage), abandoned.epoch);
                    self.never_issued(op, abandoned);
                }
            }
        }
        let mut submit = std::mem::take(&mut self.unpushed);
        for id in released {
            self.release(id, &mut submit);
        }
        for flush in flushes {
            let token = self.token();
            self.in_flight.insert(
                token,
                InFlight::Commit {
                    lineage: flush.lineage,
                    through: flush.through,
                    target: flush.file,
                },
            );
            submit.push(Submission {
                token,
                target: flush.file,
                op: SubmissionOp::Commit,
            });
        }
        let requests = calls.into_iter().map(|call| self.request(call)).collect();
        CoreActions {
            submit,
            requests,
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
        self.queue.push_back(Entry::Op(OpCompletion {
            id: held.id,
            kind: held.kind,
            outcome: Outcome::NeverIssued { abandoned },
            buffer: owned_op(held.op),
            context: held.context,
        }));
    }

    /// A held operation's gate is durable: emit it, recorded as in flight from now.
    fn release(&mut self, id: OpId, submit: &mut Vec<Submission<B>>) {
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
        submit.push(Submission {
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
