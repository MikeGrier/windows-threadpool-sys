// Copyright (c) 2026 Mike Grier
//! How an instance's engine is delivered (DI-D-52, [DI-D-55](../../DESIGN-NOTES.md#di-d-55)): on
//! the thread pool, or to whoever pumps. Everything the driver does with the engine -- consumer
//! calls, a provider's answer, a lineage handle's release, ending -- reaches it the same way under
//! either, through the slot it sits in.

use crate::clock::StampClock;
use crate::contract::EpochId;
use crate::engine::EngineGuard;
use crate::pump::PumpedDelivery;
use crate::windows_backend::WindowsBackend;
use crate::windows_backend::pool::{PoolDelivery, PoolEngine, Shared};
use windows_ioring_sys::{IoBuf, IoBufMut};

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
    /// Drained by the consumer's own thread, in [`Dioring::pump`](super::Dioring::pump).
    Pumped(PumpedDelivery<E, WindowsBackend<B, R>, C, K>),
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
        }
    }

    /// The engine's slot, for a path of the driver's own that reaches it on another thread.
    pub(crate) fn shared(&self) -> Shared<E, B, C, K, R> {
        match self {
            Self::Pool(pool) => pool.shared(),
            Self::Pumped(pumped) => pumped.shared(),
        }
    }

    /// Stop delivering, handing the engine back; a pool delivery waits for a drain in progress
    /// first.
    pub(crate) fn into_engine(self) -> PoolEngine<E, B, C, K, R> {
        match self {
            Self::Pool(pool) => pool.into_engine(),
            Self::Pumped(pumped) => pumped.into_engine(),
        }
    }

    /// Whether this is pumped delivery.
    #[cfg(test)]
    pub(crate) fn is_pumped(&self) -> bool {
        matches!(self, Self::Pumped(_))
    }
}
