// Copyright (c) 2026 Mike Grier
//! Iteration over a set, and the set operations that need no growth (FHT-D-8): each is a lazy
//! iterator over two sets, or a test over them, and makes no set of its own. The operators that
//! would (`&a | &b` and the like) are left out, as `FromIterator` is: they would have to choose a
//! capacity or panic.

use super::FixedHashSet;
use crate::map::{self, Slot};
use core::hash::{BuildHasher, Hash};
use core::iter::{Chain, FusedIterator};

/// The values, shared: from [`iter`](FixedHashSet::iter).
pub struct SetIter<'a, T>(map::Keys<'a, T, ()>);

/// The values, taken: from the set's `into_iter`. `I` is its storage's own iterator.
pub struct SetIntoIter<I>(map::IntoIter<I>);

/// The values, removed: from [`drain`](FixedHashSet::drain). Dropping it before it is exhausted
/// removes the rest.
pub struct SetDrain<'a, T, St: AsMut<[Slot<T, ()>]>, H>(map::Drain<'a, T, (), St, H>);

/// The values in one set and not another: from [`difference`](FixedHashSet::difference).
pub struct Difference<'a, T, St, H> {
    values: SetIter<'a, T>,
    other: &'a FixedHashSet<T, St, H>,
}

/// The values in both of two sets: from [`intersection`](FixedHashSet::intersection).
pub struct Intersection<'a, T, St, H> {
    values: SetIter<'a, T>,
    other: &'a FixedHashSet<T, St, H>,
}

/// The values in exactly one of two sets: from
/// [`symmetric_difference`](FixedHashSet::symmetric_difference).
pub struct SymmetricDifference<'a, T, St, H, St2, H2>(
    Chain<Difference<'a, T, St2, H2>, Difference<'a, T, St, H>>,
);

/// The values in either of two sets, each once: from [`union`](FixedHashSet::union).
pub struct Union<'a, T, St, H>(Chain<SetIter<'a, T>, Difference<'a, T, St, H>>);

impl<T, St, H> FixedHashSet<T, St, H>
where
    St: AsRef<[Slot<T, ()>]>,
{
    /// The values, in an arbitrary order.
    pub fn iter(&self) -> SetIter<'_, T> {
        SetIter(self.map.keys())
    }
}

impl<T, St, H> FixedHashSet<T, St, H>
where
    St: AsRef<[Slot<T, ()>]> + AsMut<[Slot<T, ()>]>,
{
    /// Remove every value, yielding each. The set is empty afterwards, whether or not the iterator
    /// was run to its end; its storage is kept.
    pub fn drain(&mut self) -> SetDrain<'_, T, St, H> {
        SetDrain(self.map.drain())
    }
}

impl<T, St, H> FixedHashSet<T, St, H>
where
    T: Hash + Eq,
    St: AsRef<[Slot<T, ()>]>,
    H: BuildHasher,
{
    /// The values in this set and not in `other`, whatever `other`'s storage and hasher.
    pub fn difference<'a, St2, H2>(
        &'a self,
        other: &'a FixedHashSet<T, St2, H2>,
    ) -> Difference<'a, T, St2, H2> {
        Difference {
            values: self.iter(),
            other,
        }
    }

    /// The values in exactly one of this set and `other`.
    pub fn symmetric_difference<'a, St2, H2>(
        &'a self,
        other: &'a FixedHashSet<T, St2, H2>,
    ) -> SymmetricDifference<'a, T, St, H, St2, H2>
    where
        St2: AsRef<[Slot<T, ()>]>,
        H2: BuildHasher,
    {
        SymmetricDifference(self.difference(other).chain(other.difference(self)))
    }

    /// The values in both this set and `other`, found by walking this set and looking each up in
    /// `other`: call it on the smaller of the two to make fewer lookups.
    pub fn intersection<'a, St2, H2>(
        &'a self,
        other: &'a FixedHashSet<T, St2, H2>,
    ) -> Intersection<'a, T, St2, H2> {
        Intersection {
            values: self.iter(),
            other,
        }
    }

    /// The values in either this set or `other`, each once.
    pub fn union<'a, St2, H2>(&'a self, other: &'a FixedHashSet<T, St2, H2>) -> Union<'a, T, St, H>
    where
        St2: AsRef<[Slot<T, ()>]>,
        H2: BuildHasher,
    {
        Union(self.iter().chain(other.difference(self)))
    }

    /// Whether this set and `other` share no value.
    pub fn is_disjoint<St2, H2>(&self, other: &FixedHashSet<T, St2, H2>) -> bool
    where
        St2: AsRef<[Slot<T, ()>]>,
        H2: BuildHasher,
    {
        if self.len() <= other.len() {
            self.iter().all(|value| !other.contains(value))
        } else {
            other.iter().all(|value| !self.contains(value))
        }
    }

    /// Whether every value in this set is in `other`.
    pub fn is_subset<St2, H2>(&self, other: &FixedHashSet<T, St2, H2>) -> bool
    where
        St2: AsRef<[Slot<T, ()>]>,
        H2: BuildHasher,
    {
        self.len() <= other.len() && self.iter().all(|value| other.contains(value))
    }

    /// Whether every value in `other` is in this set.
    pub fn is_superset<St2, H2>(&self, other: &FixedHashSet<T, St2, H2>) -> bool
    where
        St2: AsRef<[Slot<T, ()>]>,
        H2: BuildHasher,
    {
        other.is_subset(self)
    }
}

