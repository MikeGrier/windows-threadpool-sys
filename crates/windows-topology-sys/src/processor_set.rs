// Copyright (c) 2026 Mike Grier
//! A set of logical processors spanning one or more Windows processor groups.

use std::collections::BTreeMap;

/// How many processors one group's mask can name: one bit per machine word
/// bit, not a hardcoded 64 -- a 32-bit target's `usize` mask is only 32 bits
/// wide, and hardcoding 64 there would let `insert`/`contains` silently
/// address bits past the mask's actual width.
pub(crate) const MAX_PROCESSORS_PER_GROUP: u32 = usize::BITS;

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
    /// Panics if `number` is [`MAX_PROCESSORS_PER_GROUP`] or greater: a
    /// processor group cannot hold more processors than a mask has bits.
    pub fn insert(&mut self, group: u16, number: u8) {
        assert!(
            u32::from(number) < MAX_PROCESSORS_PER_GROUP,
            "a processor group has at most {MAX_PROCESSORS_PER_GROUP} processors, got {number}"
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
        if u32::from(number) >= MAX_PROCESSORS_PER_GROUP {
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
            (0..MAX_PROCESSORS_PER_GROUP as u8)
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

/// The wire shape of one member of a [`ProcessorSet`]: `{"group":_,"number":_}`.
///
/// Deliberately duplicated from `domain::ProcessorId` rather than reused: this
/// module sits below `domain.rs` (D-2 in `DESIGN-NOTES.md` -- durable
/// enumeration below, interpretation above), and importing a type from the
/// higher layer purely to save two fields would invert that direction for a
/// serialization convenience.
#[cfg(feature = "serde")]
#[derive(serde::Serialize, serde::Deserialize)]
struct WireProcessor {
    group: u16,
    number: u8,
}

#[cfg(feature = "serde")]
impl serde::Serialize for ProcessorSet {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        let mut seq = serializer.serialize_seq(Some(self.len()))?;
        for (group, number) in self.iter() {
            seq.serialize_element(&WireProcessor { group, number })?;
        }
        seq.end()
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for ProcessorSet {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = Vec::<WireProcessor>::deserialize(deserializer)?;
        let mut set = Self::empty();
        for entry in wire {
            if u32::from(entry.number) >= MAX_PROCESSORS_PER_GROUP {
                // A real Windows GROUP_AFFINITY.Mask is one machine word, so a
                // group genuinely cannot hold this processor: reject rather
                // than silently truncate (D-10 in `DESIGN-NOTES.md`).
                return Err(serde::de::Error::custom(format!(
                    "processor number {} is out of range: a processor group has at most \
                     {MAX_PROCESSORS_PER_GROUP} processors on this platform",
                    entry.number
                )));
            }
            set.insert(entry.group, entry.number);
        }
        Ok(set)
    }
}

#[cfg(test)]
mod tests;
