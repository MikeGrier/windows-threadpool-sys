// Copyright (c) 2026 Mike Grier
//! Iteration (FHT-D-8): over the slots in their order, which is arbitrary, yielding the occupied
//! ones; and the removals that walk the whole table, `drain` and `retain`.

use super::{FixedHashMap, Slot};
use core::hash::{BuildHasher, Hash};
use core::iter::FusedIterator;
use core::slice;

#[cfg(test)]
mod tests;

/// The entries, shared: from [`iter`](FixedHashMap::iter).
pub struct Iter<'a, K, V> {
    slots: slice::Iter<'a, Slot<K, V>>,
    left: usize,
}

/// The entries, with their values mutable: from [`iter_mut`](FixedHashMap::iter_mut).
pub struct IterMut<'a, K, V> {
    slots: slice::IterMut<'a, Slot<K, V>>,
    left: usize,
}

/// The keys: from [`keys`](FixedHashMap::keys).
pub struct Keys<'a, K, V>(Iter<'a, K, V>);

/// The values: from [`values`](FixedHashMap::values).
pub struct Values<'a, K, V>(Iter<'a, K, V>);

/// The values, mutable: from [`values_mut`](FixedHashMap::values_mut).
pub struct ValuesMut<'a, K, V>(IterMut<'a, K, V>);

/// The entries, taken: from the map's `into_iter`. `I` is its storage's own iterator.
pub struct IntoIter<I> {
    slots: I,
    left: usize,
}

/// The entries, removed: from [`drain`](FixedHashMap::drain). Dropping it before it is exhausted
/// removes the rest.
pub struct Drain<'a, K, V, St: AsMut<[Slot<K, V>]>, H> {
    map: &'a mut FixedHashMap<K, V, St, H>,
    next: usize,
}

impl<K, V, St, H> FixedHashMap<K, V, St, H>
where
    St: AsRef<[Slot<K, V>]>,
{
    /// The entries, in an arbitrary order.
    pub fn iter(&self) -> Iter<'_, K, V> {
        Iter {
            slots: self.slots.as_ref().iter(),
            left: self.len,
        }
    }

    /// The keys, in an arbitrary order.
    pub fn keys(&self) -> Keys<'_, K, V> {
        Keys(self.iter())
    }

    /// The values, in an arbitrary order.
    pub fn values(&self) -> Values<'_, K, V> {
        Values(self.iter())
    }
}

impl<K, V, St, H> FixedHashMap<K, V, St, H>
where
    St: AsRef<[Slot<K, V>]> + AsMut<[Slot<K, V>]>,
{
    /// The entries, with their values mutable, in an arbitrary order.
    pub fn iter_mut(&mut self) -> IterMut<'_, K, V> {
        IterMut {
            left: self.len,
            slots: self.slots.as_mut().iter_mut(),
        }
    }

    /// The values, mutable, in an arbitrary order.
    pub fn values_mut(&mut self) -> ValuesMut<'_, K, V> {
        ValuesMut(self.iter_mut())
    }

    /// Remove every entry, yielding each. The map is empty afterwards, whether or not the iterator
    /// was run to its end; its storage is kept.
    pub fn drain(&mut self) -> Drain<'_, K, V, St, H> {
        Drain { map: self, next: 0 }
    }
}

impl<K, V, St, H> FixedHashMap<K, V, St, H>
where
    K: Hash + Eq,
    St: AsRef<[Slot<K, V>]> + AsMut<[Slot<K, V>]>,
    H: BuildHasher,
{
    /// Keep only the entries `keep` answers `true` for, visiting each once.
    ///
    /// The walk starts just past an empty slot and goes once round the table. A probe run never
    /// crosses an empty slot, so a removal only shifts back entries the walk has not reached yet,
    /// and the slot it emptied is looked at again (FHT-D-4).
    pub fn retain<F: FnMut(&K, &mut V) -> bool>(&mut self, mut keep: F) {
        let count = self.slots.as_ref().len();
        let mask = count - 1;
        let Some(empty) = self.slots.as_ref().iter().position(Option::is_none) else {
            // Every slot is full only for a table of no slots, which cannot be made.
            return;
        };
        let mut step = 1;
        while step < count {
            let at = (empty + step) & mask;
            let keeps = match &mut self.slots.as_mut()[at] {
                None => true,
                Some((key, value)) => keep(key, value),
            };
            if keeps {
                step += 1;
            } else {
                self.remove_at(at);
            }
        }
    }
}

impl<'a, K, V> Iterator for Iter<'a, K, V> {
    type Item = (&'a K, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        let (key, value) = self.slots.find_map(Option::as_ref)?;
        self.left -= 1;
        Some((key, value))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.left, Some(self.left))
    }
}

