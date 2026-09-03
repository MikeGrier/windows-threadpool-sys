// Copyright (c) 2026 Mike Grier
//! Who reported a relation, and what they called it.
//!
//! Windows describes processor structure through two APIs that overlap without
//! agreeing on names, so "which source said this" is a fact the model has to
//! carry rather than infer. See D-15 and D-18 in `DESIGN-NOTES.md`.

/// Which platform API reported something.
///
/// Not a trust ordering. Both sources are cheap reads of the running system
/// and neither is more authoritative than the other -- where they disagree,
/// [D-15](../DESIGN-NOTES.md) keeps both rather than picking a winner, and
/// [D-17](../DESIGN-NOTES.md) expects genuine disagreement in the field. Trust
/// in the *object* is [`Provenance`](crate::Provenance), which is a different
/// question (D-22): where this says who spoke, that says how the collection
/// happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
#[non_exhaustive]
pub enum Source {
    /// `GetLogicalProcessorInformationEx` -- the relationship walk.
    RelationshipWalk,
    /// `GetSystemCpuSetInformation` -- the CPU-set enumeration.
    CpuSets,
    /// A description a caller merged in, which named no platform API.
    ///
    /// **Not** what deserialization produces: a restored relation carries no
    /// observation at all, because the object's
    /// [`Provenance`](crate::Provenance) already records that it came from a
    /// file, and restating it here would duplicate a fact D-22 has just
    /// separated. This variant is for the *mixed* case -- a caller adding
    /// described relations to a topology that was discovered -- which is
    /// exactly the case per-relation provenance exists to make visible.
    Description,
}

/// One source's report of a relation, carrying that source's own label for it.
///
/// # Why the label lives here and not on the relation
///
/// [D-15](../DESIGN-NOTES.md) identifies a relation by `(kind, membership)`,
/// because that is what the sources agree about. What they do **not** agree
/// about is naming: measured on the development host, the two report the same
/// eight-group core partition while labelling it `[0, 2, 4, ..., 14]` and
/// `[0, 1, ..., 7]`. Neither numbering is wrong and neither is a claim about
/// the machine, so a single `id` on the relation would have to pick one
/// arbitrarily and discard the other.
///
/// Putting the label on the observation removes the choice: one relation, two
/// observations, each keeping what its source called it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Observation {
    /// Which API reported this relation.
    pub source: Source,
    /// What that source called it.
    ///
    /// A NUMA node number, a processor group number, a CPU-set `CoreIndex`, or
    /// -- where the source numbers nothing -- the position it was reported in.
    /// Meaningful only alongside [`Self::source`], never on its own.
    pub label: u32,
}

impl Observation {
    /// An observation by `source`, labelled `label`.
    #[must_use]
    pub fn new(source: Source, label: u32) -> Self {
        Self { source, label }
    }
}

#[cfg(test)]
mod tests;
