// Copyright (c) 2026 Mike Grier
//! Pushing consumer operations: the checks every push makes, the one path all four forms share,
//! and holding a gated operation until its gate is durable, then pushing it (DI-D-23, DI-D-43).
//!
//! Every refusal hands back what the push took -- the buffer, `None` for a registered-span
//! operation, and the context -- including a refusal by the ring itself, which hands both back
//! (`windows-ioring-sys`' D-80).
//!
//! **A held operation is pushed by the consumer's next call** -- normally the `pop` that returns
//! the `Durable` entry releasing it. Pushing an owned buffer needs the buffer's bound, `IoBuf` for
//! a write or `IoBufMut` for a read, which the push that held it had and `pop` does not; so a held
//! owned operation carries the push it needs, as a function pointer made where the bound held. A
//! registered-span operation needs the registration instead, which only a call on the instance can
//! reach, never a callback.

use std::io;
use std::sync::Arc;

use windows_ioring_sys::{
    Batch, IoBuf, IoBufMut, OperationId, PushOptions, PushRefused, RegisteredBuffers,
    RegisteredSpan, WriteCaching as RingCaching,
};

use super::durability::{Accepted, Gate, GateState};
use super::relay::{Core, HeldPush, OpData, OpPush, OwnedPush, Via};
use super::{Dioring, Sidecar, TimeBase};
use crate::contract::{EpochId, PushResult};
use crate::ids::{DioringIds, Lineage, OpId};
use crate::types::{
    Epoch, IoTargetKey, OpKind, PushError, PushRefusal, ReadOptions, Tag, WriteCaching,
    WriteOptions,
};

type V<E> = DioringIds<E>;

