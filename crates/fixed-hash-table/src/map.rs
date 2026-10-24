// Copyright (c) 2026 Mike Grier
//! The map (FHT-D-3 to FHT-D-7): open addressing over a fixed slice of slots, with linear probing
//! and backward-shift deletion, refusing an insert past its capacity.

use core::borrow::Borrow;
use core::error::Error;
use core::fmt;
use core::hash::{BuildHasher, Hash};
use core::marker::PhantomData;
use core::mem;

use crate::hasher::FixedState;

mod entry;
mod iter;
#[cfg(test)]
mod tests;
mod traits;

pub use entry::{Entry, OccupiedEntry, VacantEntry};
pub use iter::{Drain, IntoIter, Iter, IterMut, Keys, Values, ValuesMut};

/// One slot of a table's storage: empty, or an entry.
pub type Slot<K, V> = Option<(K, V)>;

/// A hash map whose capacity is fixed when it is made, which refuses an insert when full rather
/// than grow.
///
/// `St` is its storage: a slice of [`Slot`]s, held inline as an array ([`InlineMap`]) or, under the
/// `alloc` feature, on the heap (`HeapMap`), allocated once when the map is made. `H` is its
/// [`BuildHasher`] (FHT-D-5): the default, [`FixedState`], is zero-sized and seedless.
///
/// It holds at most [`capacity`](Self::capacity) entries, fewer than its slots, so a probe always
/// meets an empty slot (FHT-D-4).
///
/// ```
/// use fixed_hash_table::InlineMap;
///
/// let mut map: InlineMap<u32, &str, 8> = InlineMap::new();
/// assert_eq!(map.capacity(), 7);
/// assert_eq!(map.insert(1, "one"), Ok(None));
/// assert_eq!(map.get(&1), Some(&"one"));
/// for key in 2..=7 {
///     map.insert(key, "more").expect("room");
/// }
/// let full = map.insert(8, "eight").expect_err("full");
/// assert_eq!((full.key, full.value), (8, "eight"));
/// ```
pub struct FixedHashMap<K, V, St, H = FixedState> {
    slots: St,
    hasher: H,
    len: usize,
    capacity: usize,
    /// How far right a hash is shifted to index the slots: its high bits are the index.
    shift: u32,
    entries: PhantomData<(K, V)>,
}

/// A [`FixedHashMap`] whose `N` slots are held inline. `N` must be a power of two; the map holds
/// up to `N - ceil(N / 8)` entries.
///
/// ```compile_fail,E0080
/// use fixed_hash_table::InlineMap;
///
/// // Twelve slots is not a power of two.
/// let map: InlineMap<u32, u32, 12> = InlineMap::new();
/// ```
pub type InlineMap<K, V, const N: usize, H = FixedState> = FixedHashMap<K, V, [Slot<K, V>; N], H>;

/// A [`FixedHashMap`] whose slots are on the heap, allocated once when it is made.
#[cfg(feature = "alloc")]
pub type HeapMap<K, V, H = FixedState> = FixedHashMap<K, V, alloc::vec::Vec<Slot<K, V>>, H>;

/// How many entries `slots` slots hold: at least one slot in eight stays empty, rounded up, so a
/// probe always ends.
const fn capacity_of(slots: usize) -> usize {
    slots - slots.div_ceil(8)
}

/// An insert refused because the map is full: the key and value it was given, handed back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Full<K, V> {
    /// The key.
    pub key: K,
    /// The value.
    pub value: V,
}

impl<K, V> fmt::Display for Full<K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the table is full")
    }
}

impl<K: fmt::Debug, V: fmt::Debug> Error for Full<K, V> {}

