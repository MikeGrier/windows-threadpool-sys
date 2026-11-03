// Copyright (c) 2026 Mike Grier
//! A table ordered by key, in storage reserved when it is made and never grown (DI-D-109): the
//! core's ordered tables, which a `BTreeMap` would allocate a node for on every insert.
//!
//! The entries are a sorted array searched by binary search. A removal leaves its slot empty in
//! place rather than shifting what follows, and the empty slots are compacted away only when an
//! insert finds the array full; an insert next to an empty slot takes it. So a table keyed by a
//! count that only rises -- a push sequence, a lineage key -- inserts at its end and removes in
//! place, and neither grows the array: it holds at most as many entries as it was made for, and
//! refuses one more. Safe code: the storage is a `Vec` whose capacity is reserved once.

use alloc::collections::TryReserveError;
use alloc::vec::Vec;
use core::ops::Index;

#[cfg(test)]
mod tests;

/// An insert the table refused: it holds as many entries as it was made for.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Full;

/// A table of at most `limit` entries, ordered by key.
pub(crate) struct Ordered<K, V> {
    /// Ascending by key; a `None` is an entry removed and not yet compacted away.
    entries: Vec<(K, Option<V>)>,
    /// The entries present.
    live: usize,
    /// The most it holds.
    limit: usize,
}

impl<K: Ord, V> Ordered<K, V> {
    /// An empty table that holds up to `limit` entries, its storage reserved now.
    ///
    /// # Errors
    ///
    /// The allocator's refusal, or a `limit` too large to reserve.
    pub(crate) fn with_capacity(limit: usize) -> Result<Self, TryReserveError> {
        let mut entries = Vec::new();
        entries.try_reserve_exact(limit)?;
        Ok(Self {
            entries,
            live: 0,
            limit,
        })
    }

    /// The entries present.
    pub(crate) fn len(&self) -> usize {
        self.live
    }

    /// Where `key`'s slot is, or where it would go.
    fn find(&self, key: &K) -> Result<usize, usize> {
        self.entries.binary_search_by(|(at, _)| at.cmp(key))
    }

    /// The value under `key`.
    pub(crate) fn get(&self, key: &K) -> Option<&V> {
        self.find(key)
            .ok()
            .and_then(|at| self.entries[at].1.as_ref())
    }