impl<B, E, C, R, K> Dioring<B, E, C, R, K>
where
    B: Send + 'static,
    E: EpochId + Send + Sync + 'static,
    C: Send + 'static,
    R: IoBufMut,
    K: TimeBase,
{
    pub(crate) fn push_write(
        &mut self,
        file: IoTargetKey,
        offset: u64,
        buffer: B,
        tag: Tag<'_, V<E>>,
        context: C,
        options: WriteOptions<V<E>>,
    ) -> PushResult<Self>
    where
        B: IoBuf,
    {
        // A transfer is never longer than the `u32` length the ring takes.
        let len = u32::try_from(buffer.bytes_len()).unwrap_or(u32::MAX);
        self.push_op(
            file,
            offset,
            OpKind::Write { epoch: tag.epoch() },
            len,
            context,
            OpData::Owned(buffer, push_owned_write::<B, Sidecar<E, C>>),
            ring_caching(options.caching),
            options.gate,
        )
    }

    pub(crate) fn push_read(
        &mut self,
        file: IoTargetKey,
        offset: u64,
        buffer: B,
        context: C,
        options: ReadOptions<V<E>>,
    ) -> PushResult<Self>
    where
        B: IoBufMut,
    {
        self.push_op(
            file,
            offset,
            OpKind::Read,
            0,
            context,
            OpData::Owned(buffer, push_owned_read::<B, Sidecar<E, C>>),
            RingCaching::Cached,
            options.gate,
        )
    }

    pub(crate) fn push_write_registered(
        &mut self,
        file: IoTargetKey,
        offset: u64,
        span: RegisteredSpan,
        tag: Tag<'_, V<E>>,
        context: C,
        options: WriteOptions<V<E>>,
    ) -> PushResult<Self> {
        self.push_op(
            file,
            offset,
            OpKind::Write { epoch: tag.epoch() },
            span.len,
            context,
            OpData::Span(span),
            ring_caching(options.caching),
            options.gate,
        )
    }

    pub(crate) fn push_read_registered(
        &mut self,
        file: IoTargetKey,
        offset: u64,
        span: RegisteredSpan,
        context: C,
        options: ReadOptions<V<E>>,
    ) -> PushResult<Self> {
        self.push_op(
            file,
            offset,
            OpKind::Read,
            0,
            context,
            OpData::Span(span),
            RingCaching::Cached,
            options.gate,
        )
    }

    /// The registration, for the consumer's access to its bytes.
    pub(crate) fn registration(&mut self) -> io::Result<&mut RegisteredBuffers<R>> {
        self.registered.as_mut().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "this instance was built without registered buffers",
            )
        })
    }

    /// Refuse, or push or hold, one operation: the path all four forms share.
    #[expect(
        clippy::too_many_arguments,
        reason = "the four push forms' arguments, gathered at the one place they meet"
    )]
    fn push_op(
        &mut self,
        file: IoTargetKey,
        offset: u64,
        kind: OpKind<V<E>>,
        len: u32,
        context: C,
        data: OpData<B, Sidecar<E, C>>,
        caching: RingCaching,
        gate: Option<Epoch<V<E>>>,
    ) -> PushResult<Self> {
        let epoch = match kind {
            OpKind::Write { epoch } => Some(epoch),
            OpKind::Read => None,
        };
        let needs_registration = matches!(data, OpData::Span(_));
        if let Some(reason) = self.refusal(file, epoch, gate, needs_registration) {
            return Err(refused(reason, data, context));
        }
        self.issue(
            OpPush {
                id: OpId {
                    instance: self.instance,
                    seq: self.next_op,
                },
                kind,
                file,
                offset,
                requested: len,
                data,
                caching,
                context,
            },
            gate,
        )
    }

    /// Why a push must be refused before anything is reserved, if it must: the file first, then
    /// the epoch's lineage, then its seal (guarantee 6), then whether it was abandoned (DI-D-35),
    /// then the registration a registered-span operation needs, then a gate naming another
    /// instance. The rest of a gate's checks are made under dioring's lock, in `issue`.
    ///
    /// Checking the seal and abandonment here and pushing after is sound because neither changes
    /// concurrently: sealing and resolving are the consumer's `&mut self` calls, never a
    /// callback's. Nor does the lineage end meanwhile, on another thread: the push's tag borrows
    /// one of its handles, so the last cannot be released until the push returns.
    fn refusal(
        &self,
        file: IoTargetKey,
        epoch: Option<Epoch<V<E>>>,
        gate: Option<Epoch<V<E>>>,
        needs_registration: bool,
    ) -> Option<PushRefusal<V<E>>> {
        if !self.files.contains_key(&file) {
            return Some(PushRefusal::UnknownFile(file));
        }
        if let Some(epoch) = epoch
            && !self.is_live(epoch.lineage)
        {
            return Some(PushRefusal::UnknownLineage(epoch.lineage));
        }
        if let Some(epoch) = epoch
            && let Some(sealed_through) =
                (self.relay.lock().durability).refuses(epoch.lineage.seq, epoch.id)
        {
            return Some(PushRefusal::Sealed {
                epoch,
                sealed_through,
            });
        }
        if let Some(epoch) = epoch
            && (self.relay.lock().durability).is_abandoned(epoch.lineage.seq, epoch.id)
        {
            return Some(PushRefusal::EpochAbandoned { epoch });
        }
        if needs_registration && self.registered.is_none() {
            return Some(PushRefusal::NoRegisteredBuffers);
        }
        if let Some(gate) = gate
            && gate.lineage.instance != self.instance
        {
            return Some(PushRefusal::UnknownLineage(gate.lineage));
        }
        None
    }

    /// Whether `lineage` is one of this instance's live lineages: minted by it, and neither ended
    /// nor retired. Takes dioring's lock, so it is never called while that is held.
    pub(crate) fn is_live(&self, lineage: Lineage) -> bool {
        lineage.instance == self.instance && self.relay.lock().durability.is_live(lineage.seq)
    }

    /// Under dioring's lock: decide a gate, then hold the operation, or push it, record a write in
    /// its lineage, and submit; so the write is recorded before its completion can be.
    ///
    /// A gate is decided here, not in `refusal`, because what it names can change on another
    /// thread: a completion can make it durable, and the release of a handle can end its lineage.
    /// Held with the gate already durable, an operation would wait for a release already past.
    ///
    /// A submission that fails does not undo the push: the kernel leaves the entry in the
    /// submission queue, and the next submission issues it (the ring crate's D-5). So the push
    /// has been accepted -- its completion will come -- and dioring submits again at the next push
    /// or pop. No test reaches a failed submission; `SubmitIoRing` fails only on a ring the
    /// kernel can no longer use.
    fn issue(&mut self, op: OpPush<E, B, C>, gate: Option<Epoch<V<E>>>) -> PushResult<Self> {
        let record = self
            .files
            .get(&op.file)
            .expect("refusal() checked the file");
        let mut core = self.relay.lock();
        let epoch = match op.kind {
            OpKind::Write { epoch } => Some(epoch),
            OpKind::Read => None,
        };
        // Checked again here, before the ring holds anything: a panic once the push is in the
        // batch would unwind through a batch with an entry not yet submitted.
        if let Some(epoch) = epoch {
            debug_assert!(
                core.durability
                    .refuses(epoch.lineage.seq, epoch.id)
                    .is_none()
                    && !core.durability.is_abandoned(epoch.lineage.seq, epoch.id),
                "refusal() checked the seal and abandonment"
            );
        }
        let named = gate;
        let held = match gate {
            None => None,
            Some(gate) => {
                let at = Gate {
                    lineage: gate.lineage.seq,
                    epoch: gate.id,
                };
                let reason = match core.durability.gate_state(at) {
                    GateState::Durable => None,
                    GateState::Waiting => {
                        let cycle = epoch.and_then(|epoch| {
                            let write = Gate {
                                lineage: epoch.lineage.seq,
                                epoch: epoch.id,
                            };
                            core.durability.write_cycle(write, at)
                        });
                        match cycle {
                            None => Some(Ok(at)),
                            Some(cycle) => Some(Err(PushRefusal::GateCycle {
                                cycle: self.epochs(cycle),
                            })),
                        }
                    }
                    GateState::Abandoned => Some(Err(PushRefusal::GateAbandoned { gate })),
                    GateState::Unknown => Some(Err(PushRefusal::UnknownLineage(gate.lineage))),
                };
                match reason {
                    Some(Err(reason)) => return Err(refused(reason, op.data, op.context)),
                    Some(Ok(at)) => Some(at),
                    None => None,
                }
            }
        };
        let accepted = epoch.map(|epoch| Accepted {
            op: op.id,
            lineage: epoch.lineage.seq,
            epoch: epoch.id,
            file: op.file,
            target: record.target.flush_target(),
            routing: record.routing,
            domains: Arc::clone(&record.domains),
        });
        let id = op.id;
        if let (Some(gate), Some(named)) = (held, named) {
            core.durability.hold(id, accepted, gate);
            let target = record.target.flush_target();
            core.held.insert(
                id.seq,
                HeldPush {
                    op,
                    target,
                    gate: named,
                },
            );
            // Only an accepted push spends an identity, so the identities of accepted pushes are
            // consecutive.
            self.next_op += 1;
            return Ok(id);
        }
        let OpPush {
            kind,
            file,
            offset,
            requested,
            data,
            caching,
            context,
            ..
        } = op;
        let sidecar = Sidecar::Consumer {
            id,
            kind,
            file,
            offset,
            requested,
            context,
        };
        let mut scope = self.delivery.scope();
        let mut batch = scope.batch();
        let via = record.target.via();
        if let Err(refused) = self.push_now(&mut batch, via, data, kind, offset, caching, sidecar) {
            let PushRefused {
                error,
                payload,
                extra,
                ..
            } = refused;
            let Sidecar::Consumer { context, .. } = extra else {
                unreachable!("a consumer push carries a consumer sidecar");
            };
            return Err(PushError {
                reason: PushRefusal::Ring(error),
                buffer: payload,
                context,
            });
        }
        if let Some(accepted) = accepted {
            core.durability.pushed(accepted);
        }
        *core.in_flight.entry(file).or_insert(0) += 1;
        core.unsubmitted = batch.submit().is_err();
        self.next_op += 1;
        Ok(id)
    }

    /// Push every released operation, in the order released, then submit. Each is recorded as
    /// issued under the lock it is pushed under. One the ring refuses waits, with those after it,
    /// for the next call; no test reaches that, because the submission queue has room for every
    /// operation the instance holds.
    pub(crate) fn release_ready(&self, core: &mut Core<E, B, C, K>) -> Option<io::Error> {
        if core.ready.is_empty() {
            return None;
        }
        let mut scope = self.delivery.scope();
        let mut batch = scope.batch();
        while let Some(&id) = core.ready.front() {
            let Some(HeldPush { op, target, gate }) = core.held.remove(&id.seq) else {
                debug_assert!(false, "a released operation with nothing held for it");
                core.ready.pop_front();
                continue;
            };
            let OpPush {
                id,
                kind,
                file,
                offset,
                requested,
                data,
                caching,
                context,
            } = op;
            let restore = match &data {
                OpData::Owned(_, push) => Restore::Owned(*push),
                OpData::Span(span) => Restore::Span(*span),
            };
            let sidecar = Sidecar::Consumer {
                id,
                kind,
                file,
                offset,
                requested,
                context,
            };
            match self.push_now(
                &mut batch,
                target.via(),
                data,
                kind,
                offset,
                caching,
                sidecar,
            ) {
                Ok(_) => {
                    core.ready.pop_front();
                    core.durability.issued(id);
                    *core.in_flight.entry(file).or_insert(0) += 1;
                }
                Err(refused) => {
                    let PushRefused { payload, extra, .. } = refused;
                    let Sidecar::Consumer { context, .. } = extra else {
                        unreachable!("a consumer push carries a consumer sidecar");
                    };
                    let data = match (restore, payload) {
                        (Restore::Owned(push), Some(buffer)) => OpData::Owned(buffer, push),
                        (Restore::Span(span), _) => OpData::Span(span),
                        (Restore::Owned(_), None) => {
                            unreachable!("a refused owned push hands its buffer back")
                        }
                    };
                    let op = OpPush {
                        id,
                        kind,
                        file,
                        offset,
                        requested,
                        data,
                        caching,
                        context,
                    };
                    core.held.insert(id.seq, HeldPush { op, target, gate });
                    break;
                }
            }
        }
        let submitted = batch.submit().err();
        core.unsubmitted |= submitted.is_some();
        submitted
    }

    /// Push one operation to `batch`, through the push an owned one carries or with the
    /// registration a span one needs.
    #[expect(
        clippy::too_many_arguments,
        reason = "one operation's whole description, as the ring's pushes take it"
    )]
    fn push_now(
        &self,
        batch: &mut Batch<'_, B, Sidecar<E, C>>,
        via: Via<'_>,
        data: OpData<B, Sidecar<E, C>>,
        kind: OpKind<V<E>>,
        offset: u64,
        caching: RingCaching,
        sidecar: Sidecar<E, C>,
    ) -> Result<OperationId, PushRefused<B, Sidecar<E, C>>> {
        let span = match data {
            OpData::Owned(buffer, push) => {
                return push(batch, via, buffer, offset, caching, sidecar);
            }
            OpData::Span(span) => span,
        };
        let registered = self
            .registered
            .as_ref()
            .expect("refusal() checked for a registration");
        let options = PushOptions::new();
        match (kind, via) {
            (OpKind::Write { .. }, Via::Registered(index)) => batch
                .write_registered_owned(index, registered, span, sidecar, offset, options, caching),
            (OpKind::Write { .. }, Via::Shared(handle)) => batch.write_registered_owned(
                handle, registered, span, sidecar, offset, options, caching,
            ),
            (OpKind::Read, Via::Registered(index)) => {
                batch.read_registered_owned(index, registered, span, sidecar, offset, options)
            }
            (OpKind::Read, Via::Shared(handle)) => {
                batch.read_registered_owned(handle, registered, span, sidecar, offset, options)
            }
        }
    }
}