impl<K, V, St, H> FixedHashMap<K, V, St, H>
where
    St: AsRef<[Slot<K, V>]>,
{
    /// A map over `slots`, which are empty and a power of two in number.
    fn from_parts(slots: St, hasher: H) -> Self {
        let count = slots.as_ref().len();
        Self {
            slots,
            hasher,
            len: 0,
            capacity: capacity_of(count),
            shift: u64::BITS - count.trailing_zeros(),
            entries: PhantomData,
        }
    }

    /// How many entries it holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether it holds none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// How many entries it can hold: an insert of a new key past this is refused.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Its hasher.
    #[must_use]
    pub fn hasher(&self) -> &H {
        &self.hasher
    }

    /// The slot a hash starts its probe at: the hash's high bits.
    fn home(&self, hash: u64) -> usize {
        // The slot count is at most `usize::MAX`, so the shifted hash fits.
        hash.checked_shr(self.shift).unwrap_or(0) as usize
    }

    /// The slot holding `key`, or the empty slot its probe ends at. A probe always ends: the map
    /// keeps an empty slot (FHT-D-4).
    fn find<Q>(&self, hash: u64, key: &Q) -> Result<usize, usize>
    where
        K: Borrow<Q>,
        Q: ?Sized + Eq,
    {
        let slots = self.slots.as_ref();
        let mask = slots.len() - 1;
        let mut i = self.home(hash);
        loop {
            match &slots[i] {
                None => return Err(i),
                Some((k, _)) if k.borrow() == key => return Ok(i),
                Some(_) => i = (i + 1) & mask,
            }
        }
    }
}

impl<K, V, St, H> FixedHashMap<K, V, St, H>
where
    K: Hash + Eq,
    St: AsRef<[Slot<K, V>]>,
    H: BuildHasher,
{
    /// The value under `key`.
    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        self.get_key_value(key).map(|(_, value)| value)
    }

    /// The key held for `key`, and its value.
    pub fn get_key_value<Q>(&self, key: &Q) -> Option<(&K, &V)>
    where
        K: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        let i = self.find(self.hasher.hash_one(key), key).ok()?;
        self.slots.as_ref()[i].as_ref().map(|(k, v)| (k, v))
    }

    /// Whether `key` is present.
    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        self.find(self.hasher.hash_one(key), key).is_ok()
    }
}

impl<K, V, St, H> FixedHashMap<K, V, St, H>
where
    K: Hash + Eq,
    St: AsRef<[Slot<K, V>]> + AsMut<[Slot<K, V>]>,
    H: BuildHasher,
{
    /// Insert `value` under `key`: the value it replaced, if `key` was present, keeping the key
    /// already held, as `std`'s map does.
    ///
    /// # Errors
    ///
    /// [`Full`], handing `key` and `value` back, when `key` is new and the map holds its
    /// [`capacity`](Self::capacity). Replacing a present key's value needs no room and is never
    /// refused (FHT-D-7).
    pub fn insert(&mut self, key: K, value: V) -> Result<Option<V>, Full<K, V>> {
        let hash = self.hasher.hash_one(&key);
        match self.find(hash, &key) {
            Ok(i) => Ok(self.slots.as_mut()[i]
                .as_mut()
                .map(|(_, held)| mem::replace(held, value))),
            Err(_) if self.len == self.capacity => Err(Full { key, value }),
            Err(empty) => {
                self.slots.as_mut()[empty] = Some((key, value));
                self.len += 1;
                Ok(None)
            }
        }
    }

    /// The value under `key`, mutably.
    pub fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        let i = self.find(self.hasher.hash_one(key), key).ok()?;
        self.slots.as_mut()[i].as_mut().map(|(_, value)| value)
    }

    /// Remove `key`, returning its value.
    pub fn remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        self.remove_entry(key).map(|(_, value)| value)
    }

    /// Remove `key`, returning the key held for it and its value. The entries after it in its
    /// probe run are shifted back into the hole, so no tombstone is left (FHT-D-4).
    pub fn remove_entry<Q>(&mut self, key: &Q) -> Option<(K, V)>
    where
        K: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        let at = self.find(self.hasher.hash_one(key), key).ok()?;
        self.remove_at(at)
    }

    /// Remove the entry in slot `hole`, shifting the rest of its probe run back.
    fn remove_at(&mut self, mut hole: usize) -> Slot<K, V> {
        let removed = self.slots.as_mut()[hole].take();
        removed.as_ref()?;
        self.len -= 1;
        let mask = self.slots.as_ref().len() - 1;
        let mut next = hole;
        loop {
            next = (next + 1) & mask;
            let home = match &self.slots.as_ref()[next] {
                None => break,
                Some((k, _)) => self.home(self.hasher.hash_one(k)),
            };
            // The entry at `next` may move back into the hole only if the hole lies within its
            // own probe run, from its home up to where it sits.
            if next.wrapping_sub(home) & mask >= next.wrapping_sub(hole) & mask {
                let moved = self.slots.as_mut()[next].take();
                self.slots.as_mut()[hole] = moved;
                hole = next;
            }
        }
        removed
    }

    /// Remove every entry. Its storage is kept.
    pub fn clear(&mut self) {
        for slot in self.slots.as_mut() {
            *slot = None;
        }
        self.len = 0;
    }
}

