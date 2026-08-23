// Copyright (c) 2026 Mike Grier
//! A set of logical processors spanning one or more Windows processor groups.

use std::collections::BTreeMap;

/// A set of logical processors, correctly spanning more than one processor
/// group.
///
/// A Windows thread's affinity is a `GROUP_AFFINITY` -- one group and a bitmask
/// within it -- because a group is a hard boundary: no single mask can name
/// processors in two different groups. A type that silently flattened to one
/// 64-bit mask would misrepresent any topology wider than one group, which is
/// exactly the class of bug this type exists to prevent. `ProcessorSet` keeps
/// each group's mask separate instead.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProcessorSet {
    groups: BTreeMap<u16, usize>,
}

impl ProcessorSet {
    /// A set naming no processors.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// A set naming every bit set in `mask` within `group`.
    #[must_use]
    pub fn from_group_mask(group: u16, mask: usize) -> Self {
        let mut set = Self::empty();
        if mask != 0 {
            set.groups.insert(group, mask);
        }
        set
    }

    /// Add `number` within `group` to the set.
    ///
    /// # Panics
    ///
    /// Panics if `number` is 64 or greater: a processor group has at most 64
    /// processors, since a mask is one machine word.
    pub fn insert(&mut self, group: u16, number: u8) {
        assert!(
            number < 64,
            "a processor group has at most 64 processors, got {number}"
        );
        *self.groups.entry(group).or_insert(0) |= 1_usize << number;
    }

    /// Whether the set names no processors.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    /// How many processors the set names, across every group.
    #[must_use]
    pub fn len(&self) -> usize {
        self.groups
            .values()
            .map(|mask| mask.count_ones() as usize)
            .sum()
    }

    /// Whether `group`/`number` is a member of this set.
    #[must_use]
    pub fn contains(&self, group: u16, number: u8) -> bool {
        if number >= 64 {
            return false;
        }
        self.groups
            .get(&group)
            .is_some_and(|mask| mask & (1_usize << number) != 0)
    }

    /// The raw mask for a single group, or `0` if the group has no members here.
    #[must_use]
    pub fn group_mask(&self, group: u16) -> usize {
        self.groups.get(&group).copied().unwrap_or(0)
    }

    /// Iterate over `(group, mask)` pairs, one per group with any member here,
    /// in ascending group order.
    pub fn group_masks(&self) -> impl Iterator<Item = (u16, usize)> + '_ {
        self.groups.iter().map(|(&group, &mask)| (group, mask))
    }

    /// Iterate over every `(group, number)` member, in ascending order.
    pub fn iter(&self) -> impl Iterator<Item = (u16, u8)> + '_ {
        self.groups.iter().flat_map(|(&group, &mask)| {
            (0..64_u8)
                .filter(move |&n| mask & (1_usize << n) != 0)
                .map(move |n| (group, n))
        })
    }

    /// The set of processors present in both `self` and `other`.
    #[must_use]
    pub fn intersection(&self, other: &Self) -> Self {
        let mut result = Self::empty();
        for (&group, &mask) in &self.groups {
            let combined = mask & other.groups.get(&group).copied().unwrap_or(0);
            if combined != 0 {
                result.groups.insert(group, combined);
            }
        }
        result
    }

    /// The set of processors present in either `self` or `other`.
    #[must_use]
    pub fn union(&self, other: &Self) -> Self {
        let mut result = self.clone();
        for (&group, &mask) in &other.groups {
            *result.groups.entry(group).or_insert(0) |= mask;
        }
        result
    }

    /// Whether `self` and `other` share no processors.
    #[must_use]
    pub fn is_disjoint(&self, other: &Self) -> bool {
        self.groups.iter().all(|(group, &mask)| {
            other
                .groups
                .get(group)
                .is_none_or(|&other_mask| mask & other_mask == 0)
        })
    }
}

impl FromIterator<(u16, u8)> for ProcessorSet {
    fn from_iter<T: IntoIterator<Item = (u16, u8)>>(iter: T) -> Self {
        let mut set = Self::empty();
        for (group, number) in iter {
            set.insert(group, number);
        }
        set
    }
}

#[cfg(test)]
mod tests;
