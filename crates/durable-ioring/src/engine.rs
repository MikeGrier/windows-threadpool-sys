// Copyright (c) 2026 Mike Grier
//! The driver's engine (DI-D-53): a [`DurableIoCore`] and the backend that runs its I/O, under one
//! lock the driver holds -- what a delivery drains when it is woken, and what a consumer call acts
//! through.
//!
//! The engine is the one place that turns the core's [`CoreActions`] into backend pushes: a
//! submission the backend refuses goes back to the core, which keeps it to emit again or, for a
//! cause that cannot clear, ends it (DI-D-54); and a provider request is kept for the driver to
//! make with the lock released (DI-D-44). Draining pops
//! the backend to empty, as an edge-triggered notification requires
//! (`windows-ioring-sys`' D-19), so it is the same for every delivery: woken by a thread pool, or
//! pumped by the consumer's own thread.
//!
//! The engine sits in a [`Slot`] the driver may share, so a provider's answer or a lineage
//! handle's release, on any thread, can reach it under the same lock; stopping delivery takes the
//! engine out of the slot, so anything reaching it afterwards finds it empty.

use std::collections::VecDeque;
use std::ops::{Deref, DerefMut};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::alloc_tags::{tag, tagged};
use crate::backend::{IoBackend, Refused, Submission, SubmissionData};
use crate::clock::StampClock;
use crate::contract::EpochId;
use crate::ids::{DioringIds, OpId};
use crate::io_core::{CoreActions, DurableIoCore};
use crate::lock_watch::WatchedGuard;
use crate::provider::ProviderRequest;
use crate::types::{
    DurabilityRequest, Epoch, IoFault, IoTargetKey, PushError, ReadOptions, SealError, WriteOptions,
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
pub struct Engine<E: EpochId + 'static, Bk: IoBackend, C, K: StampClock> {
    /// The core, which decides.
    pub core: DurableIoCore<E, Bk::Buffer, C, K>,
    /// The backend, which runs the I/O the core asks for.
    pub backend: Bk,
    /// Requests for the consumer's provider the core has made, in order, for the driver to make
    /// with its lock released.
    pub requests: Vec<ProviderRequest<V<E, K>>>,
    /// A fault the backend gave when its completions were read, kept for a consumer's pop to report
    /// once (DI-7+.2).
    fault: Option<IoFault>,
    /// Whether a doorbell a consumer's push rang failed, so that the driver tells the consumer, who
    /// pops, which rings it again and reports a cause that stays (DI-7+.2).
    attention: bool,
}

