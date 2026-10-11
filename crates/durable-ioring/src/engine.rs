// Copyright (c) 2026 Mike Grier
//! The driver's engine (DI-D-53): a [`DurableIoCore`] and the backend that runs its I/O, under one
//! lock the driver holds -- what a delivery drains when it is woken, and what a consumer call acts
//! through.
//!
//! The engine is the one place that turns the core's [`CoreActions`] into backend pushes: a
//! submission the backend refuses goes back to the core, which keeps it and emits it again, and a
//! provider request is kept for the driver to make with the lock released (DI-D-44). Draining pops
//! the backend to empty, as an edge-triggered notification requires
//! (`windows-ioring-sys`' D-19), so it is the same for every delivery: woken by a thread pool, or
//! pumped by the consumer's own thread.

// Until the driver moves onto the engine in `DI-3.10.3.5.6.2`, only tests reach it. An expectation
// rather than an allowance, so the build fails once the driver uses it and this goes.
#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the driver moves onto the engine in DI-3.10.3.5.6.2"
    )
)]

use crate::backend::{IoBackend, Refused, Submission, SubmissionData};
use crate::clock::StampClock;
use crate::contract::EpochId;
use crate::ids::{DioringIds, OpId};
use crate::io_core::{CoreActions, DurableIoCore};
use crate::provider::ProviderRequest;
use crate::types::{
    DurabilityRequest, Epoch, IoTargetKey, PushError, ReadOptions, SealError, WriteOptions,
};

#[cfg(test)]
mod tests;

/// The identities an engine over clock `K` uses.
type V<E, K> = DioringIds<E, <K as StampClock>::Instant>;

/// What a push answers through the engine.
type Pushed<E, Bk, C, K> = Result<OpId, PushError<V<E, K>, <Bk as IoBackend>::Buffer, C>>;

/// What a seal answers through the engine: the answer, and whether entries became poppable.
type Sealed<E, K> = Result<(DurabilityRequest<V<E, K>>, bool), SealError<V<E, K>>>;

/// A core and its backend, acted on together (DI-D-53). `E` is the epoch-id type, `Bk` the
/// backend, `C` the consumer's per-operation context, and `K` the clock failures are stamped with.
pub(crate) struct Engine<E: EpochId + 'static, Bk: IoBackend, C, K: StampClock> {
    pub(crate) core: DurableIoCore<E, Bk::Buffer, C, K>,
    pub(crate) backend: Bk,
    /// Requests for the consumer's provider the core has made, in order, for the driver to make
    /// with its lock released.
    pub(crate) requests: Vec<ProviderRequest<V<E, K>>>,
}

impl<E: EpochId + 'static, Bk: IoBackend, C, K: StampClock> Engine<E, Bk, C, K> {
    /// A core and its backend, with no request outstanding.
    pub(crate) fn new(core: DurableIoCore<E, Bk::Buffer, C, K>, backend: Bk) -> Self {
        Self {
            core,
            backend,
            requests: Vec::new(),
        }
    }

    /// Act on what the core asked for: push each submission, handing a refused one back to the
    /// core, which keeps it; and keep each provider request. Whether entries became poppable.
    pub(crate) fn act(&mut self, actions: CoreActions<V<E, K>, Bk::Buffer>) -> bool {
        let CoreActions {
            submit,
            requests,
            deliver,
        } = actions;
        for submission in submit {
            if let Err(refused) = self.backend.push(submission) {
                self.core.refused(refused);
            }
        }
        self.requests.extend(requests);
        deliver
    }

    /// Drain the backend to empty, feeding each completion to the core and acting on what it makes
    /// due, until a pass pops nothing; each pass first issues what is queued. Whether entries
    /// became poppable.
    ///
    /// What the backend refused before needs no step of its own: the core re-emits it with the
    /// actions of every completion, so the pop that makes room is the one that re-pushes it.
    /// A backend that cannot read its completions ends the drain; they stay where they are, for
    /// the next.
    pub(crate) fn drain(&mut self) -> bool {
        let mut deliver = false;
        loop {
            // A failed submission leaves its entries queued, and its cause is acted on by popping,
            // which is what follows.
            let _ = self.backend.submit();
            let mut popped = false;
            while let Ok(Some(completion)) = self.backend.pop() {
                popped = true;
                let actions = self.core.complete(completion);
                deliver |= self.act(actions);
            }
            if !popped {
                return deliver;
            }
        }
    }

    /// Write `data` to `target` at `offset`, tagged `epoch`, through the backend.
    ///
    /// # Errors
    ///
    /// As [`DurableIoCore::write`].
    pub(crate) fn write(
        &mut self,
        target: IoTargetKey,
        offset: u64,
        data: SubmissionData<Bk::Buffer>,
        epoch: Epoch<V<E, K>>,
        context: C,
        options: WriteOptions<V<E, K>>,
    ) -> Pushed<E, Bk, C, K> {
        let backend = &mut self.backend;
        self.core
            .write(target, offset, data, epoch, context, options, |s| {
                push(backend, s)
            })
    }

    /// Read from `target` at `offset` into `data`, through the backend.
    ///
    /// # Errors
    ///
    /// As [`DurableIoCore::read`].
    pub(crate) fn read(
        &mut self,
        target: IoTargetKey,
        offset: u64,
        data: SubmissionData<Bk::Buffer>,
        context: C,
        options: ReadOptions<V<E, K>>,
    ) -> Pushed<E, Bk, C, K> {
        let backend = &mut self.backend;
        self.core
            .read(target, offset, data, context, options, |s| push(backend, s))
    }

    /// Seal through `through`, acting on what it made due: the answer, and whether entries became
    /// poppable.
    ///
    /// # Errors
    ///
    /// As [`DurableIoCore::make_durable_through`].
    pub(crate) fn make_durable_through(&mut self, through: Epoch<V<E, K>>) -> Sealed<E, K> {
        let (answer, actions) = self.core.make_durable_through(through)?;
        Ok((answer, self.act(actions)))
    }

    /// The provider requests made so far, in order, taken for the driver to make.
    pub(crate) fn take_requests(&mut self) -> Vec<ProviderRequest<V<E, K>>> {
        std::mem::take(&mut self.requests)
    }
}

/// Push one submission to `backend`.
fn push<Bk: IoBackend>(
    backend: &mut Bk,
    submission: Submission<Bk::Buffer>,
) -> Result<(), Refused<Bk::Buffer>> {
    backend.push(submission)
}
