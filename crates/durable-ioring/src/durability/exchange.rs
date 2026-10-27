// Copyright (c) 2026 Mike Grier
// Split from durability.rs at 0a71d4f7.
//! What the durability core and the I/O core pass each other: how a write ended, coming in, and
//! what a change made due -- flushes, entries, releases and provider requests -- going out.

use super::{Gate, Key, V};
use crate::clock::Instant;
use crate::contract::EpochId;
use crate::domains::DomainId;
use crate::ids::{FailureId, InstanceId, Lineage, OpId};
#[cfg(test)]
use crate::types::SuspectWrite;
use crate::types::{Entry, Epoch, Failed, IoFault, IoTargetKey};
use alloc::vec::Vec;

/// A failure's suspect writes as they stood when an event about it was made, for a test to
/// compare: the entries carry none (DI-D-108).
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct Snapshot<E: EpochId + 'static, I: Instant> {
    pub(crate) suspect: Vec<SuspectWrite<V<E, I>>>,
}

/// A flush now due: `lineage`'s seal `through`'s flush of `file`, through `target`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Flush<E, T> {
    pub(crate) lineage: Key,
    pub(crate) through: E,
    pub(crate) file: IoTargetKey,
    pub(crate) target: T,
}

/// An entry a change made due, in the core's terms.
#[derive(Debug)]
pub(crate) enum Event<E: EpochId + 'static, I: Instant> {
    Durable {
        lineage: Key,
        through: E,
    },
    Blocked {
        lineage: Key,
        through: E,
        by: FailureId,
    },
    Failed {
        failed: Failed<V<E, I>>,
        #[cfg(test)]
        snapshot: Snapshot<E, I>,
    },
    Abandoned {
        failure: FailureId,
        #[cfg(test)]
        snapshot: Snapshot<E, I>,
    },
    Healed {
        failure: FailureId,
        #[cfg(test)]
        snapshot: Snapshot<E, I>,
    },
    /// An empty seal answered `Blocked` was replaced by a later one, which takes over its request:
    /// its un-popped `Blocked` is withdrawn (DI-D-112).
    Superseded {
        lineage: Key,
        through: E,
    },
    LineageEnded {
        lineage: Key,
        abandoned_through: Option<E>,
    },
    /// A held operation will never be issued: the epoch named was abandoned -- its gate, or its own
    /// (DI-D-43).
    NeverIssued {
        op: OpId,
        abandoned: Gate<E>,
    },
}

impl<E: EpochId + 'static, I: Instant> Event<E, I> {
    /// The entry this event is, for `instance`'s queue, or the event handed back when it is not one
    /// on its own: an operation never issued, which only the holder of the operation can make an
    /// entry of; and a seal superseded, which withdraws an entry.
    pub(crate) fn into_entry<B, C>(self, instance: InstanceId) -> Arrival<E, I, B, C> {
        let name = |seq| Lineage { instance, seq };
        Arrival::Entry(match self {
            Event::Durable { lineage, through } => Entry::Durable {
                through: Epoch::new(name(lineage), through),
            },
            Event::Blocked {
                lineage,
                through,
                by,
            } => Entry::Blocked {
                through: Epoch::new(name(lineage), through),
                by,
            },
            Event::Failed { failed, .. } => Entry::Failed(failed),
            Event::Abandoned { failure, .. } => Entry::Abandoned { failure },
            Event::Healed { failure, .. } => Entry::Healed { failure },
            Event::LineageEnded {
                lineage,
                abandoned_through,
            } => Entry::LineageEnded {
                lineage: name(lineage),
                abandoned_through,
            },
            event @ (Event::NeverIssued { .. } | Event::Superseded { .. }) => {
                return Arrival::Other(event);
            }
        })
    }
}

/// What an event becomes on the I/O core's queue.
pub(crate) enum Arrival<E: EpochId + 'static, I: Instant, B, C> {
    /// An entry, as it is.
    Entry(Entry<V<E, I>, B, C>),
    /// An event the I/O core acts on itself.
    Other(Event<E, I>),
}

/// How a write ended, as the core needs it. A target never completes a write short (DI-D-50), so a
/// write either succeeded in full or failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WriteEnd {
    /// Completed in full.
    Succeeded,
    /// Completed with a fault, as the backend reported it.
    Failed(IoFault),
}

/// What a change made due.
#[derive(Debug)]
pub(crate) struct Due<E: EpochId + 'static, T, I: Instant> {
    /// Flushes to push.
    pub(crate) flushes: Vec<Flush<E, T>>,
    /// Entries to append, in the order they happened.
    pub(crate) events: Vec<Event<E, I>>,
    /// Held operations whose gates are now durable, to push, in the order they were released.
    pub(crate) released: Vec<OpId>,
    /// Requests for the consumer's provider, one per seal that needs it, in the order made.
    pub(crate) calls: Vec<ProviderCall>,
}

impl<E: EpochId + 'static, T, I: Instant> Default for Due<E, T, I> {
    fn default() -> Self {
        Self {
            flushes: Vec::new(),
            events: Vec::new(),
            released: Vec::new(),
            calls: Vec::new(),
        }
    }
}

/// One seal's request for the consumer's provider (DI-D-27), in the core's terms: per domain the
/// provider serves that the seal touched, each file's writes named for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProviderCall {
    pub(crate) lineage: Key,
    /// The seal's sequence number, which its answers name.
    pub(crate) seal: u64,
    /// By domain, ascending; within one, files in the order first written.
    pub(crate) domains: Vec<DomainPart>,
}

/// One domain's part of a provider request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DomainPart {
    pub(crate) domain: DomainId,
    /// Each file, with the writes named for this domain, in push order.
    pub(crate) files: Vec<(IoTargetKey, Vec<OpId>)>,
}
