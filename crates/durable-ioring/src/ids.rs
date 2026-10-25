// Copyright (c) 2026 Mike Grier
//! dioring's identity types, and the marker that names them.

use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::cmp::Ordering;
use core::fmt::{self, Debug};
use core::hash::{Hash, Hasher};
use core::marker::PhantomData;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering as AtomicOrdering, fence};

use crate::alloc_tags::{tag, tagged};
use crate::capacity::{ConstructionError, list};
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

/// What a lineage's handle reaches: the instance that minted it, which keeps the handles' copy
/// counts and ends a lineage when the last copy is released.
pub trait Home: Send + Sync {
    /// The last handle of `lineage` was released: end the lineage (DI-D-30, DI-D-40).
    fn release(&self, lineage: Lineage);

    /// The copy counts of the handles this home mints (DI-D-108).
    fn slots(&self) -> &LineageSlots;
}

/// A home that never exists, for a handle whose instance keeps nothing to end. A `Weak` of it never
/// upgrades, so neither method is reached.
struct Nowhere;

/// The slots of a home that never exists: none.
static NO_SLOTS: LineageSlots = LineageSlots { slots: Vec::new() };

impl Home for Nowhere {
    fn release(&self, _: Lineage) {}

    fn slots(&self) -> &LineageSlots {
        &NO_SLOTS
    }
}

/// One lineage's copy count.
struct LineageSlot {
    /// The number of the lineage holding it: the generation a handle's lineage must match.
    lineage: AtomicU64,
    /// Its handles' copies; zero when the slot is free.
    copies: AtomicUsize,
}

/// The copy counts of an instance's lineage handles (DI-D-108): one slot for each lineage it can
/// hold, made when it is built, so minting, cloning and releasing a handle allocate nothing.
///
/// A slot is claimed when a handle is minted and freed when its last copy goes. A lineage with a
/// copy out has not been ended or retired through its handles, so the instance still holds it;
/// the slots in use are therefore at most the lineages the instance holds, and one is free whenever
/// the instance admits a mint under [`Capacity::lineages`](crate::Capacity::lineages).
pub struct LineageSlots {
    slots: Vec<LineageSlot>,
}

impl LineageSlots {
    /// A slot for each of `lineages` lineages.
    ///
    /// # Errors
    ///
    /// [`ConstructionError::Table`] when the slots cannot be allocated.
    pub fn new(lineages: usize) -> Result<Self, ConstructionError> {
        let mut slots = list("the lineage handles' slots", Some(lineages))?;
        slots.extend((0..lineages).map(|_| LineageSlot {
            lineage: AtomicU64::new(0),
            copies: AtomicUsize::new(0),
        }));
        Ok(Self { slots })
    }

    /// Claim a free slot for `lineage`, holding one copy: `None` when every slot is in use.
    fn claim(&self, lineage: Lineage) -> Option<u32> {
        self.slots.iter().enumerate().find_map(|(at, slot)| {
            slot.copies
                .compare_exchange(0, 1, AtomicOrdering::Acquire, AtomicOrdering::Relaxed)
                .ok()?;
            slot.lineage.store(lineage.seq, AtomicOrdering::Release);
            u32::try_from(at).ok()
        })
    }

    /// Slot `at`, if it is held by `lineage`'s handles.
    fn held(&self, at: u32, lineage: Lineage) -> Option<&LineageSlot> {
        let slot = self.slots.get(usize::try_from(at).ok()?)?;
        (slot.lineage.load(AtomicOrdering::Acquire) == lineage.seq).then_some(slot)
    }

    /// How many slots hold a copy.
    #[must_use]
    pub fn in_use(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| slot.copies.load(AtomicOrdering::Acquire) > 0)
            .count()
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
///
/// A handle is its home, its lineage, and the index of the slot in the home's [`LineageSlots`]
/// that counts its copies (DI-D-108); the lineage's number is the slot's generation. A handle whose
/// home is gone, or never existed, counts nothing: it is never the last, and ends nothing.
#[must_use = "releasing a lineage's last handle ends the lineage, abandoning every epoch of it not yet durable"]
pub struct LineageHandle {
    home: Weak<dyn Home>,
    lineage: Lineage,
    slot: u32,
}

impl LineageHandle {
    /// A handle to `lineage`, holding a slot of `home`'s, whose last release ends it through
    /// `home`: `None` when `home` is gone or every one of its slots is in use.
    pub fn mint(lineage: Lineage, home: Weak<dyn Home>) -> Option<Self> {
        let slot = home.upgrade()?.slots().claim(lineage)?;
        Some(Self {
            home,
            lineage,
            slot,
        })
    }

