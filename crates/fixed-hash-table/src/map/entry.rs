// Copyright (c) 2026 Mike Grier
//! The entry API (FHT-D-8): a key's place in the map, occupied or vacant, with a vacant insert that
//! is refused when the map is full.

use super::{FixedHashMap, Full, Slot};
use core::hash::{BuildHasher, Hash};
use core::mem;

#[cfg(test)]
mod tests;

/// A key's place in a [`FixedHashMap`], from [`entry`](FixedHashMap::entry).
pub enum Entry<'a, K, V, St, H> {
    /// The key is present.
    Occupied(OccupiedEntry<'a, K, V, St, H>),
    /// The key is absent.
    Vacant(VacantEntry<'a, K, V, St, H>),
}

/// A present key's entry.
pub struct OccupiedEntry<'a, K, V, St, H> {
    map: &'a mut FixedHashMap<K, V, St, H>,
    index: usize,
}

/// An absent key's entry: inserting into it is refused when the map is full.
pub struct VacantEntry<'a, K, V, St, H> {
    map: &'a mut FixedHashMap<K, V, St, H>,
    key: K,
    index: usize,
}

/// What an occupied entry's slot holds. Its slot was found occupied when the entry was made, and
/// nothing else reaches the map while the entry borrows it.
const OCCUPIED: &str = "an occupied entry's slot holds its entry";

impl<K, V, St, H> FixedHashMap<K, V, St, H>
where
    K: Hash + Eq,
    St: AsRef<[Slot<K, V>]> + AsMut<[Slot<K, V>]>,
    H: BuildHasher,
{
    /// `key`'s entry, for reading, changing or inserting in place.
    ///
    /// ```
    /// use fixed_hash_table::InlineMap;
    ///
    /// let mut counts: InlineMap<&str, u32, 8> = InlineMap::new();
    /// for word in ["a", "b", "a"] {
    ///     *counts.entry(word).or_insert(0).expect("room") += 1;
    /// }
    /// assert_eq!(counts.get("a"), Some(&2));
    /// ```
    pub fn entry(&mut self, key: K) -> Entry<'_, K, V, St, H> {
        let hash = self.hasher.hash_one(&key);
        match self.find(hash, &key) {
            Ok(index) => Entry::Occupied(OccupiedEntry { map: self, index }),
            Err(index) => Entry::Vacant(VacantEntry {
                map: self,
                key,
                index,
            }),
        }
    }
}

impl<'a, K, V, St, H> Entry<'a, K, V, St, H>
where
    K: Hash + Eq,
    St: AsRef<[Slot<K, V>]> + AsMut<[Slot<K, V>]>,
    H: BuildHasher,
{
    /// The entry's key: the one held, for an occupied entry.
    pub fn key(&self) -> &K {
        match self {
            Self::Occupied(entry) => entry.key(),
            Self::Vacant(entry) => entry.key(),
        }
    }

    /// The value, inserting `default` if the key is absent.
    ///
    /// # Errors
    ///
    /// [`Full`], handing the key and `default` back, when the key is absent and the map is full.
    pub fn or_insert(self, default: V) -> Result<&'a mut V, Full<K, V>> {
        match self {
            Self::Occupied(entry) => Ok(entry.into_mut()),
            Self::Vacant(entry) => entry.insert(default),
        }
    }

    /// The value, inserting what `make` makes if the key is absent. `make` is called only if there
    /// is room.
    ///
    /// # Errors
    ///
    /// [`Full`], handing the key and `make` back uncalled, when the key is absent and the map is
    /// full.
    pub fn or_insert_with<F: FnOnce() -> V>(self, make: F) -> Result<&'a mut V, Full<K, F>> {
        match self {
            Self::Occupied(entry) => Ok(entry.into_mut()),
            Self::Vacant(entry) if entry.map.len == entry.map.capacity => Err(Full {
                key: entry.key,
                value: make,
            }),
            Self::Vacant(entry) => Ok(entry.place(make())),
        }
    }

    /// As [`or_insert_with`](Self::or_insert_with), with `make` given the key.
    ///
    /// # Errors
    ///
    /// As [`or_insert_with`](Self::or_insert_with).
    pub fn or_insert_with_key<F: FnOnce(&K) -> V>(self, make: F) -> Result<&'a mut V, Full<K, F>> {
        match self {
            Self::Occupied(entry) => Ok(entry.into_mut()),
            Self::Vacant(entry) if entry.map.len == entry.map.capacity => Err(Full {
                key: entry.key,
                value: make,
            }),
            Self::Vacant(entry) => {
                let value = make(&entry.key);
                Ok(entry.place(value))
            }
        }
    }

    /// The value, inserting `V::default()` if the key is absent.
    ///
    /// # Errors
    ///
    /// [`Full`], handing the key and a default value back, when the key is absent and the map is
    /// full.
    pub fn or_default(self) -> Result<&'a mut V, Full<K, V>>
    where
        V: Default,
    {
        self.or_insert_with(V::default).map_err(|full| Full {
            key: full.key,
            value: (full.value)(),
        })
    }

    /// Run `change` on the value if the key is present.
    #[must_use]
    pub fn and_modify<F: FnOnce(&mut V)>(mut self, change: F) -> Self {
        if let Self::Occupied(entry) = &mut self {
            change(entry.get_mut());
        }
        self
    }
}

