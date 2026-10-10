// Copyright (c) 2026 Mike Grier
//! [`DurableIoCore`]: dioring as a portable state machine (DI-D-49, DI-D-50 P11, DI-D-52).
//!
//! The core holds no backend and does no I/O. It decides, and says what to do: a consumer
//! operation is checked, then handed to the caller's push as a [`Submission`], and recorded only
//! once the push accepts it; a completion, and a seal, return [`CoreActions`] -- the commits now
//! due, and whether entries became poppable. The caller -- the driver -- pushes those submissions
//! to its backend, submits, and feeds every completion back through [`DurableIoCore::complete`].
//! Entries are popped from the core's own queue, in the order they happened.
//!
//! **Refusals** (DI-D-52). A consumer operation the backend refuses is undone: the caller's push
//! hands the submission back, nothing was recorded, no operation id was spent, and the buffer and
//! context go back to the consumer with the backend's [`RefusalCause`](crate::backend::RefusalCause).
//! A submission the core originated -- a commit -- is kept when refused: the driver hands it back
//! through [`DurableIoCore::refused`], and it is emitted again with the next actions, or at once
//! by [`DurableIoCore::retry`].
//!
//! The durability rules themselves -- seals, coverage, failures and their suspect sets -- are the
//! durability core's, shared with the Windows driver while both paths exist.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use crate::backend::{
    Completion, DuplicateTarget, Refused, Submission, SubmissionData, SubmissionOp, SubmissionToken,
};
use crate::clock::StampClock;
use crate::contract::EpochId;
use crate::diagnostics::DiagnosticSink;
use crate::dioring::durability::{
    Accepted, DEFAULT_LINEAGE, Due, Durability, Key, Routing, Sealing, WriteEnd,
};
use crate::dioring::{DomainId, epoch_state, intern_all, routing};
use crate::ids::{DioringIds, InstanceId, Lineage, OpId};
use crate::io_target::IoTargetDescription;
use crate::types::{
    DurabilityRequest, Entry, Epoch, EpochState, FlushDomain, IoTargetBusy, IoTargetKey,
    OpCompletion, OpKind, Outcome, PushError, PushRefusal, RemoveTargetError, SealError,
    UnknownLineage, WriteCaching,
};

#[cfg(test)]
mod tests;

/// The identities a core over clock `K` uses.
type V<E, K> = DioringIds<E, <K as StampClock>::Instant>;

/// What a seal answers: the request's answer and the commits it made due, or why it was refused.
type SealAnswer<E, B, K> = Result<(DurabilityRequest<V<E, K>>, CoreActions<B>), SealError<V<E, K>>>;

/// An epoch's state, or that its lineage is not this core's live one.
type StateAnswer<E, K> = Result<EpochState<V<E, K>>, UnknownLineage<V<E, K>>>;

/// What the driver does after a call into the core.
#[derive(Debug)]
#[must_use = "the submissions must be pushed and submitted, and a delivery made when asked"]
pub struct CoreActions<B> {
    /// Submissions the core originated -- commits, and any refused before -- to push to the
    /// backend in order, then submit. One the backend refuses goes back through
    /// [`DurableIoCore::refused`]; the rest may still be pushed.
    pub submit: Vec<Submission<B>>,
    /// Entries became poppable where there were none: the driver's readiness should be raised.
    pub deliver: bool,
}

impl<B> CoreActions<B> {
    /// Nothing to do.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.submit.is_empty() && !self.deliver
    }
}

/// What the core knows of a submission in flight.
enum InFlight<E: EpochId + 'static, I: crate::clock::Instant, C> {
    /// A consumer operation, with what its completion entry carries.
    Op {
        id: OpId,
        kind: OpKind<DioringIds<E, I>>,
        target: IoTargetKey,
        context: C,
    },
    /// A seal's commit of one target.
    Commit {
        lineage: Key,
        through: E,
        target: IoTargetKey,
    },
}

/// A target as the core decides with it.
struct TargetRecord {
    description: IoTargetDescription,
    /// Its flush domains, interned, sorted, without repeats; shared with each write's record.
    domains: Arc<[DomainId]>,
    /// Who answers for its writes (DI-D-27); the built-in default until a provider is supported.
    routing: Routing,
}

/// dioring's core: every lineage's durability, the targets it decides for, the operations in
/// flight, and the queue of entries for the consumer (DI-D-49). `E` is the epoch-id type, `B` the
/// backend's owned buffer, `C` the consumer's per-operation context, and `K` the clock failures are
/// stamped with.
pub struct DurableIoCore<E: EpochId + 'static, B, C, K: StampClock> {
    instance: InstanceId,
    durability: Durability<E, IoTargetKey, K::Instant, K>,
    targets: HashMap<IoTargetKey, TargetRecord>,
    /// Every flush domain any target was declared with, interned.
    domains: HashMap<FlushDomain, DomainId>,
    /// Every submission handed out and not yet completed.
    in_flight: HashMap<SubmissionToken, InFlight<E, K::Instant, C>>,
    /// Consumer operations in flight, by target, as `remove_target` reports them; a target with
    /// none has no entry.
    per_target: HashMap<IoTargetKey, usize>,
    queue: VecDeque<Entry<V<E, K>, B, C>>,
    /// Commits the backend refused, emitted again with the next actions.
    unpushed: Vec<Submission<B>>,
    next_op: u64,
    next_token: u64,
}