    /// The value under `key`, mutably.
    pub(crate) fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        self.find(key)
            .ok()
            .and_then(|at| self.entries[at].1.as_mut())
    }

    /// Put `value` under `key`, handing back the value it replaced.
    ///
    /// # Errors
    ///
    /// [`Full`] when `key` is new and the table already holds as many entries as it was made
    /// for; nothing changes.
    pub(crate) fn insert(&mut self, key: K, value: V) -> Result<Option<V>, Full> {
        let at = match self.find(&key) {
            Ok(at) => {
                let old = self.entries[at].1.replace(value);
                if old.is_none() {
                    self.live += 1;
                }
                return Ok(old);
            }
            Err(at) => at,
        };
        if self.live == self.limit {
            return Err(Full);
        }
        // An empty slot beside the place keeps the order if the entry takes it.
        if at > 0 && self.entries[at - 1].1.is_none() {
            self.entries[at - 1] = (key, Some(value));
        } else if at < self.entries.len() && self.entries[at].1.is_none() {
            self.entries[at] = (key, Some(value));
        } else if self.entries.len() < self.limit {
            self.entries.insert(at, (key, Some(value)));
        } else {
            self.compact();
            let at = self.find(&key).err().unwrap_or(self.entries.len());
            self.entries.insert(at, (key, Some(value)));
        }
        self.live += 1;
        Ok(None)
    }

    /// Take the value under `key` out.
    pub(crate) fn remove(&mut self, key: &K) -> Option<V> {
        let at = self.find(key).ok()?;
        let value = self.entries[at].1.take()?;
        self.live -= 1;
        while self
            .entries
            .last()
            .is_some_and(|(_, value)| value.is_none())
        {
            self.entries.pop();
        }
        Some(value)
    }

    /// Drop every empty slot.
    fn compact(&mut self) {
        self.entries.retain(|(_, value)| value.is_some());
    }

    /// Every entry, in key order.
    pub(crate) fn iter(&self) -> Present<'_, K, V> {
        Present(self.entries.iter())
    }

    /// Every entry, in key order, its value mutably.
    pub(crate) fn iter_mut(&mut self) -> PresentMut<'_, K, V> {
        PresentMut(self.entries.iter_mut())
    }

    /// Every key, in order.
    pub(crate) fn keys(&self) -> impl DoubleEndedIterator<Item = &K> + Clone {
        self.iter().map(|(key, _)| key)
    }

    /// Every value, in key order.
    pub(crate) fn values(&self) -> impl DoubleEndedIterator<Item = &V> + Clone {
        self.iter().map(|(_, value)| value)
    }

    /// Every value, in key order, mutably.
    pub(crate) fn values_mut(&mut self) -> impl DoubleEndedIterator<Item = &mut V> {
        self.iter_mut().map(|(_, value)| value)
    }

    /// Every value, in key order, taken.
    pub(crate) fn into_values(self) -> impl DoubleEndedIterator<Item = V> {
        self.entries.into_iter().filter_map(|(_, value)| value)
    }

    /// Every entry from the first key `before` is false for up to the last key `upto` is true for,
    /// in key order: a range given by two predicates, each true for a leading run of the keys, for
    /// keys a `RangeBounds` cannot name, such as every key of one lineage.
    pub(crate) fn between(
        &self,
        before: impl Fn(&K) -> bool,
        upto: impl Fn(&K) -> bool,
    ) -> Present<'_, K, V> {
        let start = self.entries.partition_point(|(key, _)| before(key));
        let end = self
            .entries
            .partition_point(|(key, _)| upto(key))
            .max(start);
        Present(self.entries[start..end].iter())
    }

    /// As [`between`](Self::between), the values mutably.
    pub(crate) fn between_mut(
        &mut self,
        before: impl Fn(&K) -> bool,
        upto: impl Fn(&K) -> bool,
    ) -> PresentMut<'_, K, V> {
        let start = self.entries.partition_point(|(key, _)| before(key));
        let end = self
            .entries
            .partition_point(|(key, _)| upto(key))
            .max(start);
        PresentMut(self.entries[start..end].iter_mut())
    }

    /// Keep only the entries `keep` says to, in key order.
    pub(crate) fn retain(&mut self, mut keep: impl FnMut(&K, &mut V) -> bool) {
        for (key, slot) in &mut self.entries {
            if let Some(value) = slot
                && !keep(key, value)
            {
                *slot = None;
                self.live -= 1;
            }
        }
        while self
            .entries
            .last()
            .is_some_and(|(_, value)| value.is_none())
        {
            self.entries.pop();
        }
    }

    /// Take out every entry `takes` picks, in key order, giving each to `taken`.
    pub(crate) fn take_where(
        &mut self,
        mut takes: impl FnMut(&K, &V) -> bool,
        mut taken: impl FnMut(&K, V),
    ) {
        for (key, slot) in &mut self.entries {
            if slot.as_ref().is_some_and(|value| takes(key, value))
                && let Some(value) = slot.take()
            {
                self.live -= 1;
                taken(key, value);
            }
        }
        while self
            .entries
            .last()
            .is_some_and(|(_, value)| value.is_none())
        {
            self.entries.pop();
        }
    }
}

/// The entries present in a run of a table's slots, in key order.
pub(crate) struct Present<'a, K, V>(core::slice::Iter<'a, (K, Option<V>)>);

impl<K, V> Clone for Present<'_, K, V> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<'a, K, V> Iterator for Present<'a, K, V> {
    type Item = (&'a K, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        self.0
            .by_ref()
            .find_map(|(key, value)| value.as_ref().map(|value| (key, value)))
    }
}

impl<K, V> DoubleEndedIterator for Present<'_, K, V> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.0
            .by_ref()
            .rev()
            .find_map(|(key, value)| value.as_ref().map(|value| (key, value)))
    }
}

/// The entries present in a run of a table's slots, in key order, their values mutably.
pub(crate) struct PresentMut<'a, K, V>(core::slice::IterMut<'a, (K, Option<V>)>);

impl<'a, K, V> Iterator for PresentMut<'a, K, V> {
    type Item = (&'a K, &'a mut V);

    fn next(&mut self) -> Option<Self::Item> {
        self.0
            .by_ref()
            .find_map(|(key, value)| value.as_mut().map(|value| (&*key, value)))
    }
}

impl<K, V> DoubleEndedIterator for PresentMut<'_, K, V> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.0
            .by_ref()
            .rev()
            .find_map(|(key, value)| value.as_mut().map(|value| (&*key, value)))
    }
}

impl<K: Ord, V> Index<&K> for Ordered<K, V> {
    type Output = V;

    /// # Panics
    ///
    /// If no entry is under `key`.
    fn index(&self, key: &K) -> &V {
        self.get(key).expect("an entry under the key")
    }
}
