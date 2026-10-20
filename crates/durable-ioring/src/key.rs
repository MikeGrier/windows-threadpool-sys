// Copyright (c) 2026 Mike Grier
//! A target's key, typed by the access its holder declares for it (DI-13.10, DI-D-66).
//!
//! A ring addresses a target by [`IoTargetKey`], a number the consumer picks, so the key alone says
//! nothing of what the target can do. [`Key<A>`] is the same number with a declaration beside it: that
//! the target can do what the [`HasData`] marker `A` says. The ring's submit methods take a
//! [`WriteKey`] or a [`ReadKey`], and a `Key<A>` is one only where `A` can write or can read, so a
//! write through a `Key<ReadOnly>` does not compile. An untyped `IoTargetKey` is both, and is judged
//! when it is pushed ([DI-13.9](crate::backend::RefusalCause::AccessDenied)); so is any typed key that
//! declared more than its target has, unless registration refused it first.
//!
//! The declaration is the consumer's, and the ring cannot see it: the target is erased behind a
//! number. Registration is where it is compared with the target's own stated access, so it is the
//! only runtime step a typed key adds. A key that declares less than its target can do is accepted:
//! it is a narrower view of the same target.

use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;

use crate::access::{Access, CanRead, CanWrite, HasData};
use crate::types::{AddTargetCause, IoTargetKey};

mod sealed {
    pub trait Sealed {}
}

/// A key a target can be registered under: [`IoTargetKey`], which declares nothing, or a [`Key<A>`],
/// which declares the access `A` names. Sealed, so that no other type can stand for a key that has
/// not been checked against what it claims.
pub trait TargetKey: sealed::Sealed {
    /// The number the ring addresses the target by.
    fn untyped(&self) -> IoTargetKey;

    /// The access this key declares for its target, or `None` for an untyped key.
    fn declared(&self) -> Option<Access>;

    /// Whether a target that states `actual` may be registered under this key: a declaration the
    /// target covers, or none. What it refuses is the cause to report.
    ///
    /// # Errors
    ///
    /// [`AddTargetCause::AccessNotCovered`] when the key declares more than `actual` can do.
    fn admitted_by(&self, actual: Access) -> Result<(), AddTargetCause> {
        match self.declared() {
            Some(declared) if !actual.covers(declared) => {
                Err(AddTargetCause::AccessNotCovered { declared, actual })
            }
            _ => Ok(()),
        }
    }
}

/// A key a write may be pushed through: an [`IoTargetKey`], or a [`Key<A>`] where `A` can write.
///
/// ```compile_fail,E0277
/// use durable_ioring::{Key, ReadOnly, WriteKey};
///
/// fn needs_write<K: WriteKey>(_: K) {}
/// needs_write(Key::<ReadOnly>::new(durable_ioring::IoTargetKey(1)));
/// ```
pub trait WriteKey: TargetKey {}

/// A key a read may be pushed through: an [`IoTargetKey`], or a [`Key<A>`] where `A` can read.
///
/// ```compile_fail,E0277
/// use durable_ioring::{Key, ReadKey, WriteOnly};
///
/// fn needs_read<K: ReadKey>(_: K) {}
/// needs_read(Key::<WriteOnly>::new(durable_ioring::IoTargetKey(1)));
/// ```
pub trait ReadKey: TargetKey {}

/// An [`IoTargetKey`] with the access its holder declares for the target under it, as the type `A`.
/// The declaration is checked against the target's stated access when the target is registered
/// under this key, and not afterwards.
pub struct Key<A: HasData> {
    key: IoTargetKey,
    access: PhantomData<fn() -> A>,
}

impl<A: HasData> Key<A> {
    /// `key`, declared to address a target that can do what `A` says.
    #[must_use]
    pub const fn new(key: IoTargetKey) -> Self {
        Self {
            key,
            access: PhantomData,
        }
    }

    /// The number the ring addresses the target by.
    #[must_use]
    pub const fn untyped(self) -> IoTargetKey {
        self.key
    }
}

// By hand: a derive would require `A` to implement each trait, and the markers implement few.
impl<A: HasData> Clone for Key<A> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<A: HasData> Copy for Key<A> {}

impl<A: HasData> PartialEq for Key<A> {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl<A: HasData> Eq for Key<A> {}

impl<A: HasData> Hash for Key<A> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.key.hash(state);
    }
}

impl<A: HasData> fmt::Debug for Key<A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Key")
            .field("key", &self.key)
            .field("declared", &A::ACCESS)
            .finish()
    }
}

impl<A: HasData> From<Key<A>> for IoTargetKey {
    fn from(key: Key<A>) -> Self {
        key.key
    }
}

impl sealed::Sealed for IoTargetKey {}
impl<A: HasData> sealed::Sealed for Key<A> {}

impl TargetKey for IoTargetKey {
    fn untyped(&self) -> IoTargetKey {
        *self
    }

    fn declared(&self) -> Option<Access> {
        None
    }
}

impl<A: HasData> TargetKey for Key<A> {
    fn untyped(&self) -> IoTargetKey {
        self.key
    }

    fn declared(&self) -> Option<Access> {
        Some(A::ACCESS)
    }
}

impl WriteKey for IoTargetKey {}
impl ReadKey for IoTargetKey {}
impl<A: CanWrite> WriteKey for Key<A> {}
impl<A: CanRead> ReadKey for Key<A> {}

#[cfg(test)]
mod tests;