    /// A handle to `lineage` that ends nothing when released: for a lineage whose instance keeps
    /// nothing to end, such as a fake's one lineage.
    pub fn detached(lineage: Lineage) -> Self {
        let home: Weak<dyn Home> = Weak::<Nowhere>::new();
        Self {
            home,
            lineage,
            slot: u32::MAX,
        }
    }

    /// The lineage, by its `Copy` name: what entries, reports and a consumer's own records use.
    pub fn lineage(&self) -> Lineage {
        self.lineage
    }

    /// Act on this handle's slot, if its home is there and the slot is still its lineage's.
    fn with_slot<R>(&self, act: impl FnOnce(&dyn Home, &LineageSlot) -> R) -> Option<R> {
        let home = self.home.upgrade()?;
        let slot = home.slots().held(self.slot, self.lineage)?;
        Some(act(home.as_ref(), slot))
    }

    /// Epoch `id` of this lineage, as a write or a seal takes it.
    pub fn at<E: EpochId + 'static, I: Instant>(&self, id: E) -> Tag<'_, DioringIds<E, I>> {
        Tag::new(self, id)
    }

    /// Whether this handle reports its release to `home`. The lineage a handle names and its
    /// instance are inputs to [`mint`](Self::mint) and [`detached`](Self::detached), so they prove
    /// nothing about where the handle came from; the home does, because a caller holding no
    /// reference to the instance's own cannot name it.
    #[must_use]
    pub fn is_homed_at<T: ?Sized>(&self, home: &Arc<T>) -> bool {
        core::ptr::addr_eq(self.home.as_ptr(), Arc::as_ptr(home))
    }

    /// Whether this is the lineage's only remaining copy.
    #[must_use]
    pub fn is_last(&self) -> bool {
        self.with_slot(|_, slot| slot.copies.load(AtomicOrdering::Acquire) == 1)
            .unwrap_or(false)
    }

    /// Consume the lineage's last handle without ending the lineage through it, because the caller
    /// is ending or retiring it explicitly. Hands the handle back if another copy exists.
    pub fn take_last(self) -> Result<Lineage, Self> {
        let taken = self.with_slot(|_, slot| {
            slot.copies
                .compare_exchange(1, 0, AtomicOrdering::Acquire, AtomicOrdering::Relaxed)
                .is_ok()
        });
        if taken == Some(true) {
            let lineage = self.lineage;
            let mut taken = self;
            // Disarmed: a home that never upgrades, so its drop counts and ends nothing.
            taken.home = Weak::<Nowhere>::new();
            drop(taken);
            Ok(lineage)
        } else {
            Err(self)
        }
    }
}

impl Clone for LineageHandle {
    fn clone(&self) -> Self {
        // A copy is counted from a copy that is counted, so the count is not zero: relaxed, as
        // `Arc`'s is.
        self.with_slot(|_, slot| slot.copies.fetch_add(1, AtomicOrdering::Relaxed));
        Self {
            home: Weak::clone(&self.home),
            lineage: self.lineage,
            slot: self.slot,
        }
    }
}

impl Drop for LineageHandle {
    fn drop(&mut self) {
        let lineage = self.lineage;
        self.with_slot(|home, slot| {
            if slot.copies.fetch_sub(1, AtomicOrdering::Release) == 1 {
                fence(AtomicOrdering::Acquire);
                home.release(lineage);
            }
        });
    }
}

impl Debug for LineageHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("LineageHandle").field(&self.lineage).finish()
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
        let live = tagged(tag::IDS_FAILURE_TOKEN_MINT, || {
            Arc::new(AtomicBool::new(true))
        });
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
