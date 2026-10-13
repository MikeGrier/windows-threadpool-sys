// Copyright (c) 2026 Mike Grier
//! dioring's identity types, and the marker that names them.

use std::cmp::Ordering;
use std::fmt::{self, Debug};
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, Weak};

use crate::clock::Instant;
use crate::contract::{EpochId, Identities};
use crate::types::Tag;

#[cfg(test)]
mod tests;

/// Which instance minted a value: it exists so foreign values can be refused. A driver takes one
/// for each instance it builds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct InstanceId(u64);

impl InstanceId {
    /// A value no other instance in this process has had. Instances are counted, never reused,
    /// so a value an instance minted is never mistaken for one a later instance minted.
    #[must_use]
    pub fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, AtomicOrdering::Relaxed))
    }

    /// The number it was minted with, for a diagnostic to name it by.
    #[must_use]
    pub const fn number(self) -> u64 {
        self.0
    }
}

/// dioring's lineage: minted at any time, never reused, including after retirement. Carries its
/// instance, so another instance's lineage is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Lineage {
    /// The instance that minted it.
    pub instance: InstanceId,
    /// Its place in that instance's lineages: the default lineage is 0.
    pub seq: u64,
}

/// What a lineage's handle reaches when its last copy is released: the instance that minted it.
pub trait Home: Send + Sync {
    /// The last handle of `lineage` was released: end the lineage (DI-D-30, DI-D-40).
    fn release(&self, lineage: Lineage);
}

/// A home that never exists, for a handle whose instance keeps nothing to end.
struct Nowhere;

impl Home for Nowhere {
    fn release(&self, _: Lineage) {}
}

/// What every copy of one lineage's handle shares.
struct Held {
    lineage: Lineage,
    home: Weak<dyn Home>,
    /// Cleared when the lineage is ended or retired through its last handle, so releasing that
    /// handle ends nothing a second time.
    armed: bool,
}

impl Drop for Held {
    fn drop(&mut self) {
        if self.armed
            && let Some(home) = self.home.upgrade()
        {
            home.release(self.lineage);
        }
    }
}

/// dioring's handle to a live lineage (DI-D-40): what pushing into the lineage and sealing it
/// take. Cloning it is cheap, and every copy names the same lineage.
///
/// **Releasing the last copy ends the lineage** (DI-D-30): every epoch of it not yet durable is
/// abandoned, and a `LineageEnded` entry reports it. `#[must_use]` warns when a returned handle is
/// discarded unused, but not for `let _ = ...`, which releases it at once, nor for a handle
/// dropped with whatever held it -- so keep a copy for as long as the lineage's work matters. The
/// default lineage never ends this way: the instance holds a copy of its own.
#[must_use = "releasing a lineage's last handle ends the lineage, abandoning every epoch of it not yet durable"]
#[derive(Clone)]
pub struct LineageHandle(Arc<Held>);

impl LineageHandle {
    /// A handle to `lineage`, whose last release ends it through `home`.
    pub fn mint(lineage: Lineage, home: Weak<dyn Home>) -> Self {
        Self(Arc::new(Held {
            lineage,
            home,
            armed: true,
        }))
    }

    /// A handle to `lineage` that ends nothing when released: for a lineage whose instance keeps
    /// nothing to end, such as a fake's one lineage.
    pub fn detached(lineage: Lineage) -> Self {
        Self::mint(lineage, Weak::<Nowhere>::new())
    }

    /// The lineage, by its `Copy` name: what entries, reports and a consumer's own records use.
    pub fn lineage(&self) -> Lineage {
        self.0.lineage
    }

    /// Epoch `id` of this lineage, as a write or a seal takes it.
    pub fn at<E: EpochId + 'static, I: Instant>(&self, id: E) -> Tag<'_, DioringIds<E, I>> {
        Tag::new(self, id)
    }

    /// Whether this is the lineage's only remaining copy.
    #[must_use]
    pub fn is_last(&self) -> bool {
        Arc::strong_count(&self.0) == 1
    }

    /// Consume the lineage's last handle without ending the lineage through it, because the caller
    /// is ending or retiring it explicitly. Hands the handle back if another copy exists.
    pub fn take_last(self) -> Result<Lineage, Self> {
        match Arc::try_unwrap(self.0) {
            Ok(mut held) => {
                held.armed = false;
                Ok(held.lineage)
            }
            Err(shared) => Err(Self(shared)),
        }
    }
}

impl Debug for LineageHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("LineageHandle")
            .field(&self.0.lineage)
            .finish()
    }
}

/// dioring's identity for one pushed operation. Ordered by push order within an instance;
/// identities from two instances are incomparable (`partial_cmp` is `None`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct OpId {
    /// The instance that pushed it.
    pub instance: InstanceId,
    /// Its place in that instance's push order.
    pub seq: u64,
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

/// dioring's identities, with the consumer's epoch-id type `E` and the time type `I` its stamps
/// are read in (DI-D-50): the clock the core was given decides it, and each driver names its own
/// (`win-durable-ioring`'s is `WindowsIds`). Zero-sized; it exists only to name them, so its
/// comparison and hashing traits hold for every `E` and `I`.
pub struct DioringIds<E, I>(PhantomData<fn() -> (E, I)>);

impl<E, I> Clone for DioringIds<E, I> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<E, I> Copy for DioringIds<E, I> {}

impl<E, I> Debug for DioringIds<E, I> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DioringIds")
    }
}

impl<E, I> PartialEq for DioringIds<E, I> {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl<E, I> Eq for DioringIds<E, I> {}

impl<E, I> Hash for DioringIds<E, I> {
    fn hash<H: Hasher>(&self, _: &mut H) {}
}

impl<E: EpochId + 'static, I: Instant> Identities for DioringIds<E, I> {
    type EpochId = E;
    type Lineage = Lineage;
    type OpId = OpId;
    type FailureId = FailureId;
    type FailureToken = FailureToken;
    type LineageHandle = LineageHandle;
    type Instant = I;

    fn token_id(token: &FailureToken) -> FailureId {
        token.id()
    }

    fn handle_lineage(handle: &LineageHandle) -> Lineage {
        handle.lineage()
    }
}
