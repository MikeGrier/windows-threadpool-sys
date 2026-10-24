// Copyright (c) 2026 Mike Grier
//! The set's trait implementations and bulk insert (FHT-D-8), as the map has them: `Debug`,
//! `PartialEq` and `Eq` across storages and hashers, `Clone` for inline storage and a fallible
//! `try_clone` for heap storage, and `try_extend` in place of `Extend`.

use super::FixedHashSet;
use crate::map::{Full, Slot};
use core::fmt;
use core::hash::{BuildHasher, Hash};

impl<T, St, H> FixedHashSet<T, St, H>
where
    T: Hash + Eq,
    St: AsRef<[Slot<T, ()>]> + AsMut<[Slot<T, ()>]>,
    H: BuildHasher,
{
    /// Insert every value of `values`, in order, as [`insert`](Self::insert) would.
    ///
    /// There is no `Extend`: extending cannot report a refusal. Pass `values.by_ref()` to keep what
    /// follows a refused value.
    ///
    /// # Errors
    ///
    /// [`Full`] for the first value refused, handed back as its `key`. The values before it are
    /// inserted, and none after it is taken from the iterator.
    pub fn try_extend<I: IntoIterator<Item = T>>(&mut self, values: I) -> Result<(), Full<T, ()>> {
        self.map
            .try_extend(values.into_iter().map(|value| (value, ())))
    }
}

impl<T: fmt::Debug, St: AsRef<[Slot<T, ()>]>, H> fmt::Debug for FixedHashSet<T, St, H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.iter()).finish()
    }
}

/// Two sets are equal when they hold the same values, whatever their storage, hashers or
/// capacities.
impl<T, St, H, St2, H2> PartialEq<FixedHashSet<T, St2, H2>> for FixedHashSet<T, St, H>
where
    T: Hash + Eq,
    St: AsRef<[Slot<T, ()>]>,
    St2: AsRef<[Slot<T, ()>]>,
    H2: BuildHasher,
{
    fn eq(&self, other: &FixedHashSet<T, St2, H2>) -> bool {
        self.map == other.map
    }
}

impl<T, St, H> Eq for FixedHashSet<T, St, H>
where
    T: Hash + Eq,
    St: AsRef<[Slot<T, ()>]>,
    H: BuildHasher,
{
}

/// Cloning an inline set copies its slots and allocates nothing. A heap set has `try_clone`
/// instead, with the `alloc` feature: `Clone` cannot report an allocation that fails.
impl<T: Clone, const N: usize, H: Clone> Clone for FixedHashSet<T, [Slot<T, ()>; N], H> {
    fn clone(&self) -> Self {
        Self {
            map: self.map.clone(),
        }
    }
}

#[cfg(feature = "alloc")]
impl<T: Clone, H: Clone> FixedHashSet<T, alloc::vec::Vec<Slot<T, ()>>, H> {
    /// A copy, with slots of its own allocated now.
    ///
    /// # Errors
    ///
    /// [`ReserveError::Alloc`](crate::ReserveError::Alloc) when the allocator cannot provide them.
    /// Never panics on running out of memory.
    pub fn try_clone(&self) -> Result<Self, crate::ReserveError> {
        self.map.try_clone().map(|map| Self { map })
    }
}
