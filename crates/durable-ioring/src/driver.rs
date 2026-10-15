// Copyright (c) 2026 Mike Grier
//! What a driver builds on: the engine that joins a core to its backend under one lock, the pump a
//! consumer's own thread drains, and the few helpers the core's own bookkeeping shares with a
//! driver (DI-D-50, DI-D-53).
//!
//! **This is not the consumer's surface.** A consumer holds a [`DurableRing`](crate::DurableRing),
//! which a driver implements. A driver for another platform than Windows builds on these, as
//! `win-durable-ioring` does, so they are public and are part of what a driver may rely on; the
//! rules each keeps are in the item's own documentation.

pub use crate::domains::first_repeat;
pub use crate::durability::inconsistent;
pub use crate::engine::{Engine, EngineGuard, Slot, lock, present};
pub use crate::ids::{Home, InstanceId};
pub use crate::provider::{Answer, DomainCall};
pub use crate::pump::{Pumped, PumpedDelivery, PumpedSetup};
