// Copyright (c) 2026 Mike Grier
//! The set (FHT-D-8): a [`FixedHashMap`] whose values are `()`, so it probes, deletes and refuses
//! exactly as the map does (FHT-D-3 to FHT-D-7).

use core::borrow::Borrow;
use core::hash::{BuildHasher, Hash};

use crate::hasher::FixedState;
use crate::map::{FixedHashMap, Full, Slot};

mod iter;
#[cfg(test)]
mod tests;
mod traits;

pub use iter::{
    Difference, Intersection, SetDrain, SetIntoIter, SetIter, SymmetricDifference, Union,
};

/// A hash set whose capacity is fixed when it is made, which refuses an insert when full rather
/// than grow.
///
/// It is a [`FixedHashMap`] of the values to `()`: `St` is a slice of `Slot<T, ()>`, held inline
/// ([`InlineSet`]) or, under the `alloc` feature, on the heap (`HeapSet`), and `H` is its
/// [`BuildHasher`].
///
/// ```
/// use fixed_hash_table::InlineSet;
///
/// let mut set: InlineSet<u32, 8> = InlineSet::new();
/// assert_eq!(set.capacity(), 7);
/// assert_eq!(set.insert(1), Ok(true));
/// assert_eq!(set.insert(1), Ok(false));
/// for value in 2..=7 {
///     set.insert(value).expect("room");
/// }
/// assert_eq!(set.insert(8).expect_err("full").key, 8);
/// ```
pub struct FixedHashSet<T, St, H = FixedState> {
    map: FixedHashMap<T, (), St, H>,
}

/// A [`FixedHashSet`] whose `N` slots are held inline. `N` must be a power of two; the set holds
/// up to `N - ceil(N / 8)` values.
///
/// ```compile_fail,E0080
/// use fixed_hash_table::InlineSet;
///
/// // Twelve slots is not a power of two.
/// let set: InlineSet<u32, 12> = InlineSet::new();
/// ```
pub type InlineSet<T, const N: usize, H = FixedState> = FixedHashSet<T, [Slot<T, ()>; N], H>;

/// A [`FixedHashSet`] whose slots are on the heap, allocated once when it is made.
#[cfg(feature = "alloc")]
pub type HeapSet<T, H = FixedState> = FixedHashSet<T, alloc::vec::Vec<Slot<T, ()>>, H>;

impl<T, St, H> FixedHashSet<T, St, H>
where
    St: AsRef<[Slot<T, ()>]>,
{
    /// How many values it holds.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Whether it holds no values.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// The most values it holds; an insert of a new value beyond this is refused.
    pub fn capacity(&self) -> usize {
        self.map.capacity()
    }

    /// Its hasher.
    pub fn hasher(&self) -> &H {
        self.map.hasher()
    }
}

impl<T, St, H> FixedHashSet<T, St, H>
where
    T: Hash + Eq,
    St: AsRef<[Slot<T, ()>]>,
    H: BuildHasher,
{
    /// Whether `value` is present.
    pub fn contains<Q>(&self, value: &Q) -> bool
    where
        T: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        self.map.contains_key(value)
    }

    /// The value held equal to `value`.
    pub fn get<Q>(&self, value: &Q) -> Option<&T>
    where
        T: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        self.map.get_key_value(value).map(|(held, ())| held)
    }
}

impl<T, St, H> FixedHashSet<T, St, H>
where
    St: AsMut<[Slot<T, ()>]>,
{
    /// Remove every value. Its storage is kept.
    pub fn clear(&mut self) {
        self.map.clear();
    }
}

impl<T, St, H> FixedHashSet<T, St, H>
where
    T: Hash + Eq,
    St: AsRef<[Slot<T, ()>]> + AsMut<[Slot<T, ()>]>,
    H: BuildHasher,
{
    /// Add `value`: whether it was new. A value already present is kept, and `value` dropped, as
    /// `std`'s set does.
    ///
    /// # Errors
    ///
    /// [`Full`], handing `value` back as its `key`, when `value` is new and the set holds its
    /// [`capacity`](Self::capacity). A value already present needs no room and is never refused.
    pub fn insert(&mut self, value: T) -> Result<bool, Full<T, ()>> {
        self.map
            .insert(value, ())
            .map(|replaced| replaced.is_none())
    }

    /// Add `value`, replacing a value held equal to it: the one replaced.
    ///
    /// # Errors
    ///
    /// As [`insert`](Self::insert): refused only when `value` is new and the set is full.
    pub fn replace(&mut self, value: T) -> Result<Option<T>, Full<T, ()>> {
        // Removing a present value makes the room its replacement needs.
        let replaced = self.map.remove_entry(&value).map(|(held, ())| held);
        self.map.insert(value, ())?;
        Ok(replaced)
    }

    /// Remove `value`: whether it was present.
    pub fn remove<Q>(&mut self, value: &Q) -> bool
    where
        T: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        self.map.remove(value).is_some()
    }

    /// Remove `value`, returning the value that was held equal to it.
    pub fn take<Q>(&mut self, value: &Q) -> Option<T>
    where
        T: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        self.map.remove_entry(value).map(|(held, ())| held)
    }

    /// Keep only the values `keep` answers `true` for, visiting each once.
    pub fn retain<F: FnMut(&T) -> bool>(&mut self, mut keep: F) {
        self.map.retain(|value, ()| keep(value));
    }
}

impl<T, const N: usize, H> FixedHashSet<T, [Slot<T, ()>; N], H> {
    /// An empty set hashing with `hasher`. Allocates nothing.
    pub fn with_hasher(hasher: H) -> Self {
        Self {
            map: FixedHashMap::with_hasher(hasher),
        }
    }
}

impl<T, const N: usize, H: Default> FixedHashSet<T, [Slot<T, ()>; N], H> {
    /// An empty set with the default hasher. Allocates nothing.
    pub fn new() -> Self {
        Self::with_hasher(H::default())
    }
}

impl<T, const N: usize, H: Default> Default for FixedHashSet<T, [Slot<T, ()>; N], H> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "alloc")]
impl<T, H> FixedHashSet<T, alloc::vec::Vec<Slot<T, ()>>, H> {
    /// An empty set holding at least `capacity` values, hashing with `hasher`: its slots are
    /// allocated now, and never again.
    ///
    /// # Errors
    ///
    /// [`ReserveError`](crate::ReserveError) when no such table fits a `usize` or the allocator
    /// cannot provide it. Never panics on running out of memory.
    pub fn try_with_capacity_and_hasher(
        capacity: usize,
        hasher: H,
    ) -> Result<Self, crate::ReserveError> {
        FixedHashMap::try_with_capacity_and_hasher(capacity, hasher).map(|map| Self { map })
    }
}

#[cfg(feature = "alloc")]
impl<T, H: Default> FixedHashSet<T, alloc::vec::Vec<Slot<T, ()>>, H> {
    /// An empty set holding at least `capacity` values, with the default hasher.
    ///
    /// # Errors
    ///
    /// As [`try_with_capacity_and_hasher`](Self::try_with_capacity_and_hasher).
    pub fn try_with_capacity(capacity: usize) -> Result<Self, crate::ReserveError> {
        Self::try_with_capacity_and_hasher(capacity, H::default())
    }
}
