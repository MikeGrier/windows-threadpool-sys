// Copyright (c) 2026 Mike Grier
//! The map's trait implementations and bulk insert (FHT-D-8): `Debug`, `PartialEq` and `Eq`,
//! `Index`, `Clone` for inline storage and a fallible `try_clone` for heap storage, and
//! `try_extend` in place of `Extend`.

use super::{FixedHashMap, Full, Slot};
use core::borrow::Borrow;
use core::fmt;
use core::hash::{BuildHasher, Hash};
use core::marker::PhantomData;
use core::ops::Index;

#[cfg(test)]
mod tests;

impl<K, V, St, H> FixedHashMap<K, V, St, H>
where
    K: Hash + Eq,
    St: AsRef<[Slot<K, V>]> + AsMut<[Slot<K, V>]>,
    H: BuildHasher,
{
    /// Insert every entry of `entries`, in order, as [`insert`](Self::insert) would.
    ///
    /// There is no `Extend`: extending cannot report a refusal. Pass `entries.by_ref()` to keep
    /// what follows a refused entry.
    ///
    /// # Errors
    ///
    /// [`Full`] for the first entry refused, handed back. The entries before it are inserted, and
    /// none after it is taken from the iterator.
    pub fn try_extend<I: IntoIterator<Item = (K, V)>>(
        &mut self,
        entries: I,
    ) -> Result<(), Full<K, V>> {
        for (key, value) in entries {
            self.insert(key, value)?;
        }
        Ok(())
    }
}

impl<K, V, St, H> fmt::Debug for FixedHashMap<K, V, St, H>
where
    K: fmt::Debug,
    V: fmt::Debug,
    St: AsRef<[Slot<K, V>]>,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

/// Two maps are equal when they hold the same keys with equal values, whatever their storage,
/// hashers or capacities.
impl<K, V, St, H, St2, H2> PartialEq<FixedHashMap<K, V, St2, H2>> for FixedHashMap<K, V, St, H>
where
    K: Hash + Eq,
    V: PartialEq,
    St: AsRef<[Slot<K, V>]>,
    St2: AsRef<[Slot<K, V>]>,
    H2: BuildHasher,
{
    fn eq(&self, other: &FixedHashMap<K, V, St2, H2>) -> bool {
        self.len == other.len
            && self
                .iter()
                .all(|(key, value)| other.get(key) == Some(value))
    }
}

impl<K, V, St, H> Eq for FixedHashMap<K, V, St, H>
where
    K: Hash + Eq,
    V: Eq,
    St: AsRef<[Slot<K, V>]>,
    H: BuildHasher,
{
}

/// `map[key]`, as `std`'s map has it.
///
/// # Panics
///
/// When `key` is absent; [`get`](FixedHashMap::get) does not.
impl<K, Q, V, St, H> Index<&Q> for FixedHashMap<K, V, St, H>
where
    K: Hash + Eq + Borrow<Q>,
    Q: ?Sized + Hash + Eq,
    St: AsRef<[Slot<K, V>]>,
    H: BuildHasher,
{
    type Output = V;

    fn index(&self, key: &Q) -> &V {
        self.get(key).expect("no entry for the key")
    }
}

/// Cloning an inline map copies its slots and allocates nothing. A heap map has `try_clone`
/// instead, with the `alloc` feature: `Clone` cannot report an allocation that
/// fails.
impl<K: Clone, V: Clone, const N: usize, H: Clone> Clone
    for FixedHashMap<K, V, [Slot<K, V>; N], H>
{
    fn clone(&self) -> Self {
        Self {
            slots: self.slots.clone(),
            hasher: self.hasher.clone(),
            len: self.len,
            capacity: self.capacity,
            shift: self.shift,
            entries: PhantomData,
        }
    }
}

#[cfg(feature = "alloc")]
impl<K: Clone, V: Clone, H: Clone> FixedHashMap<K, V, alloc::vec::Vec<Slot<K, V>>, H> {
    /// A copy, with slots of its own allocated now.
    ///
    /// # Errors
    ///
    /// [`ReserveError::Alloc`](super::ReserveError::Alloc) when the allocator cannot provide them.
    /// Never panics on running out of memory.
    pub fn try_clone(&self) -> Result<Self, super::ReserveError> {
        let mut slots = alloc::vec::Vec::new();
        slots
            .try_reserve_exact(self.slots.len())
            .map_err(super::ReserveError::Alloc)?;
        // Within the capacity just reserved, so this does not allocate.
        slots.extend(self.slots.iter().cloned());
        Ok(Self {
            slots,
            hasher: self.hasher.clone(),
            len: self.len,
            capacity: self.capacity,
            shift: self.shift,
            entries: PhantomData,
        })
    }
}