impl<'a, K, V> Iterator for IterMut<'a, K, V> {
    type Item = (&'a K, &'a mut V);

    fn next(&mut self) -> Option<Self::Item> {
        let (key, value) = self.slots.find_map(Option::as_mut)?;
        self.left -= 1;
        Some((&*key, value))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.left, Some(self.left))
    }
}

impl<'a, K, V> Iterator for Keys<'a, K, V> {
    type Item = &'a K;

    fn next(&mut self) -> Option<&'a K> {
        self.0.next().map(|(key, _)| key)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl<'a, K, V> Iterator for Values<'a, K, V> {
    type Item = &'a V;

    fn next(&mut self) -> Option<&'a V> {
        self.0.next().map(|(_, value)| value)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl<'a, K, V> Iterator for ValuesMut<'a, K, V> {
    type Item = &'a mut V;

    fn next(&mut self) -> Option<&'a mut V> {
        self.0.next().map(|(_, value)| value)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl<K, V, I: Iterator<Item = Slot<K, V>>> Iterator for IntoIter<I> {
    type Item = (K, V);

    fn next(&mut self) -> Option<(K, V)> {
        let entry = self.slots.find_map(|slot| slot)?;
        self.left -= 1;
        Some(entry)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.left, Some(self.left))
    }
}

impl<K, V, St: AsMut<[Slot<K, V>]>, H> Iterator for Drain<'_, K, V, St, H> {
    type Item = (K, V);

    fn next(&mut self) -> Option<(K, V)> {
        let slots = self.map.slots.as_mut();
        while self.next < slots.len() {
            let taken = slots[self.next].take();
            self.next += 1;
            if taken.is_some() {
                self.map.len -= 1;
                return taken;
            }
        }
        None
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.map.len, Some(self.map.len))
    }
}

impl<K, V, St: AsMut<[Slot<K, V>]>, H> Drop for Drain<'_, K, V, St, H> {
    fn drop(&mut self) {
        for slot in &mut self.map.slots.as_mut()[self.next..] {
            *slot = None;
        }
        self.map.len = 0;
    }
}

impl<K, V> ExactSizeIterator for Iter<'_, K, V> {}
impl<K, V> ExactSizeIterator for IterMut<'_, K, V> {}
impl<K, V> ExactSizeIterator for Keys<'_, K, V> {}
impl<K, V> ExactSizeIterator for Values<'_, K, V> {}
impl<K, V> ExactSizeIterator for ValuesMut<'_, K, V> {}
impl<K, V, I: Iterator<Item = Slot<K, V>>> ExactSizeIterator for IntoIter<I> {}
impl<K, V, St: AsMut<[Slot<K, V>]>, H> ExactSizeIterator for Drain<'_, K, V, St, H> {}

impl<K, V> FusedIterator for Iter<'_, K, V> {}
impl<K, V> FusedIterator for IterMut<'_, K, V> {}
impl<K, V> FusedIterator for Keys<'_, K, V> {}
impl<K, V> FusedIterator for Values<'_, K, V> {}
impl<K, V> FusedIterator for ValuesMut<'_, K, V> {}
impl<K, V, I: Iterator<Item = Slot<K, V>>> FusedIterator for IntoIter<I> {}
impl<K, V, St: AsMut<[Slot<K, V>]>, H> FusedIterator for Drain<'_, K, V, St, H> {}

impl<'a, K, V, St: AsRef<[Slot<K, V>]>, H> IntoIterator for &'a FixedHashMap<K, V, St, H> {
    type Item = (&'a K, &'a V);
    type IntoIter = Iter<'a, K, V>;

    fn into_iter(self) -> Iter<'a, K, V> {
        self.iter()
    }
}

impl<'a, K, V, St, H> IntoIterator for &'a mut FixedHashMap<K, V, St, H>
where
    St: AsRef<[Slot<K, V>]> + AsMut<[Slot<K, V>]>,
{
    type Item = (&'a K, &'a mut V);
    type IntoIter = IterMut<'a, K, V>;

    fn into_iter(self) -> IterMut<'a, K, V> {
        self.iter_mut()
    }
}

impl<K, V, St: IntoIterator<Item = Slot<K, V>>, H> IntoIterator for FixedHashMap<K, V, St, H> {
    type Item = (K, V);
    type IntoIter = IntoIter<St::IntoIter>;

    fn into_iter(self) -> IntoIter<St::IntoIter> {
        IntoIter {
            left: self.len,
            slots: self.slots.into_iter(),
        }
    }
}
