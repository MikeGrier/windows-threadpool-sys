// Copyright (c) 2026 Mike Grier
//! Signalled delivery (DI-7.5, [DI-D-55](../../DESIGN-NOTES.md#di-d-55)): the thread pool tells the
//! consumer, through an event of the instance's own, that completions are waiting, and the consumer
//! drains with `pump`.
//!
//! The ring crate's `CompletionWake` owns the ring's completion event: a thread-pool wait, which
//! holds no thread while it waits. Its callback sets one auto-reset [`Event`] and does nothing else,
//! holding neither the engine nor the ring, so it takes no lock. The consumer is handed duplicates
//! of that event to wait on beside its own handles; nothing is shared with the ring, so it may wait
//! on it, set it or reset it, and the worst that costs is its own wakeup.
//!
//! A pump in this mode waits on that same event, never the ring's, which the wake owns (the ring
//! crate's D-21). It drains before it waits, so an edge a consumer's own wait took cannot leave it
//! blocked while completions sit in the ring.

use std::io;
use std::os::windows::io::AsHandle;
use std::time::Duration;

use win_sync_sys::{Event, ResetMode};
use windows_ioring_sys::{CompletionWake, IoBuf, IoBufMut};
use windows_threadpool_sys::callback_env::CallbackEnviron;

use super::WindowsBackend;
use super::pool::{PoolEngine, Shared};
use durable_ioring::StampClock;

use super::CompletionEvent;
use durable_ioring::EpochId;
use durable_ioring::driver::EngineGuard;
use durable_ioring::driver::{Pumped, PumpedDelivery};

/// A failed construction: why, and the engine it was given.
pub(crate) type SignalledSetup<E, B, C, K, R> =
    Result<SignalledDelivery<E, B, C, K, R>, (io::Error, Box<PoolEngine<E, B, C, K, R>>)>;

/// The engine, drained by the consumer's own calls to [`pump`](Self::pump), which the pool tells
/// through [`signal`](Self::signal).
pub(crate) struct SignalledDelivery<E, B, C, K, R>
where
    E: EpochId + 'static,
    B: IoBuf,
    R: IoBufMut,
    K: StampClock,
{
    /// Declared first: dropping it stops the wait and waits for a running callback, which holds
    /// only a duplicate of `signal`.
    wake: CompletionWake,
    pumped: PumpedDelivery<E, WindowsBackend<B, R>, C, K>,
    signal: Event,
}

impl<E, B, C, K, R> SignalledDelivery<E, B, C, K, R>
where
    E: EpochId + 'static,
    B: IoBuf,
    R: IoBufMut,
    K: StampClock,
{
    /// Deliver `engine` to whoever pumps, telling them on the thread pool -- `env`'s, or the
    /// process's default -- through the event [`signal`](Self::signal) hands out. The event is set
    /// once as the wake is made, as the ring crate raises its setup signal, and again whenever the
    /// ring's completions may have gone from none to some.
    ///
    /// # Errors
    ///
    /// With the engine handed back: the event could not be made, or duplicated for the notification
    /// that owns its handle, or the ring crate's
    /// wake could not be made or armed, as `CompletionWake::new`.
    pub(crate) fn new(
        mut engine: PoolEngine<E, B, C, K, R>,
        env: Option<&mut CallbackEnviron<'_>>,
    ) -> SignalledSetup<E, B, C, K, R> {
        let made = Event::new(ResetMode::Auto, false).and_then(|signal| {
            let waker = signal.clone();
            // A `CompletionEvent` owns its handle outright, so it is given a duplicate of its own;
            // the waker and the signal handed out share the event's one handle.
            let notification = CompletionEvent::new(signal.as_handle().try_clone_to_owned()?);
            Ok((signal, waker, notification))
        });
        let (signal, waker, notification) = match made {
            Ok(made) => made,
            Err(error) => return Err((error, Box::new(engine))),
        };
        let on_wake = move || {
            // Setting an event this instance created, with full access, does not fail.
            let set = waker.set();
            debug_assert!(set.is_ok(), "setting the completion signal failed: {set:?}");
        };
        let wake = match engine.backend.wake(on_wake, env) {
            Ok(wake) => wake,
            Err(error) => return Err((error, Box::new(engine))),
        };
        Ok(Self {
            wake,
            pumped: PumpedDelivery::with_notification(engine, notification),
            signal,
        })
    }

    /// A clone of the event the pool sets, sharing its handle, for the consumer to wait on.
    ///
    /// # Errors
    ///
    /// None: a clone makes no handle. The `Result` is the shape of the call it answers.
    pub(crate) fn signal(&self) -> io::Result<Event> {
        Ok(self.signal.clone())
    }

    /// As [`PumpedDelivery::pump`], waiting on the signal.
    pub(crate) fn pump(&mut self, timeout: Option<Duration>) -> Pumped {
        self.pumped.pump(timeout)
    }

    /// The engine, locked: what a consumer call acts through.
    pub(crate) fn engine(&self) -> EngineGuard<'_, PoolEngine<E, B, C, K, R>> {
        self.pumped.engine()
    }

    /// The engine's slot, for a path of the driver's own that reaches it on another thread.
    pub(crate) fn shared(&self) -> Shared<E, B, C, K, R> {
        self.pumped.shared()
    }

    /// Stop delivering, handing the engine back: the wake is stopped and a running callback
    /// waited for first, then the engine is taken out of its slot.
    pub(crate) fn into_engine(self) -> PoolEngine<E, B, C, K, R> {
        let Self { wake, pumped, .. } = self;
        drop(wake);
        pumped.into_engine()
    }
}
