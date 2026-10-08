// Copyright (c) 2026 Mike Grier
//! dioring's identity types, and the marker that names them.

use std::cmp::Ordering;
use std::fmt::{self, Debug};
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};

use crate::contract::{EpochId, Identities};

#[cfg(test)]
mod tests;

/// Which instance minted a value. Private: it exists so foreign values can be refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct InstanceId(u64);

impl InstanceId {
    /// A value no other instance in this process has had. Instances are counted, never reused,
    /// so a value an instance minted is never mistaken for one a later instance minted.
    pub(crate) fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, AtomicOrdering::Relaxed))
    }
}

/// dioring's lineage: minted at any time, never reused, including after retirement. Carries its
/// instance, so another instance's lineage is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Lineage {
    pub(crate) instance: InstanceId,
    pub(crate) seq: u64,
}

/// dioring's identity for one pushed operation. Ordered by push order within an instance;
/// identities from two instances are incomparable (`partial_cmp` is `None`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct OpId {
    pub(crate) instance: InstanceId,
    pub(crate) seq: u64,
}

impl PartialOrd for OpId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        (self.instance == other.instance).then(|| self.seq.cmp(&other.seq))
    }
}

/// dioring's failure identity: `Copy`, never reused within an instance. It cannot resolve
/// anything.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FailureId {
    pub(crate) instance: InstanceId,
    pub(crate) seq: u64,
}

/// dioring's failure token: the one thing that resolves its failure. At most one is live per
/// failure (DI-D-12). Resolving consumes it; dropping it is [`close`](Self::close).
#[must_use = "an unresolved failure stalls the high-water mark; heal, abandon, or close it"]
#[derive(Debug)]
pub struct FailureToken {
    pub(crate) id: FailureId,
    /// Shared with the instance's record of the failure, which reads it to know whether a token is
    /// live and so whether the inventory may hand out another.
    pub(crate) live: Arc<AtomicBool>,
}

impl FailureToken {
    /// A live token for `id`, and the flag its drop clears.
    pub(crate) fn mint(id: FailureId) -> (Self, Arc<AtomicBool>) {
        let live = Arc::new(AtomicBool::new(true));
        (
            Self {
                id,
                live: Arc::clone(&live),
            },
            live,
        )
    }

    /// The failure this token resolves.
    pub fn id(&self) -> FailureId {
        self.id
    }

    /// Set the failure aside, unresolved: it stays in the inventory, which can hand out its token
    /// again (DI-D-12 (c)). The same as dropping the token.
    pub fn close(self) {}
}

impl Drop for FailureToken {
    fn drop(&mut self) {
        self.live.store(false, AtomicOrdering::Release);
    }
}

/// dioring's identities, with the consumer's epoch-id type `E`. Zero-sized; it exists only to
/// name them, so its comparison and hashing traits hold for every `E`.
pub struct DioringIds<E>(PhantomData<fn() -> E>);

impl<E> Clone for DioringIds<E> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<E> Copy for DioringIds<E> {}

impl<E> Debug for DioringIds<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DioringIds")
    }
}

impl<E> PartialEq for DioringIds<E> {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl<E> Eq for DioringIds<E> {}

impl<E> Hash for DioringIds<E> {
    fn hash<H: Hasher>(&self, _: &mut H) {}
}

impl<E: EpochId + 'static> Identities for DioringIds<E> {
    type EpochId = E;
    type Lineage = Lineage;
    type OpId = OpId;
    type FailureId = FailureId;
    type FailureToken = FailureToken;

    fn token_id(token: &FailureToken) -> FailureId {
        token.id()
    }
}
