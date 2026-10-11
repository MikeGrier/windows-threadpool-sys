// Copyright (c) 2026 Mike Grier
//! Pool delivery (DI-D-52's Model A, DI-D-53): the ring crate's `CompletionWake` over the Windows
//! backend's ring, draining the driver's [`Engine`] whenever the thread pool says to.
//!
//! The wake holds no lock and no ring; woken, this takes the engine's lock, drains the backend to
//! empty through the core, releases the lock, and only then tells the driver that entries became
//! poppable -- so they are poppable before it is told (DI-D-28), and the driver's own code never
//! runs under the engine's lock. Two wakes may overlap (the ring crate's D-83); the engine's lock
//! serializes them.
//!
//! The engine sits in a slot the driver may share -- [`PoolDelivery::shared`] -- so a provider's
//! answer or a lineage handle's release, on any thread, can reach it under the same lock. Stopping
//! delivery takes the engine out of the slot, so anything reaching it afterwards finds it empty.

use std::io;
use std::ops::{Deref, DerefMut};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use windows_ioring_sys::{CompletionWake, IoBuf, IoBufMut};
use windows_threadpool_sys::callback_env::CallbackEnviron;

use super::WindowsBackend;
use crate::clock::StampClock;
use crate::contract::EpochId;
use crate::engine::Engine;

#[cfg(test)]
mod tests;

/// The engine a pool delivery drains.
pub(crate) type PoolEngine<E, B, C, K, R> = Engine<E, WindowsBackend<B, R>, C, K>;

/// The slot the engine sits in, shared between the delivery, its wake, and whatever else of the
/// driver's reaches it; empty once delivery has stopped.
pub(crate) type Shared<E, B, C, K, R> = Arc<Mutex<Option<PoolEngine<E, B, C, K, R>>>>;

/// A failed construction: why, and the engine it was given.
pub(crate) type PoolSetup<E, B, C, K, R> =
    Result<PoolDelivery<E, B, C, K, R>, (io::Error, Box<PoolEngine<E, B, C, K, R>>)>;

/// The engine, drained on a pool thread whenever the backend's ring says so.
pub(crate) struct PoolDelivery<E, B, C, K, R>
where
    E: EpochId + 'static,
    B: IoBuf,
    R: IoBufMut,
    K: StampClock,
{
    /// Declared before `engine`: dropping it stops the wait and waits for a running drain, so no
    /// drain touches the engine once it goes.
    wake: CompletionWake,
    engine: Shared<E, B, C, K, R>,
}

impl<E, B, C, K, R> PoolDelivery<E, B, C, K, R>
where
    E: EpochId + Send + Sync + 'static,
    B: IoBuf + Send + 'static,
    C: Send + 'static,
    K: StampClock,
    K::Instant: Send + Sync,
    R: IoBufMut + Send + 'static,
{
    /// Deliver `engine` on the thread pool -- `env`'s, or the process's default -- calling
    /// `on_ready` whenever a drain made entries poppable where there were none.
    ///
    /// # Errors
    ///
    /// The ring crate's, with the engine handed back: the system does not support a ring
    /// completion event, or the wait could not be made or armed.
    pub(crate) fn new(
        engine: PoolEngine<E, B, C, K, R>,
        on_ready: impl Fn() + Send + Sync + 'static,
        env: Option<&mut CallbackEnviron<'_>>,
    ) -> PoolSetup<E, B, C, K, R> {
        let engine = Arc::new(Mutex::new(Some(engine)));
        let drained = Arc::clone(&engine);
        let on_wake = move || {
            let deliver = lock(&drained).as_mut().is_some_and(Engine::drain);
            if deliver {
                on_ready();
            }
        };
        // Under the engine's lock, which lends the backend's ring to the wake; a drain the setup
        // signal starts waits for the lock until this returns.
        let woken = present(lock(&engine)).backend.wake(on_wake, env);
        match woken {
            Ok(wake) => Ok(Self { wake, engine }),
            Err(error) => {
                let engine = lock(&engine).take().expect("put there above");
                Err((error, Box::new(engine)))
            }
        }
    }
}

impl<E, B, C, K, R> PoolDelivery<E, B, C, K, R>
where
    E: EpochId + 'static,
    B: IoBuf,
    R: IoBufMut,
    K: StampClock,
{
    /// The engine, locked: what a consumer call acts through.
    pub(crate) fn engine(&self) -> EngineGuard<'_, PoolEngine<E, B, C, K, R>> {
        present(lock(&self.engine))
    }

    /// The engine's slot, for a path of the driver's own that reaches it on another thread.
    pub(crate) fn shared(&self) -> Shared<E, B, C, K, R> {
        Arc::clone(&self.engine)
    }

    /// Stop delivering, handing the engine back: the wake is stopped and any running drain waited
    /// for first, then the engine is taken out of its slot, so a shared path finds it empty.
    pub(crate) fn into_engine(self) -> PoolEngine<E, B, C, K, R> {
        let Self { wake, engine } = self;
        drop(wake);
        lock(&engine)
            .take()
            .expect("only stopping delivery takes the engine")
    }
}

/// Lock the engine's slot. A panic while it was held is an inconsistency the core already reported
/// and panicked on (DI-D-46); the state is left as that panic found it.
pub(crate) fn lock<T>(engine: &Mutex<T>) -> MutexGuard<'_, T> {
    engine.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The engine, locked, while delivery runs: its slot is full until delivery stops.
pub(crate) struct EngineGuard<'a, T>(MutexGuard<'a, Option<T>>);

fn present<T>(guard: MutexGuard<'_, Option<T>>) -> EngineGuard<'_, T> {
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