/// What a refused release needs to put its operation back.
enum Restore<B, S> {
    Owned(OwnedPush<B, S>),
    Span(RegisteredSpan),
}

/// A refusal, handing back what the push took.
fn refused<E: EpochId + 'static, B, S, C>(
    reason: PushRefusal<V<E>>,
    data: OpData<B, S>,
    context: C,
) -> PushError<V<E>, B, C> {
    PushError {
        reason,
        buffer: match data {
            OpData::Owned(buffer, _) => Some(buffer),
            OpData::Span(_) => None,
        },
        context,
    }
}

/// Push an owned write: made where `B: IoBuf` holds, and carried by a held one.
fn push_owned_write<B: IoBuf, S>(
    batch: &mut Batch<'_, B, S>,
    via: Via<'_>,
    buffer: B,
    offset: u64,
    caching: RingCaching,
    extra: S,
) -> Result<OperationId, PushRefused<B, S>> {
    let options = PushOptions::new();
    match via {
        Via::Registered(index) => batch.write_owned(index, buffer, extra, offset, options, caching),
        Via::Shared(handle) => batch.write_owned(handle, buffer, extra, offset, options, caching),
    }
}

/// Push an owned read: made where `B: IoBufMut` holds, and carried by a held one.
fn push_owned_read<B: IoBufMut, S>(
    batch: &mut Batch<'_, B, S>,
    via: Via<'_>,
    buffer: B,
    offset: u64,
    _caching: RingCaching,
    extra: S,
) -> Result<OperationId, PushRefused<B, S>> {
    let options = PushOptions::new();
    match via {
        Via::Registered(index) => batch.read_owned(index, buffer, extra, offset, options),
        Via::Shared(handle) => batch.read_owned(handle, buffer, extra, offset, options),
    }
}

/// The contract's caching choice, as the ring crate spells it.
fn ring_caching(caching: WriteCaching) -> RingCaching {
    match caching {
        WriteCaching::Cached => RingCaching::Cached,
        WriteCaching::WriteThrough => RingCaching::WriteThrough,
    }
}
