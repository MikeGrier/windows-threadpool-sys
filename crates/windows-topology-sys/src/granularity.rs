// Copyright (c) 2026 Mike Grier
//! The granularity order: what a set of processors shares, ordered by
//! observed set inclusion.
//!
//! This is the model `M2+.2`, `M2+.3` and `M2+.4` call for, and the three are
//! one type because they cannot be separated: an order derived from inclusion
//! has to say what its top is and what it does when two elements do not nest.
//!
//! # Why inclusion, and not the level number
//!
//! [`DomainKind::Cache`](crate::DomainKind::Cache) carries a firmware `level`,
//! and ordering by it is the obvious thing to do. This crate does not, because
//! a level number is **asserted** by firmware while a membership is
//! **observed**, and asserted structure is what has bitten this crate before:
//! the ARM64 host that reports no L3 at all, and the consumer that swept a
//! hard-coded `1..=4`. Inclusion is checkable against the very sets Windows
//! reported, so it cannot disagree with them.
//!
//! It is also strictly more general. Inclusion orders a cache against a *core*
//! and against a *NUMA node*, which share no numbering with it and could not
//! otherwise be compared at all.

use crate::domain::Domain;
use crate::processor_set::ProcessorSet;
use crate::topology::MachineMemoryTopology;

/// One element of the granularity order: something a set of processors can
/// be observed to share.
///
/// Ordered by inclusion of the processors covered, never by any level number
/// (see the module documentation).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Granularity<'a> {
    /// One relation the platform reported.
    Relation(&'a Domain),
    /// **The machine** -- the order's top, and the reason a query over it is
    /// total.
    ///
    /// Two processors in one machine always share *something*: one address
    /// space, one scheduler, one memory system. Without an explicit top, a
    /// query about a pair that no reported relation covers would answer
    /// "nothing", and every caller would write the same empty-case branch for
    /// a cross-node pair.
    ///
    /// It is deliberately **not** a [`Domain`] in
    /// [`MachineMemoryTopology::domains`]: putting it there would claim the
    /// platform observed a relation it never reported.
    Machine,
}

impl<'a> Granularity<'a> {
    /// The relation this names, or `None` for [`Granularity::Machine`].
    #[must_use]
    pub fn relation(self) -> Option<&'a Domain> {
        match self {
            Self::Relation(domain) => Some(domain),
            Self::Machine => None,
        }
    }

    /// Whether this is the order's top.
    #[must_use]
    pub fn is_machine(self) -> bool {
        matches!(self, Self::Machine)
    }

    /// The processors this granularity covers.
    ///
    /// Takes the topology because [`Granularity::Machine`] has no membership
    /// of its own -- it is every processor the topology knows, which only the
    /// topology can supply.
    #[must_use]
    pub fn processors(self, topology: &MachineMemoryTopology) -> ProcessorSet {
        match self {
            Self::Relation(domain) => domain.processors.clone(),
            Self::Machine => topology.machine_processors(),
        }
    }
}

impl MachineMemoryTopology {
    /// Every processor this topology knows, online or not.
    ///
    /// The membership of [`Granularity::Machine`]. Offline processors are
    /// included on purpose: a slot that exists is still part of the machine,
    /// and excluding it would make a query naming it answer "nothing shared"
    /// rather than "the machine", which is the empty-case branch the top
    /// exists to remove.
    #[must_use]
    pub fn machine_processors(&self) -> ProcessorSet {
        let mut set = ProcessorSet::empty();
        for processor in &self.processors {
            set.insert(processor.id.group, processor.id.number);
        }
        set
    }

    /// The **minimal** granularities that cover every processor in `of` --
    /// the tightest things they all share.
    ///
    /// # The answer is a set, not one element
    ///
    /// Inclusion is a *partial* order, so two granularities can be
    /// incomparable: neither contains the other, and both are therefore
    /// minimal. It is almost always one element, but not by construction, and
    /// a caller that takes the first must say that is what it is doing.
    ///
    /// Equal memberships are the ordinary case of this. Measured on the
    /// development host, L1 arrives as a data cache *and* an instruction cache
    /// over the very same processors -- two relations, distinct in kind and
    /// attributes, tied in the order. Both are returned, because picking one
    /// would be arbitrary.
    ///
    /// # When the answer is the machine
    ///
    /// [`Granularity::Machine`] is returned exactly when **no** reported
    /// relation covers `of`. It is the fallback that makes this query total
    /// rather than an element that competes with observed relations, so it
    /// never appears alongside one.
    ///
    /// # When the answer is empty
    ///
    /// Totality holds over processors this topology knows. A set naming a
    /// processor it does not know is answered with an empty result rather than
    /// with the machine, because claiming the machine contains a processor it
    /// has never heard of would be an invention.
    #[must_use]
    pub fn minimal_shared(&self, of: &ProcessorSet) -> Vec<Granularity<'_>> {
        if !of.is_subset(&self.machine_processors()) {
            return Vec::new();
        }

        let covering: Vec<&Domain> = self
            .domains
            .iter()
            .filter(|domain| of.is_subset(&domain.processors))
            .collect();

        if covering.is_empty() {
            return vec![Granularity::Machine];
        }

        covering
            .iter()
            .filter(|candidate| {
                // Minimal means nothing else covering `of` sits strictly
                // inside it. Strict is what keeps a tie -- two relations over
                // identical sets do not exclude each other, so both survive.
                !covering.iter().any(|other| {
                    other.processors.is_subset(&candidate.processors)
                        && other.processors != candidate.processors
                })
            })
            .map(|domain| Granularity::Relation(domain))
            .collect()
    }

    /// Whether `finer` sits strictly inside `coarser` in the granularity
    /// order.
    ///
    /// The order's comparison, exposed so a caller can position an answer
    /// against another without re-deriving what "tighter" means -- the
    /// re-derivation this model exists to stop.
    #[must_use]
    pub fn is_finer_than(&self, finer: Granularity<'_>, coarser: Granularity<'_>) -> bool {
        let finer = finer.processors(self);
        let coarser = coarser.processors(self);
        finer.is_subset(&coarser) && finer != coarser
    }
}

#[cfg(test)]
mod tests;