impl<K, V, const N: usize, H> FixedHashMap<K, V, [Slot<K, V>; N], H> {
    /// Rejects at compile time a slot count that is not a power of two.
    const SLOTS_ARE_A_POWER_OF_TWO: () = assert!(
        N.is_power_of_two(),
        "an inline table's slot count must be a power of two"
    );

    /// An empty map hashing with `hasher`. Allocates nothing.
    pub fn with_hasher(hasher: H) -> Self {
        let () = Self::SLOTS_ARE_A_POWER_OF_TWO;
        Self::from_parts(core::array::from_fn(|_| None), hasher)
    }
}

impl<K, V, const N: usize, H: Default> FixedHashMap<K, V, [Slot<K, V>; N], H> {
    /// An empty map with the default hasher. Allocates nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::with_hasher(H::default())
    }
}

impl<K, V, const N: usize, H: Default> Default for FixedHashMap<K, V, [Slot<K, V>; N], H> {
    fn default() -> Self {
        Self::new()
    }
}

/// Why a [`HeapMap`] could not be made.
#[cfg(feature = "alloc")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReserveError {
    /// No power-of-two slot count that fits a `usize` holds the capacity asked for.
    TooLarge,
    /// The allocator could not provide the slots.
    Alloc(alloc::collections::TryReserveError),
}

#[cfg(feature = "alloc")]
impl fmt::Display for ReserveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge => f.write_str("no table that large can be made"),
            Self::Alloc(error) => write!(f, "the table's slots could not be allocated: {error}"),
        }
    }
}

#[cfg(feature = "alloc")]
impl Error for ReserveError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::TooLarge => None,
            Self::Alloc(error) => Some(error),
        }
    }
}

/// The fewest slots, a power of two, that hold `capacity` entries.
#[cfg(feature = "alloc")]
fn slots_for(capacity: usize) -> Option<usize> {
    let mut slots = capacity.checked_next_power_of_two()?;
    while capacity_of(slots) < capacity {
        slots = slots.checked_mul(2)?;
    }
    Some(slots)
}

#[cfg(feature = "alloc")]
impl<K, V, H> FixedHashMap<K, V, alloc::vec::Vec<Slot<K, V>>, H> {
    /// An empty map holding at least `capacity` entries, hashing with `hasher`: its slots are
    /// allocated now, and never again.
    ///
    /// # Errors
    ///
    /// [`ReserveError`] when no such table fits a `usize` or the allocator cannot provide it.
    /// Never panics on running out of memory.
    pub fn try_with_capacity_and_hasher(capacity: usize, hasher: H) -> Result<Self, ReserveError> {
        let count = slots_for(capacity).ok_or(ReserveError::TooLarge)?;
        let mut slots = alloc::vec::Vec::new();
        slots
            .try_reserve_exact(count)
            .map_err(ReserveError::Alloc)?;
        // Within the capacity just reserved, so this does not allocate.
        slots.resize_with(count, || None);
        Ok(Self::from_parts(slots, hasher))
    }
}

#[cfg(feature = "alloc")]
impl<K, V, H: Default> FixedHashMap<K, V, alloc::vec::Vec<Slot<K, V>>, H> {
    /// An empty map holding at least `capacity` entries, with the default hasher.
    ///
    /// # Errors
    ///
    /// As [`try_with_capacity_and_hasher`](Self::try_with_capacity_and_hasher).
    pub fn try_with_capacity(capacity: usize) -> Result<Self, ReserveError> {
        Self::try_with_capacity_and_hasher(capacity, H::default())
    }
}
