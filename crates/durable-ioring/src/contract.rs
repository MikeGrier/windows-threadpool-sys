// Copyright (c) 2026 Mike Grier
//! The contract as traits (DI-2.15, DI-D-24 in DESIGN-NOTES.md).
//!
//! Everything in [`crate::types`] that is generic over `V: Identities` is shared by every
//! implementation; the identity types themselves are each implementation's own. The trait grows
//! as the implementation does: it carries only the operations dioring implements, never a
//! method that cannot yet be called.

use std::fmt::Debug;
use std::hash::Hash;

use crate::types::{Entry, LineageInfo, PushError};

#[cfg(test)]
mod tests;

/// What dioring requires of an epoch id, the ordered value within a lineage that names an epoch.
/// The consumer chooses the type -- a `u64` (the default), an `Lsn` newtype, a
/// `(segment, offset)` pair -- and dioring only ever compares and stores it. "Id" rather than
/// "number" because it need not be one.
///
/// Two requirements the compiler cannot check, stated in the contract:
/// - `Ord` is a total order that stays consistent for the life of the instance;
/// - values never wrap: a sequence that compares with wraparound (as jbd2's 32-bit transaction
///   ids do) is not a total order, and must be mapped onto one that does not wrap.
pub trait EpochId: Copy + Ord + Debug {}

impl<T: Copy + Ord + Debug> EpochId for T {}

/// The identity types an implementation mints, and the consumer's epoch-id type (DI-D-24). Each
/// identity carries its implementation's private provenance, so only that implementation can
/// make one, and another instance's is refused. A zero-sized marker implements this; the shared
/// types are generic over it, so a consumer names one parameter, `D::Ids`, not five.
pub trait Identities: Copy + Debug + Eq + Hash + 'static {
    /// The consumer's epoch-id type.
    type EpochId: EpochId;
    /// A durability lineage (DI-D-19).
    type Lineage: Copy + Debug + Eq + Hash;
    /// One pushed operation. Ordered by push order within an instance; `None` across instances.
    type OpId: Copy + Debug + Eq + Hash + PartialOrd;
    /// A durability failure's `Copy` identity, used in events and queries.
    type FailureId: Copy + Debug + Eq + Hash;
    /// The only thing that can resolve a failure: affine, move-only, and dropping it is closing
    /// it (DI-D-12).
    type FailureToken: Debug;

    /// The failure a token resolves.
    fn token_id(token: &Self::FailureToken) -> Self::FailureId;
}

/// The contract: an instance that takes tagged writes and reports durability by epoch.
///
/// Generic only -- consumers write `D: DurableRing` -- and construction stays on each
/// implementation (DI-D-24).
pub trait DurableRing {
    /// The implementation's identity types, and the consumer's epoch-id type.
    type Ids: Identities;
    /// The owned buffer type of writes and reads.
    type Buffer;
    /// The consumer's per-operation context, handed back with each completion.
    type Context;

    /// The lineage that always exists.
    fn default_lineage(&self) -> Lin<Self>;

    /// Every live lineage, the default among them.
    fn lineages(&self) -> Vec<LineageInfo<Self::Ids>>;
}

/// An implementation's lineage type.
pub type Lin<D> = <<D as DurableRing>::Ids as Identities>::Lineage;
/// An implementation's operation identity.
pub type OpIdOf<D> = <<D as DurableRing>::Ids as Identities>::OpId;
/// An implementation's failure identity.
pub type FailureIdOf<D> = <<D as DurableRing>::Ids as Identities>::FailureId;
/// An implementation's failure token.
pub type TokenOf<D> = <<D as DurableRing>::Ids as Identities>::FailureToken;
/// An implementation's epoch-id type.
pub type EpochIdOf<D> = <<D as DurableRing>::Ids as Identities>::EpochId;
/// An implementation's queue entry.
pub type EntryOf<D> =
    Entry<<D as DurableRing>::Ids, <D as DurableRing>::Buffer, <D as DurableRing>::Context>;
/// What an implementation's push returns.
pub type PushResult<D> = Result<
    OpIdOf<D>,
    PushError<<D as DurableRing>::Ids, <D as DurableRing>::Buffer, <D as DurableRing>::Context>,
>;
