// Copyright (c) 2026 Mike Grier
//! How an instance's engine is delivered (DI-D-52, [DI-D-55](../../DESIGN-NOTES.md#di-d-55)): on
//! the thread pool, to whoever pumps, or to whoever pumps once the pool has said so. Everything the
//! driver does with the engine -- consumer calls, a provider's answer, a lineage handle's release,
//! ending -- reaches it the same way under any, through the slot it sits in.

use std::io;
use std::time::Duration;

use win_sync_sys::Event;
use windows_ioring_sys::{IoBuf, IoBufMut};

use crate::clock::StampClock;
use crate::contract::EpochId;
use crate::engine::EngineGuard;
use crate::pump::{Pumped, PumpedDelivery};
use crate::windows_backend::WindowsBackend;
use crate::windows_backend::pool::{PoolDelivery, PoolEngine, Shared};
use crate::windows_backend::signalled::SignalledDelivery;

/// The engine's delivery, as an instance was built with it.
pub(crate) enum Delivery<E, B, C, K, R>
where
    E: EpochId + 'static,
    B: IoBuf,
    R: IoBufMut,
    K: StampClock,
{
    /// Drained on a pool thread whenever the ring's completion event says to.
    Pool(PoolDelivery<E, B, C, K, R>),
    /// Drained by the consumer's own thread, in [`Dioring::pump`](super::Dioring::pump), which waits
    /// on the ring's completion event.
    Pumped(PumpedDelivery<E, WindowsBackend<B, R>, C, K>),
    /// Drained by the consumer's own thread, which the pool tells through an event of the
    /// instance's own.
    Signalled(SignalledDelivery<E, B, C, K, R>),
}

impl<E, B, C, K, R> Delivery<E, B, C, K, R>
where
    E: EpochId + 'static,
    B: IoBuf,
    R: IoBufMut,
    K: StampClock,
{
    /// The engine, locked: what a consumer call acts through.
    pub(crate) fn engine(&self) -> EngineGuard<'_, PoolEngine<E, B, C, K, R>> {
        match self {
            Self::Pool(pool) => pool.engine(),
            Self::Pumped(pumped) => pumped.engine(),
            Self::Signalled(signalled) => signalled.engine(),
        }
    }

    /// The engine's slot, for a path of the driver's own that reaches it on another thread.
    pub(crate) fn shared(&self) -> Shared<E, B, C, K, R> {
        match self {
            Self::Pool(pool) => pool.shared(),
            Self::Pumped(pumped) => pumped.shared(),
            Self::Signalled(signalled) => signalled.shared(),
        }
    }

    /// Stop delivering, handing the engine back; a pool or signalled delivery waits for a drain or
    /// a callback in progress first.
    pub(crate) fn into_engine(self) -> PoolEngine<E, B, C, K, R> {
        match self {
            Self::Pool(pool) => pool.into_engine(),
            Self::Pumped(pumped) => pumped.into_engine(),
            Self::Signalled(signalled) => signalled.into_engine(),
        }
    }

    /// Pump, if the instance is drained by its consumer; `None` for a pool, which drains itself.
    pub(crate) fn pump(&mut self, timeout: Option<Duration>) -> Option<Pumped> {
        match self {
            Self::Pool(_) => None,
            Self::Pumped(pumped) => Some(pumped.pump(timeout)),
            Self::Signalled(signalled) => Some(signalled.pump(timeout)),
        }
    }

    /// A duplicate of the event the pool sets, if the instance has one: `None` unless signalled.
    pub(crate) fn signal(&self) -> Option<io::Result<Event>> {
        match self {
            Self::Signalled(signalled) => Some(signalled.signal()),
            Self::Pool(_) | Self::Pumped(_) => None,
        }
    }

    /// Whether this is pumped delivery, waiting on the ring's own event.
    #[cfg(test)]
    pub(crate) fn is_pumped(&self) -> bool {
        matches!(self, Self::Pumped(_))
    }

    /// Whether this is signalled delivery.
    #[cfg(test)]
    pub(crate) fn is_signalled(&self) -> bool {
        matches!(self, Self::Signalled(_))
    }
}