impl<'a, T> Iterator for SetIter<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<&'a T> {
        self.0.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl<T, I: Iterator<Item = Slot<T, ()>>> Iterator for SetIntoIter<I> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        self.0.next().map(|(value, ())| value)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl<T, St: AsMut<[Slot<T, ()>]>, H> Iterator for SetDrain<'_, T, St, H> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        self.0.next().map(|(value, ())| value)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl<'a, T, St, H> Iterator for Difference<'a, T, St, H>
where
    T: Hash + Eq,
    St: AsRef<[Slot<T, ()>]>,
    H: BuildHasher,
{
    type Item = &'a T;

    fn next(&mut self) -> Option<&'a T> {
        let other = self.other;
        self.values.find(|value| !other.contains(*value))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, self.values.size_hint().1)
    }
}

impl<'a, T, St, H> Iterator for Intersection<'a, T, St, H>
where
    T: Hash + Eq,
    St: AsRef<[Slot<T, ()>]>,
    H: BuildHasher,
{
    type Item = &'a T;

    fn next(&mut self) -> Option<&'a T> {
        let other = self.other;
        self.values.find(|value| other.contains(*value))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, self.values.size_hint().1)
    }
}

impl<'a, T, St, H, St2, H2> Iterator for SymmetricDifference<'a, T, St, H, St2, H2>
where
    T: Hash + Eq,
    St: AsRef<[Slot<T, ()>]>,
    H: BuildHasher,
    St2: AsRef<[Slot<T, ()>]>,
    H2: BuildHasher,
{
    type Item = &'a T;

    fn next(&mut self) -> Option<&'a T> {
        self.0.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl<'a, T, St, H> Iterator for Union<'a, T, St, H>
where
    T: Hash + Eq,
    St: AsRef<[Slot<T, ()>]>,
    H: BuildHasher,
{
    type Item = &'a T;

    fn next(&mut self) -> Option<&'a T> {
        self.0.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl<T> ExactSizeIterator for SetIter<'_, T> {}
impl<T, I: Iterator<Item = Slot<T, ()>>> ExactSizeIterator for SetIntoIter<I> {}
impl<T, St: AsMut<[Slot<T, ()>]>, H> ExactSizeIterator for SetDrain<'_, T, St, H> {}

impl<T> FusedIterator for SetIter<'_, T> {}
impl<T, I: Iterator<Item = Slot<T, ()>>> FusedIterator for SetIntoIter<I> {}
impl<T, St: AsMut<[Slot<T, ()>]>, H> FusedIterator for SetDrain<'_, T, St, H> {}
impl<T: Hash + Eq, St: AsRef<[Slot<T, ()>]>, H: BuildHasher> FusedIterator
    for Difference<'_, T, St, H>
{
}
impl<T: Hash + Eq, St: AsRef<[Slot<T, ()>]>, H: BuildHasher> FusedIterator
    for Intersection<'_, T, St, H>
{
}
impl<T, St, H, St2, H2> FusedIterator for SymmetricDifference<'_, T, St, H, St2, H2>
where
    T: Hash + Eq,
    St: AsRef<[Slot<T, ()>]>,
    H: BuildHasher,
    St2: AsRef<[Slot<T, ()>]>,
    H2: BuildHasher,
{
}
impl<T: Hash + Eq, St: AsRef<[Slot<T, ()>]>, H: BuildHasher> FusedIterator for Union<'_, T, St, H> {}

impl<'a, T, St: AsRef<[Slot<T, ()>]>, H> IntoIterator for &'a FixedHashSet<T, St, H> {
    type Item = &'a T;
    type IntoIter = SetIter<'a, T>;

    fn into_iter(self) -> SetIter<'a, T> {
        self.iter()
    }
}

impl<T, St: IntoIterator<Item = Slot<T, ()>>, H> IntoIterator for FixedHashSet<T, St, H> {
    type Item = T;
    type IntoIter = SetIntoIter<St::IntoIter>;

    fn into_iter(self) -> SetIntoIter<St::IntoIter> {
        SetIntoIter(self.map.into_iter())
    }
}
