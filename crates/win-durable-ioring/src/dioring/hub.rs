// Copyright (c) 2026 Mike Grier
//! The hub (DI-D-53): what an instance's paths off the consumer's thread share with it -- the engine's
//! slot, the readiness event, and the count of provider completions not yet answered -- and where a
//! provider's answer and a lineage handle's release arrive, on whatever thread gives them.
//!
//! Each takes the engine's lock, acts on the core, and releases the lock before it sets the
//! readiness event, as a pool drain does; neither calls the provider, so a request either one makes
//! waits in the engine for the consumer's next call (DI-D-44). Once the instance has ended, the
//! engine's slot is empty and both do nothing -- but an answer is still counted down, because
//! ending waits for every completion to be answered or dropped (DI-D-25).

use std::io;
use std::sync::{Arc, Condvar, Mutex};

use win_sync_sys::Event;
use windows_ioring_sys::{IoBuf, IoBufMut};

use crate::windows_backend::pool::{PoolEngine, Shared};
use durable_ioring::DiagnosticSink;
use durable_ioring::DomainOutcome;
use durable_ioring::EndLineageRefusal;
use durable_ioring::EpochId;
use durable_ioring::Lineage;
use durable_ioring::StampClock;
use durable_ioring::driver::inconsistent;
use durable_ioring::driver::{Answer, DomainCall};
use durable_ioring::driver::{ENGINE_LOCK, LockWatch, lock};
use durable_ioring::driver::{Home, InstanceId};

/// What an instance shares with its paths off the consumer's thread.
pub(crate) struct Hub<E, B, C, K, R>
where
    E: EpochId + 'static,
    B: IoBuf,
    R: IoBufMut,
    K: StampClock,
{
    /// The engine's slot, shared with its delivery; empty once the instance has ended.
    engine: Shared<E, B, C, K, R>,
    /// Auto-reset; the consumer's `readiness()` is a duplicate of it.
    readiness: Event,
    /// The instance, for what an inconsistency names.
    instance: InstanceId,
    diagnostics: Arc<dyn DiagnosticSink>,
    /// Times the waits for `engine`'s lock.
    watch: LockWatch,
    /// Provider completions made and not yet answered, dropped ones included.
    ///
    /// Not taken through `watch`: it is held only for an increment or a decrement, so a wait for it
    /// is not a delay worth reporting (the engineer, 2026-10-20). If more work comes to be done
    /// with it held, that changes, and the wait for it may then warrant a lock name of its own.
    unanswered: Mutex<usize>,
    /// Notified when the last unanswered completion is answered.
    answered: Condvar,
}

impl<E, B, C, K, R> Hub<E, B, C, K, R>
where
    E: EpochId + 'static,
    B: IoBuf,
    R: IoBufMut,
    K: StampClock,
{
    /// A hub over `engine`'s slot, setting `readiness`, reporting to `diagnostics` as `instance`.
    pub(crate) fn new(
        engine: Shared<E, B, C, K, R>,
        readiness: Event,
        instance: InstanceId,
        diagnostics: Arc<dyn DiagnosticSink>,
        watch: LockWatch,
    ) -> Self {
        Self {
            engine,
            readiness,
            instance,
            diagnostics,
            watch,
            unanswered: Mutex::new(0),
            answered: Condvar::new(),
        }
    }

    /// A clone of the readiness event, sharing its handle.
    pub(crate) fn readiness(&self) -> io::Result<Event> {
        Ok(self.readiness.clone())
    }

    /// Entries became poppable where there were none: set the readiness event (DI-D-28). Called
    /// with the engine's lock released, once they are poppable.
    pub(crate) fn signal(&self) {
        // Setting an event this instance created, with full access, does not fail.
        let set = self.readiness.set();
        debug_assert!(set.is_ok(), "setting the readiness event failed: {set:?}");
    }

    /// Wait until every provider completion made has been answered or dropped (DI-D-25).
    pub(crate) fn wait_for_answers(&self) {
        let mut unanswered = lock(&self.unanswered);
        while *unanswered > 0 {
            unanswered = self
                .answered
                .wait(unanswered)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    /// The instance found its own state inconsistent (DI-D-46): reported, then panicked on.
    #[cold]
    #[track_caller]
    pub(crate) fn inconsistent(&self, what: &'static str, detail: std::fmt::Arguments<'_>) {
        inconsistent(self.instance, self.diagnostics.as_ref(), what, detail);
    }

    /// Act on the engine, if the instance has not ended, and signal if that made entries
    /// poppable.
    fn reach(&self, act: impl FnOnce(&mut PoolEngine<E, B, C, K, R>) -> bool) {
        let deliver = self
            .watch
            .lock(&self.engine, ENGINE_LOCK)
            .as_mut()
            .is_some_and(act);
        if deliver {
            self.signal();
        }
    }
}

/// Where a lineage's handles report the release of the last of them (DI-D-40). The core refuses
/// the default lineage, whose last handle is the instance's own and is released as the instance
/// ends, and there is nothing to do for it. Any other refusal -- a lineage that is not live --
/// is an inconsistency (DI-D-46): a handle releases only when no explicit end or retire disarmed
/// it, so its lineage is live.
impl<E, B, C, K, R> Home for Hub<E, B, C, K, R>
where
    E: EpochId + Send + Sync + 'static,
    B: IoBuf + Send + 'static,
    C: Send + 'static,
    K: StampClock,
    R: IoBufMut + Send + 'static,
{
    fn release(&self, lineage: Lineage) {
        let mut refused = None;
        self.reach(|engine| match engine.core.end_lineage(lineage) {
            Ok(actions) => engine.act(actions),
            Err(refusal) => {
                refused = Some(refusal);
                false
            }
        });
        // Reported with the engine's lock released, since the report panics.
        match refused {
            None | Some(EndLineageRefusal::Default) => {}
            Some(refusal) => self.inconsistent(
                "a lineage's last handle was released but the lineage was not live",
                format_args!("lineage {lineage:?}, refused as {refusal:?}"),
            ),
        }
    }
}

/// Where the consumer's provider answers (DI-D-27, DI-D-44), on whatever thread answers -- inside
/// the provider's call included, which the instance never makes with the engine's lock held.
impl<E, B, C, K, R> Answer for Hub<E, B, C, K, R>
where
    E: EpochId + Send + Sync + 'static,
    B: IoBuf + Send + 'static,
    C: Send + 'static,
    K: StampClock,
    R: IoBufMut + Send + 'static,
{
    fn issued(&self) {
        *lock(&self.unanswered) += 1;
    }

    fn answer(&self, call: &DomainCall, outcome: DomainOutcome) {
        // Counted down first, even once the instance has ended, which is what ending waits for. An
        // answer counted down before it reaches the engine is safe: ending takes the engine out of
        // its slot before it waits, so a late answer finds the slot empty.
        {
            let mut unanswered = lock(&self.unanswered);
            let Some(left) = unanswered.checked_sub(1) else {
                drop(unanswered);
                self.inconsistent(
                    "a provider answered a completion never made",
                    format_args!(
                        "lineage {:?}, seal {}, domain {:?}",
                        call.lineage, call.seal, call.name
                    ),
                );
                return;
            };
            *unanswered = left;
            if left == 0 {
                self.answered.notify_all();
            }
        }
        self.reach(|engine| {
            let actions = engine
                .core
                .provided(call.lineage, call.seal, &call.name, outcome);
            engine.act(actions)
        });
    }
}