impl<E: EpochId + 'static, B, C, K: StampClock> DurableIoCore<E, B, C, K> {
    /// A core with no targets and only its default lineage, stamping failures with `clock` and
    /// reporting to `diagnostics`.
    pub fn new(clock: K, diagnostics: Arc<dyn DiagnosticSink>) -> Self {
        let instance = InstanceId::next();
        Self {
            instance,
            durability: Durability::new(instance, clock, diagnostics),
            targets: HashMap::new(),
            domains: HashMap::new(),
            in_flight: HashMap::new(),
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
        let routing = routing(&domains, &[]);
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
    /// naming every hold at once: operations in flight, uncovered writes, and the failures
    /// suspecting writes to it.
    pub fn remove_target(
        &mut self,
        key: IoTargetKey,
    ) -> Result<IoTargetDescription, RemoveTargetError<V<E, K>>> {
        if !self.targets.contains_key(&key) {
            return Err(RemoveTargetError::UnknownKey(key));
        }
        let in_flight = self.per_target.get(&key).copied().unwrap_or(0);
        let holds = self.durability.file_holds(key);
        if in_flight == 0 && holds.is_empty() {
            let record = self.targets.remove(&key).expect("present, checked above");
            return Ok(record.description);
        }
        Err(RemoveTargetError::Busy(IoTargetBusy {
            target: key,
            in_flight,
            held_for_gate: 0,
            lowest_gates: Vec::new(),
            uncovered: holds.uncovered,
            uncovered_through: holds.uncovered_through,
            failures: holds.failures,
        }))
    }

    /// Write `data` to `target` at `offset`, tagged `epoch`: checked, then handed to `push` as a
    /// submission, and recorded once `push` accepts it. The caller submits.
    ///
    /// # Errors
    ///
    /// [`PushError`], handing back the buffer (`None` for a registered region) and `context`:
    /// for an unknown target, an epoch whose lineage is not this core's live one, at or below its
    /// seal point, or abandoned -- checked in that order -- or `PushRefusal::Backend` with the
    /// cause `push` reported.
    #[expect(
        clippy::too_many_arguments,
        reason = "one write's whole description, and the push that issues it"
    )]
    pub fn write(
        &mut self,
        target: IoTargetKey,
        offset: u64,
        data: SubmissionData<B>,
        epoch: Epoch<V<E, K>>,
        context: C,
        caching: WriteCaching,
        push: impl FnOnce(Submission<B>) -> Result<(), Refused<B>>,
    ) -> Result<OpId, PushError<V<E, K>, B, C>> {
        if let Some(reason) = self.refusal(target, Some(epoch)) {
            return Err(PushError {
                reason,
                buffer: owned(data),
                context,
            });
        }
        let token = self.token();
        let op = SubmissionOp::Write {
            offset,
            data,
            caching,
        };
        if let Err(refused) = push(Submission { token, target, op }) {
            return Err(handed_back(refused, context));
        }
        let id = self.op_id();
        let record = &self.targets[&target];
        self.durability.pushed(Accepted {
            op: id,
            lineage: epoch.lineage.seq,
            epoch: epoch.id,
            file: target,
            target,
            routing: record.routing,
            domains: Arc::clone(&record.domains),
        });
        self.issued(token, id, OpKind::Write { epoch }, target, context);
        Ok(id)
    }

    /// Read from `target` at `offset` into `data`: checked, then handed to `push`, and recorded
    /// once `push` accepts it. The caller submits.
    ///
    /// # Errors
    ///
    /// [`PushError`], handing back the buffer and `context`: for an unknown target, or
    /// `PushRefusal::Backend` with the cause `push` reported.
    pub fn read(
        &mut self,
        target: IoTargetKey,
        offset: u64,
        data: SubmissionData<B>,
        context: C,
        push: impl FnOnce(Submission<B>) -> Result<(), Refused<B>>,
    ) -> Result<OpId, PushError<V<E, K>, B, C>> {
        if let Some(reason) = self.refusal(target, None) {
            return Err(PushError {
                reason,
                buffer: owned(data),
                context,
            });
        }
        let token = self.token();
        let op = SubmissionOp::Read { offset, data };
        if let Err(refused) = push(Submission { token, target, op }) {
            return Err(handed_back(refused, context));
        }
        let id = self.op_id();
        self.issued(token, id, OpKind::Read, target, context);
        Ok(id)
    }

    /// Seal every epoch of `through`'s lineage at or below it (DI-D-9): the answer, and the commits
    /// it made due at once.
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
                cycle: cycle
                    .into_iter()
                    .map(|gate| Epoch::new(self.name(gate.lineage), gate.epoch))
                    .collect(),
            }),
        }
    }

    /// Record a completion the backend reported: a consumer operation's entry goes on the queue
    /// before anything it makes due, so a `Durable` it leads to follows it (guarantee 5).
    ///
    /// A completion for no submission in flight -- one never handed out, or completed twice -- is
    /// the backend breaking its contract, and is panicked on (DI-D-46, DI-D-50 P9).
    pub fn complete(&mut self, completion: Completion<B>) -> CoreActions<B> {
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

    /// Take back a submission the core originated that the backend refused: it is emitted again
    /// with the next actions, or by [`retry`](Self::retry).
    ///
    /// A consumer operation never comes back this way -- its push is refused to the consumer --
    /// so one that does, or a submission never handed out, is panicked on (DI-D-46).
    pub fn refused(&mut self, refused: Refused<B>) {
        let token = refused.submission.token;
        if matches!(self.in_flight.get(&token), Some(InFlight::Commit { .. })) {
            self.unpushed.push(refused.submission);
        } else {
            self.durability.inconsistent(
                "a refusal handed back that is not one of the core's own submissions",
                format_args!("token {token:?}, cause {}", refused.cause),
            );
        }
    }

    /// The submissions refused before, to push again -- once the driver has submitted or drained,
    /// as their causes said.
    pub fn retry(&mut self) -> CoreActions<B> {
        CoreActions {
            submit: std::mem::take(&mut self.unpushed),
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

    /// A lineage key's `Lineage` name.
    fn name(&self, seq: Key) -> Lineage {
        Lineage {
            instance: self.instance,
            seq,
        }
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

    /// Why an operation on `target`, tagged `epoch` if it is a write, must be refused before it is
    /// pushed, if it must: the target, then the epoch's lineage, then its seal (guarantee 6), then
    /// whether it was abandoned (DI-D-35).
    fn refusal(
        &self,
        target: IoTargetKey,
        epoch: Option<Epoch<V<E, K>>>,
    ) -> Option<PushRefusal<V<E, K>>> {
        if !self.targets.contains_key(&target) {
            return Some(PushRefusal::UnknownFile(target));
        }
        let epoch = epoch?;
        if !self.is_live(epoch.lineage) {
            return Some(PushRefusal::UnknownLineage(epoch.lineage));
        }
        if let Some(sealed_through) = self.durability.refuses(epoch.lineage.seq, epoch.id) {
            return Some(PushRefusal::Sealed {
                epoch,
                sealed_through,
            });
        }
        if self.durability.is_abandoned(epoch.lineage.seq, epoch.id) {
            return Some(PushRefusal::EpochAbandoned { epoch });
        }
        None
    }

    /// A token no submission in flight has.
    fn token(&mut self) -> SubmissionToken {
        let token = SubmissionToken(self.next_token);
        self.next_token += 1;
        token
    }

    /// The next operation id. Only an accepted push spends one, so accepted pushes' ids are
    /// consecutive.
    fn op_id(&mut self) -> OpId {
        let id = OpId {
            instance: self.instance,
            seq: self.next_op,
        };
        self.next_op += 1;
        id
    }

    /// Record a consumer operation the backend accepted.
    fn issued(
        &mut self,
        token: SubmissionToken,
        id: OpId,
        kind: OpKind<V<E, K>>,
        target: IoTargetKey,
        context: C,
    ) {
        self.in_flight.insert(
            token,
            InFlight::Op {
                id,
                kind,
                target,
                context,
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

    /// Act on what a change made due: append its entries in order, and emit a commit for every
    /// flush now due, after any refused before. `was_empty` is whether the queue was empty when the
    /// change began.
    fn act(&mut self, due: Due<E, IoTargetKey, K::Instant>, was_empty: bool) -> CoreActions<B> {
        let Due {
            flushes,
            events,
            released,
            calls,
        } = due;
        if !released.is_empty() || !calls.is_empty() {
            self.durability.inconsistent(
                "a gate release or a provider request, though the core supports neither yet",
                format_args!("released {released:?}, calls {calls:?}"),
            );
        }
        for event in events {
            match event.into_entry(self.instance) {
                Ok(entry) => self.queue.push_back(entry),
                Err((op, _)) => self.durability.inconsistent(
                    "an operation never issued, though the core holds none",
                    format_args!("operation {op:?}"),
                ),
            }
        }
        let mut submit = std::mem::take(&mut self.unpushed);
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
        CoreActions {
            submit,
            deliver: was_empty && !self.queue.is_empty(),
        }
    }
}

/// The buffer `data` owns, if it owns one.
fn owned<B>(data: SubmissionData<B>) -> Option<B> {
    match data {
        SubmissionData::Owned(buffer) => Some(buffer),
        SubmissionData::Registered(_) => None,
    }
}

/// A consumer operation the backend refused, handed back with its cause.
fn handed_back<V: crate::contract::Identities, B, C>(
    refused: Refused<B>,
    context: C,
) -> PushError<V, B, C> {
    let Refused { cause, submission } = refused;
    let buffer = match submission.op {
        SubmissionOp::Write { data, .. } | SubmissionOp::Read { data, .. } => owned(data),
        SubmissionOp::Commit => None,
    };
    PushError {
        reason: PushRefusal::Backend(cause),
        buffer,
        context,
    }
}
