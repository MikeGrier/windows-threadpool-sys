// Copyright (c) 2026 Mike Grier
//! Pushing consumer operations: the checks every push makes, and the one path all four forms
//! share.
//!
//! Every refusal hands back what the push took -- the buffer, `None` for a registered-span
//! operation, and the context -- including a refusal by the ring itself, which hands both back
//! (`windows-ioring-sys`' D-80).

use std::io;
use std::sync::Arc;

use windows_ioring_sys::{
    Batch, IoBuf, IoBufMut, OperationId, PushOptions, PushRefused, RegisteredBuffers,
    RegisteredSpan, WriteCaching as RingCaching,
};

use super::durability::Accepted;
use super::{Dioring, FileSlot, Sidecar, TimeBase};
use crate::contract::{EpochId, PushResult};
use crate::ids::{DioringIds, Lineage, OpId};
use crate::types::{
    Epoch, FileKey, OpKind, PushError, PushRefusal, ReadOptions, WriteCaching, WriteOptions,
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
        file: FileKey,
        offset: u64,
        buffer: B,
        epoch: Epoch<V<E>>,
        context: C,
        options: WriteOptions<V<E>>,
    ) -> PushResult<Self>
    where
        B: IoBuf,
    {
        no_gate(options.gate);
        if let Some(reason) = self.refusal(file, Some(epoch), false) {
            return Err(PushError {
                reason,
                buffer: Some(buffer),
                context,
            });
        }
        let caching = ring_caching(options.caching);
        // A transfer is never longer than the `u32` length the ring takes.
        let len = u32::try_from(buffer.bytes_len()).unwrap_or(u32::MAX);
        self.issue(
            file,
            offset,
            OpKind::Write { epoch },
            len,
            context,
            move |batch, slot, _, sidecar| match slot {
                FileSlot::Registered { index, .. } => {
                    batch.write_owned(index, buffer, sidecar, offset, PushOptions::new(), caching)
                }
                FileSlot::Shared(handle) => {
                    batch.write_owned(handle, buffer, sidecar, offset, PushOptions::new(), caching)
                }
            },
        )
    }

    pub(crate) fn push_read(
        &mut self,
        file: FileKey,
        offset: u64,
        buffer: B,
        context: C,
        options: ReadOptions<V<E>>,
    ) -> PushResult<Self>
    where
        B: IoBufMut,
    {
        no_gate(options.gate);
        if let Some(reason) = self.refusal(file, None, false) {
            return Err(PushError {
                reason,
                buffer: Some(buffer),
                context,
            });
        }
        self.issue(
            file,
            offset,
            OpKind::Read,
            0,
            context,
            move |batch, slot, _, sidecar| match slot {
                FileSlot::Registered { index, .. } => {
                    batch.read_owned(index, buffer, sidecar, offset, PushOptions::new())
                }
                FileSlot::Shared(handle) => {
                    batch.read_owned(handle, buffer, sidecar, offset, PushOptions::new())
                }
            },
        )
    }

    pub(crate) fn push_write_registered(
        &mut self,
        file: FileKey,
        offset: u64,
        span: RegisteredSpan,
        epoch: Epoch<V<E>>,
        context: C,
        options: WriteOptions<V<E>>,
    ) -> PushResult<Self> {
        no_gate(options.gate);
        if let Some(reason) = self.refusal(file, Some(epoch), true) {
            return Err(PushError {
                reason,
                buffer: None,
                context,
            });
        }
        let caching = ring_caching(options.caching);
        self.issue(
            file,
            offset,
            OpKind::Write { epoch },
            span.len,
            context,
            move |batch, slot, registered, sidecar| {
                let registered = registered.expect("refusal() checked for a registration");
                match slot {
                    FileSlot::Registered { index, .. } => batch.write_registered_owned(
                        index,
                        registered,
                        span,
                        sidecar,
                        offset,
                        PushOptions::new(),
                        caching,
                    ),
                    FileSlot::Shared(handle) => batch.write_registered_owned(
                        handle,
                        registered,
                        span,
                        sidecar,
                        offset,
                        PushOptions::new(),
                        caching,
                    ),
                }
            },
        )
    }

    pub(crate) fn push_read_registered(
        &mut self,
        file: FileKey,
        offset: u64,
        span: RegisteredSpan,
        context: C,
        options: ReadOptions<V<E>>,
    ) -> PushResult<Self> {
        no_gate(options.gate);
        if let Some(reason) = self.refusal(file, None, true) {
            return Err(PushError {
                reason,
                buffer: None,
                context,
            });
        }
        self.issue(
            file,
            offset,
            OpKind::Read,
            0,
            context,
            move |batch, slot, registered, sidecar| {
                let registered = registered.expect("refusal() checked for a registration");
                match slot {
                    FileSlot::Registered { index, .. } => batch.read_registered_owned(
                        index,
                        registered,
                        span,
                        sidecar,
                        offset,
                        PushOptions::new(),
                    ),
                    FileSlot::Shared(handle) => batch.read_registered_owned(
                        handle,
                        registered,
                        span,
                        sidecar,
                        offset,
                        PushOptions::new(),
                    ),
                }
            },
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

    /// Why a push must be refused before anything is reserved, if it must: the file first, then
    /// the epoch's lineage, then its seal (guarantee 6), then whether it was abandoned (DI-D-35),
    /// then the registration a registered-span operation needs.
    ///
    /// Checking the seal and abandonment here and pushing after is sound because neither changes
    /// concurrently: sealing and resolving are the consumer's `&mut self` calls, never a
    /// callback's.
    fn refusal(
        &self,
        file: FileKey,
        epoch: Option<Epoch<V<E>>>,
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
            && let Some(sealed_through) = self.relay.lock().lineage.refuses(epoch.id)
        {
            return Some(PushRefusal::Sealed {
                epoch,
                sealed_through,
            });
        }
        if let Some(epoch) = epoch
            && self.relay.lock().lineage.is_abandoned(epoch.id)
        {
            return Some(PushRefusal::EpochAbandoned { epoch });
        }
        if needs_registration && self.registered.is_none() {
            return Some(PushRefusal::NoRegisteredBuffers);
        }
        None
    }

    /// Whether `lineage` is one of this instance's live lineages. Only the default exists until
    /// lineages can be minted (DI-3.2.5); a lineage of another instance never is.
    pub(crate) fn is_live(&self, lineage: Lineage) -> bool {
        lineage.instance == self.instance && lineage.seq == 0
    }

    /// Mint the next operation's identity, record it in the sidecar with the consumer's context,
    /// push it through `push`, record a write -- asking to write `len` bytes -- in its lineage, and
    /// submit -- all under dioring's
    /// lock, so the write is recorded before its completion can be.
    ///
    /// A submission that fails does not undo the push: the kernel leaves the entry in the
    /// submission queue, and the next submission issues it (the ring crate's D-5). So the push
    /// has been accepted -- its completion will come -- and dioring submits again at the next push
    /// or pop. No test reaches a failed submission; `SubmitIoRing` fails only on a ring the
    /// kernel can no longer use.
    fn issue<P>(
        &mut self,
        file: FileKey,
        offset: u64,
        kind: OpKind<V<E>>,
        len: u32,
        context: C,
        push: P,
    ) -> PushResult<Self>
    where
        P: FnOnce(
            &mut Batch<'_, B, Sidecar<E, C>>,
            &FileSlot,
            Option<&RegisteredBuffers<R>>,
            Sidecar<E, C>,
        ) -> Result<OperationId, PushRefused<B, Sidecar<E, C>>>,
    {
        let id = OpId {
            instance: self.instance,
            seq: self.next_op,
        };
        let sidecar = Sidecar::Consumer {
            id,
            kind,
            file,
            offset,
            context,
        };
        let record = self.files.get(&file).expect("refusal() checked the file");
        let mut core = self.relay.lock();
        // Checked again here, before the ring holds anything: a panic once the push is in the
        // batch would unwind through a batch with an entry not yet submitted.
        if let OpKind::Write { epoch } = kind {
            debug_assert!(
                core.lineage.refuses(epoch.id).is_none() && !core.lineage.is_abandoned(epoch.id),
                "refusal() checked the seal and abandonment"
            );
        }
        let mut scope = self.delivery.scope();
        let mut batch = scope.batch();
        if let Err(refused) = push(
            &mut batch,
            &record.target,
            self.registered.as_ref(),
            sidecar,
        ) {
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
        if let OpKind::Write { epoch } = kind {
            core.lineage.pushed(Accepted {
                op: id,
                epoch: epoch.id,
                file,
                target: record.target.flush_target(),
                routing: record.routing,
                domains: Arc::clone(&record.domains),
                len,
            });
        }
        core.unsubmitted = batch.submit().is_err();
        // Only an accepted push spends an identity, so the identities of accepted pushes are
        // consecutive.
        self.next_op += 1;
        Ok(id)
    }
}

/// No public setter makes a gate until gates are honoured (DI-3.2.5); one here would be a
/// crate-internal caller's defect.
fn no_gate<E: EpochId + 'static>(gate: Option<Epoch<V<E>>>) {
    debug_assert!(gate.is_none(), "a gate before gates are honoured");
}

/// The contract's caching choice, as the ring crate spells it.
fn ring_caching(caching: WriteCaching) -> RingCaching {
    match caching {
        WriteCaching::Cached => RingCaching::Cached,
        WriteCaching::WriteThrough => RingCaching::WriteThrough,
    }
}
