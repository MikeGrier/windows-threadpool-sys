// Copyright (c) 2026 Mike Grier
//! Pumped delivery (DI-D-52's Model B, [DI-D-55](../DESIGN-NOTES.md#di-d-55)): the consumer's own
//! thread waits on the backend's notification and drains the driver's [`Engine`] when it says to.
//!
//! The backend has no threads, and nothing here starts one: who waits is whoever calls
//! [`PumpedDelivery::pump`]. The wait is made **outside the engine's lock**, so a push, a provider's
//! answer or a lineage handle's release on another thread is never held up by it; the drain that
//! follows takes the lock, as a pool delivery's does, and is the same drain
//! ([`Engine::drain`]).
//!
//! Written over any [`IoBackend`] whose notification is [`Waitable`], so it serves the Windows
//! backend, whose notification is the ring's completion event, and the in-memory one, whose
//! notification is a count, alike.

use alloc::sync::Arc;
use core::time::Duration;
use std::sync::Mutex;

use crate::alloc_tags::{tag, tagged};
use crate::backend::IoBackend;
use crate::clock::StampClock;
use crate::contract::EpochId;
use crate::engine::{Engine, EngineGuard, Slot, lock, present_watched};
use crate::lock_watch::{ENGINE_LOCK, LockWatch, WatchedGuard};
use crate::types::IoFault;

#[cfg(test)]
mod tests;

/// A backend notification a thread can wait on.
///
/// A notification is an **edge**: the backend raises it when its completions go from none to some,
/// not once for each ([`IoBackend`]). So a wait returns for however many raisings came since the
/// last wait that saw one, and a waiter that does not drain to empty before waiting again can
/// wait on a completion that has already been raised for. Raisings before the first wait count.
pub trait Waitable {
    /// Wait for the notification to have been raised since the last wait that saw it, for up to
    /// `timeout` -- `None` is no limit and `Duration::ZERO` only asks. Whether it was.
    ///
    /// # Errors
    ///
    /// The notification's own report, when it cannot be waited on, as a fault naming the layer
    /// that made it (DI-D-105).
    fn wait(&mut self, timeout: Option<Duration>) -> Result<bool, IoFault>;
}

/// What a pump did.
#[derive(Debug)]
pub struct Pumped {
    /// Whether the drain made entries poppable where there were none.
    pub ready: bool,
    /// How the wait went: whether it saw the notification raised, which is `false` when no wait
    /// was made because the first drain delivered. The drain after a wait happens whatever the
    /// wait's outcome: a delivery that cannot wait still delivers what has completed.
    pub waited: Result<bool, IoFault>,
}

/// A failed construction: why, and the engine it was given.
pub type PumpedSetup<E, Bk, C, K> =
    Result<PumpedDelivery<E, Bk, C, K>, (IoFault, Box<Engine<E, Bk, C, K>>)>;

/// The engine, drained by the consumer's own thread when it calls [`pump`](Self::pump).
pub struct PumpedDelivery<E, Bk, C, K>
where
    E: EpochId + 'static,
    Bk: IoBackend,
    K: StampClock,
{
    /// Waited on with no lock held; the engine's slot is not touched until the wait is over.
    notification: Bk::Notification,
    engine: Slot<E, Bk, C, K>,
    /// Times the waits for `engine`'s lock, if the driver asked.
    watch: Option<LockWatch>,
}

impl<E, Bk, C, K> PumpedDelivery<E, Bk, C, K>
where
    E: EpochId + 'static,
    Bk: IoBackend,
    Bk::Notification: Waitable,
    K: StampClock,
{
    /// Deliver `engine` to whoever pumps.
    ///
    /// # Errors
    ///
    /// The backend's, with the engine handed back: it could not make its notification.
    pub fn new(engine: Engine<E, Bk, C, K>) -> PumpedSetup<E, Bk, C, K> {
        match tagged(tag::PUMP_NEW_NOTIFICATION, || engine.backend.notification()) {
            Ok(notification) => Ok(Self::with_notification(engine, notification)),
            Err(fault) => Err((
                fault,
                tagged(tag::PUMP_NEW_HANDED_BACK, || Box::new(engine)),
            )),
        }
    }

    /// Deliver `engine`, waiting on `notification` -- not the backend's own -- when a pump waits.
    /// For a delivery whose waker is something else, which signals `notification` itself.
    pub fn with_notification(engine: Engine<E, Bk, C, K>, notification: Bk::Notification) -> Self {
        Self {
            notification,
            engine: tagged(tag::PUMP_WITH_NOTIFICATION_SLOT, || {
                Arc::new(Mutex::new(Some(engine)))
            }),
            watch: None,
        }
    }

    /// Report the waits for the engine's lock this delivery makes through `watch` (DI-D-26). A
    /// path of the driver's own that reaches the slot through [`shared`](Self::shared) is not
    /// reported unless it takes the lock through the same watch.
    #[must_use]
    pub fn with_lock_watch(mut self, watch: LockWatch) -> Self {
        self.watch = Some(watch);
        self
    }

    /// Drain the backend to empty; if that delivered entries, return. Otherwise wait up to
    /// `timeout` for the notification and drain again. A wait that times out drains too: a drain of
    /// nothing is a no-op, and a zero timeout is the pump that does not wait.
    ///
    /// **Drained before it waits**, as the notification being an edge requires (the ring crate's
    /// D-19): the completions that raised it may already have been taken by a wait that was not
    /// this one -- another handle on the same event -- and waiting for a raising that has happened
    /// would block while they sit in the backend.
    ///
    /// Returning does not promise entries: the wait may have been satisfied by a raising whose
    /// completions an earlier drain took, or by the setup signal the ring crate raises once
    /// ([D-20](../../windows-ioring-sys/DESIGN-NOTES.md#d-20)).
    pub fn pump(&mut self, timeout: Option<Duration>) -> Pumped {
        if self.locked().as_mut().is_some_and(Engine::drain) {
            return Pumped {
                ready: true,
                waited: Ok(false),
            };
        }
        let waited = tagged(tag::PUMP_PUMP_WAIT, || self.notification.wait(timeout));
        let ready = self.locked().as_mut().is_some_and(Engine::drain);
        Pumped { ready, waited }
    }
}

impl<E, Bk, C, K> PumpedDelivery<E, Bk, C, K>
where
    E: EpochId + 'static,
    Bk: IoBackend,
    K: StampClock,
{
    /// The engine, locked: what a consumer call acts through.
    pub fn engine(&self) -> EngineGuard<'_, Engine<E, Bk, C, K>> {
        present_watched(self.locked())
    }

    fn locked(&self) -> WatchedGuard<'_, Option<Engine<E, Bk, C, K>>> {
        match &self.watch {
            Some(watch) => watch.lock(&self.engine, ENGINE_LOCK),
            None => WatchedGuard::unwatched(lock(&self.engine)),
        }
    }

    /// The engine's slot, for a path of the driver's own that reaches it on another thread.
    pub fn shared(&self) -> Slot<E, Bk, C, K> {
        Arc::clone(&self.engine)
    }

    /// Stop delivering, handing the engine back: it is taken out of its slot, so a shared path
    /// finds it empty.
    pub fn into_engine(self) -> Engine<E, Bk, C, K> {
        let Self { engine, .. } = self;
        lock(&engine)
            .take()
            .expect("only stopping delivery takes the engine")
    }
}
