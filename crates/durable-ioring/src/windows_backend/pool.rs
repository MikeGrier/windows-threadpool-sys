// Copyright (c) 2026 Mike Grier
//! Pool delivery (DI-D-52's Model A, DI-D-53): the ring crate's `CompletionWake` over the Windows
//! backend's ring, draining the driver's [`Engine`] whenever the thread pool says to.
//!
//! The wake holds no lock and no ring; woken, this takes the engine's lock, drains the backend to
//! empty through the core, releases the lock, and only then tells the driver that entries became
//! poppable -- so they are poppable before it is told (DI-D-28), and the driver's own code never
//! runs under the engine's lock. Two wakes may overlap (the ring crate's D-83); the engine's lock
//! serializes them.

// Until the driver moves onto pool delivery in `DI-3.10.3.5.6.2`, only tests reach it. An
// expectation rather than an allowance, so the build fails once the driver uses it and this goes.
#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the driver moves onto pool delivery in DI-3.10.3.5.6.2"
    )
)]

use std::io;
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
type PoolEngine<E, B, C, K, R> = Engine<E, WindowsBackend<B, R>, C, K>;

/// The engine, shared between the delivery and its wake.
type Shared<E, B, C, K, R> = Arc<Mutex<PoolEngine<E, B, C, K, R>>>;

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
        let engine = Arc::new(Mutex::new(engine));
        let drained = Arc::clone(&engine);
        let on_wake = move || {
            let deliver = lock(&drained).drain();
            if deliver {
                on_ready();
            }
        };
        // Under the engine's lock, which lends the backend's ring to the wake; a drain the setup
        // signal starts waits for the lock until this returns.
        let woken = lock(&engine).backend.wake(on_wake, env);
        match woken {
            Ok(wake) => Ok(Self { wake, engine }),
            Err(error) => {
                // The wake was never built, so its closure -- the only other reference -- is gone.
                let engine = Arc::try_unwrap(engine)
                    .ok()
                    .expect("a wake that was never built holds no reference to the engine")
                    .into_inner()
                    .unwrap_or_else(PoisonError::into_inner);
                Err((error, Box::new(engine)))
            }
        }
    }

    /// The engine, locked: what a consumer call acts through.
    pub(crate) fn engine(&self) -> MutexGuard<'_, PoolEngine<E, B, C, K, R>> {
        lock(&self.engine)
    }

    /// Stop delivering, handing the engine back: the wake is stopped and any running drain waited
    /// for first, so nothing else holds the engine.
    pub(crate) fn into_engine(self) -> PoolEngine<E, B, C, K, R> {
        let Self { wake, engine } = self;
        drop(wake);
        Arc::try_unwrap(engine)
            .ok()
            .expect("a stopped wake holds no reference to the engine")
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// Lock the engine. A panic while it was held is an inconsistency the core already reported and
/// panicked on (DI-D-46); the state is left as that panic found it.
fn lock<T>(engine: &Mutex<T>) -> MutexGuard<'_, T> {
    engine.lock().unwrap_or_else(PoisonError::into_inner)
}