impl<'a, K, V, St, H> OccupiedEntry<'a, K, V, St, H>
where
    K: Hash + Eq,
    St: AsRef<[Slot<K, V>]> + AsMut<[Slot<K, V>]>,
    H: BuildHasher,
{
    /// The key held.
    pub fn key(&self) -> &K {
        &self.entry().0
    }

    /// The value.
    pub fn get(&self) -> &V {
        &self.entry().1
    }

    /// The value, mutably.
    pub fn get_mut(&mut self) -> &mut V {
        &mut self.map.slots.as_mut()[self.index]
            .as_mut()
            .expect(OCCUPIED)
            .1
    }

    /// The value, mutably, for as long as the map was borrowed.
    pub fn into_mut(self) -> &'a mut V {
        &mut self.map.slots.as_mut()[self.index]
            .as_mut()
            .expect(OCCUPIED)
            .1
    }

    /// Replace the value, returning the one replaced.
    pub fn insert(&mut self, value: V) -> V {
        mem::replace(self.get_mut(), value)
    }

    /// Remove the entry, returning its value.
    pub fn remove(self) -> V {
        self.remove_entry().1
    }

    /// Remove the entry, returning the key held and its value.
    pub fn remove_entry(self) -> (K, V) {
        self.map.remove_at(self.index).expect(OCCUPIED)
    }

    fn entry(&self) -> &(K, V) {
        self.map.slots.as_ref()[self.index]
            .as_ref()
            .expect(OCCUPIED)
    }
}

impl<'a, K, V, St, H> VacantEntry<'a, K, V, St, H>
where
    St: AsMut<[Slot<K, V>]>,
{
    /// The key it would insert.
    pub fn key(&self) -> &K {
        &self.key
    }

    /// The key, given back.
    pub fn into_key(self) -> K {
        self.key
    }

    /// Insert `value` under the key, returning it in place.
    ///
    /// # Errors
    ///
    /// [`Full`], handing the key and `value` back, when the map is full.
    pub fn insert(self, value: V) -> Result<&'a mut V, Full<K, V>> {
        if self.map.len == self.map.capacity {
            return Err(Full {
                key: self.key,
                value,
            });
        }
        Ok(self.place(value))
    }

    /// Put the entry in its slot, which the caller has checked there is room for.
    fn place(self, value: V) -> &'a mut V {
        self.map.len += 1;
        let slot = &mut self.map.slots.as_mut()[self.index];
        &mut slot.insert((self.key, value)).1
    }
}