impl<E: EpochId + 'static, Bk: IoBackend, C, K: StampClock> Engine<E, Bk, C, K> {
    /// A core and its backend, with no request outstanding.
    pub fn new(core: DurableIoCore<E, Bk::Buffer, C, K>, backend: Bk) -> Self {
        Self {
            core,
            backend,
            requests: Vec::new(),
            fault: None,
            attention: false,
        }
    }

    /// Act on what the core asked for: push each submission, handing a refused one back to the
    /// core, which keeps it or ends it (DI-D-54) and says what that made due, acted on in turn;
    /// keep each provider request, in the order made; and ring the doorbell once for everything
    /// pushed. Whether entries became poppable, or the doorbell failed, which a consumer's pop
    /// rings again and reports (DI-7+.2).
    ///
    /// A backend only queues what it is pushed, so the doorbell is the engine's to ring, once for a
    /// batch. A push refused because the queue is full is made once more after the doorbell has
    /// made room: the one condition that retry is for.
    ///
    /// It ends: a refusal either is kept, making nothing due, or ends one submission for good.
    pub fn act(&mut self, actions: CoreActions<V<E, K>, Bk::Buffer>) -> bool {
        let mut deliver = false;
        let mut pending = tagged(tag::ENGINE_ACT_PENDING, || VecDeque::from([actions]));
        while let Some(CoreActions {
            submit,
            requests,
            deliver: ready,
        }) = pending.pop_front()
        {
            deliver |= ready;
            tagged(tag::ENGINE_ACT_REQUESTS, || self.requests.extend(requests));
            for submission in submit {
                if let Err(refused) = self.push(submission) {
                    let more = self.core.refused(refused);
                    tagged(tag::ENGINE_ACT_REFUSED, || pending.push_back(more));
                }
            }
        }
        deliver | self.ring()
    }

    /// Push `submission`, and, if the backend's queue was full, once more after the doorbell has
    /// made room.
    fn push(&mut self, submission: Submission<Bk::Buffer>) -> Result<(), Refused<Bk::Buffer>> {
        match tagged(tag::ENGINE_PUSH_BACKEND, || self.backend.push(submission)) {
            Err(refused)
                if refused.cause.needs_submit()
                    && tagged(tag::ENGINE_PUSH_SUBMIT, || self.backend.submit()).is_ok() =>
            {
                tagged(tag::ENGINE_PUSH_BACKEND_AGAIN, || {
                    self.backend.push(refused.submission)
                })
            }
            pushed => pushed,
        }
    }

    /// Ring the backend's doorbell, issuing what was pushed. Whether it failed: what was pushed
    /// stays queued, and nothing else will issue it if nothing is in flight.
    fn ring(&mut self) -> bool {
        tagged(tag::ENGINE_RING_SUBMIT, || self.backend.submit()).is_err()
    }

    /// Drain the backend to empty, feeding each completion to the core and acting on what it makes
    /// due, until a pass pops nothing; each pass first issues what is queued. Whether entries
    /// became poppable, or a doorbell failed or the completions could not be read, which a
    /// consumer's pop reports (DI-7+.2).
    ///
    /// What the backend refused before needs no step of its own: the core re-emits it with the
    /// actions of every completion, so the pop that makes room is the one that re-pushes it.
    /// A backend that cannot read its completions ends the drain: they stay where they are, for
    /// the next, and the fault is kept for [`take_fault`](Self::take_fault).
    pub fn drain(&mut self) -> bool {
        let mut deliver = false;
        loop {
            deliver |= self.ring();
            let mut popped = false;
            loop {
                match tagged(tag::ENGINE_DRAIN_POP, || self.backend.pop()) {
                    Ok(Some(completion)) => {
                        self.fault = None;
                        popped = true;
                        let actions = self.core.complete(completion);
                        deliver |= self.act(actions);
                    }
                    Ok(None) => {
                        self.fault = None;
                        break;
                    }
                    Err(fault) => {
                        self.fault.get_or_insert(fault);
                        return true;
                    }
                }
            }
            if !popped {
                return deliver;
            }
        }
    }

    /// The fault the backend gave when a drain tried to read its completions, once: the first
    /// since it was last taken, unless a later read has worked, which makes it stale. A backend
    /// that cannot read its completions is not one to go on with, and the consumer's remedy is to
    /// drop the instance.
    pub fn take_fault(&mut self) -> Option<IoFault> {
        self.fault.take()
    }

    /// Whether a doorbell rung for a consumer's push failed since this was last asked, so the
    /// driver is to tell the consumer, whose pop rings it again.
    pub fn take_attention(&mut self) -> bool {
        std::mem::take(&mut self.attention)
    }
    /// Write `data` to `target` at `offset`, tagged `epoch`, through the backend.
    ///
    /// # Errors
    ///
    /// As [`DurableIoCore::write`].
    pub fn write(
        &mut self,
        target: IoTargetKey,
        offset: u64,
        data: SubmissionData<Bk::Buffer>,
        epoch: Epoch<V<E, K>>,
        context: C,
        options: WriteOptions<V<E, K>>,
    ) -> Pushed<E, Bk, C, K> {
        let pushed = self.core.write(
            target,
            offset,
            data,
            epoch,
            context,
            options,
            &mut self.backend,
        );
        self.rung(pushed)
    }

    /// Read from `target` at `offset` into `data`, through the backend.
    ///
    /// # Errors
    ///
    /// As [`DurableIoCore::read`].
    pub fn read(
        &mut self,
        target: IoTargetKey,
        offset: u64,
        data: SubmissionData<Bk::Buffer>,
        context: C,
        options: ReadOptions<V<E, K>>,
    ) -> Pushed<E, Bk, C, K> {
        let pushed = self
            .core
            .read(target, offset, data, context, options, &mut self.backend);
        self.rung(pushed)
    }

    /// Ring the doorbell for an accepted push, and note a failure for the driver to tell the
    /// consumer of.
    fn rung(&mut self, pushed: Pushed<E, Bk, C, K>) -> Pushed<E, Bk, C, K> {
        if pushed.is_ok() {
            self.attention |= self.ring();
        }
        pushed
    }

    /// Seal through `through`, acting on what it made due: the answer, and whether entries became
    /// poppable.
    ///
    /// # Errors
    ///
    /// As [`DurableIoCore::make_durable_through`].
    pub fn make_durable_through(&mut self, through: Epoch<V<E, K>>) -> Sealed<E, K> {
        let (answer, actions) = self.core.make_durable_through(through)?;
        Ok((answer, self.act(actions)))
    }

    /// The provider requests made so far, in order, taken for the driver to make.
    pub fn take_requests(&mut self) -> Vec<ProviderRequest<V<E, K>>> {
        std::mem::take(&mut self.requests)
    }
}

/// The slot an engine sits in, shared between its delivery and whatever else of the driver's
/// reaches it; empty once delivery has stopped.
pub type Slot<E, Bk, C, K> = Arc<Mutex<Option<Engine<E, Bk, C, K>>>>;

/// Lock the engine's slot. A panic while it was held is an inconsistency the core already reported
/// and panicked on (DI-D-46); the state is left as that panic found it.
pub fn lock<T>(engine: &Mutex<T>) -> MutexGuard<'_, T> {
    engine.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The engine, locked, while delivery runs: its slot is full until delivery stops.
pub struct EngineGuard<'a, T>(WatchedGuard<'a, Option<T>>);

/// The engine in a locked slot.
///
/// # Panics
///
/// If the slot is empty: it is taken only as delivery stops, and nothing locks it after.
pub fn present<T>(guard: MutexGuard<'_, Option<T>>) -> EngineGuard<'_, T> {
    present_watched(WatchedGuard::unwatched(guard))
}

/// The engine in a slot locked through a [`LockWatch`](crate::lock_watch::LockWatch), whose wait,
/// if the lock was contended, is reported when the lock is acquired, before the guard is returned,
/// and not when the guard is dropped: the pair does not include the time the lock is held.
///
/// # Panics
///
/// As [`present`].
pub fn present_watched<T>(guard: WatchedGuard<'_, Option<T>>) -> EngineGuard<'_, T> {
    assert!(
        guard.is_some(),
        "the engine is taken only as delivery stops"
    );
    EngineGuard(guard)
}

impl<T> Deref for EngineGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.0.as_ref().expect("checked when locked")
    }
}

impl<T> DerefMut for EngineGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.0.as_mut().expect("checked when locked")
    }
}
