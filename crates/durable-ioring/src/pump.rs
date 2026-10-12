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

use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::backend::IoBackend;
use crate::clock::StampClock;
use crate::contract::EpochId;
use crate::engine::{Engine, EngineGuard, Slot, lock, present};
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
    /// The notification's own report, when it cannot be waited on.
    fn wait(&mut self, timeout: Option<Duration>) -> io::Result<bool>;
}

/// What a pump did.
#[derive(Debug)]
pub(crate) struct Pumped {
    /// Whether the drain made entries poppable where there were none.
    pub(crate) ready: bool,
    /// How the wait went: whether it saw the notification raised, which is `false` when no wait
    /// was made because the first drain delivered. The drain after a wait happens whatever the
    /// wait's outcome: a delivery that cannot wait still delivers what has completed.
    pub(crate) waited: io::Result<bool>,
}

/// A failed construction: why, and the engine it was given.
pub(crate) type PumpedSetup<E, Bk, C, K> =
    Result<PumpedDelivery<E, Bk, C, K>, (io::Error, Box<Engine<E, Bk, C, K>>)>;

/// The engine, drained by the consumer's own thread when it calls [`pump`](Self::pump).
pub(crate) struct PumpedDelivery<E, Bk, C, K>
where
    E: EpochId + 'static,
    Bk: IoBackend,
    K: StampClock,
{
    /// Waited on with no lock held; the engine's slot is not touched until the wait is over.
    notification: Bk::Notification,
    engine: Slot<E, Bk, C, K>,
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
    pub(crate) fn new(engine: Engine<E, Bk, C, K>) -> PumpedSetup<E, Bk, C, K> {
        match engine.backend.notification() {
            Ok(notification) => Ok(Self::with_notification(engine, notification)),
            Err(fault) => Err((fault_error(fault), Box::new(engine))),
        }
    }

    /// Deliver `engine`, waiting on `notification` -- not the backend's own -- when a pump waits.
    /// For a delivery whose waker is something else, which signals `notification` itself.
    pub(crate) fn with_notification(
        engine: Engine<E, Bk, C, K>,
        notification: Bk::Notification,
    ) -> Self {
        Self {
            notification,
            engine: Arc::new(Mutex::new(Some(engine))),
        }
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
    pub(crate) fn pump(&mut self, timeout: Option<Duration>) -> Pumped {
        if lock(&self.engine).as_mut().is_some_and(Engine::drain) {
            return Pumped {
                ready: true,
                waited: Ok(false),
            };
        }
        let waited = self.notification.wait(timeout);
        let ready = lock(&self.engine).as_mut().is_some_and(Engine::drain);
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
    pub(crate) fn engine(&self) -> EngineGuard<'_, Engine<E, Bk, C, K>> {
        present(lock(&self.engine))
    }

    /// The engine's slot, for a path of the driver's own that reaches it on another thread.
    pub(crate) fn shared(&self) -> Slot<E, Bk, C, K> {
        Arc::clone(&self.engine)
    }

    /// Stop delivering, handing the engine back: it is taken out of its slot, so a shared path
    /// finds it empty.
    pub(crate) fn into_engine(self) -> Engine<E, Bk, C, K> {
        let Self { engine, .. } = self;
        lock(&engine)
            .take()
            .expect("only stopping delivery takes the engine")
    }
}

/// A backend's fault as the `io::Error` a construction reports: the backend's own report when
/// nothing else holds it, and otherwise one of its kind and text.
fn fault_error(fault: IoFault) -> io::Error {
    Arc::try_unwrap(fault.error)
        .unwrap_or_else(|shared| io::Error::new(shared.kind(), shared.to_string()))
}
