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

use crate::domain::{Domain, DomainKind, ProcessorId};
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

    /// What a set of processors shares, and whether the answer is complete.
    ///
    /// The pairwise proximity query, generalised to any set because nothing in
    /// it is specific to two: `proximity(&[a, b])` is the pair case, and the
    /// same call sizes an MPSC fan-in over a whole block.
    ///
    /// # It is a helper over the order, not the primary surface
    ///
    /// The body is [`Self::minimal_shared`] plus the coverage check below. That
    /// is deliberate: the *collection* is what a caller reaching for structure
    /// should use, because rebuilding the grouping from repeated pairwise calls
    /// takes union-find over `O(n^2)` questions -- and that reconstruction is
    /// exactly what `SH-16.9` records three consumers doing, two of them
    /// differently. Providing the helper here means there is one implementation
    /// of the grouping rather than one per caller.
    ///
    /// # Errors
    ///
    /// None -- the query is total over processors this topology knows, and
    /// answers [`Granularity::Machine`] rather than "nothing" for a pair no
    /// relation covers. A processor it does not know yields an empty
    /// [`Proximity::shared`], because claiming the machine contains a processor
    /// it has never heard of would be an invention.
    #[must_use]
    pub fn proximity(&self, processors: &[ProcessorId]) -> Proximity<'_> {
        let mut set = ProcessorSet::empty();
        for id in processors {
            set.insert(id.group, id.number);
        }
        Proximity {
            shared: self.minimal_shared(&set),
            finer_unobserved: processors
                .iter()
                .any(|id| !self.kinds_covering(*id).is_empty()),
        }
    }

    /// The relation kinds this machine reports that `processor` appears in
    /// **no** instance of.
    ///
    /// Evidence of a gap in what the platform said, not of a fact about the
    /// machine. A kind is only counted when some *other* processor is covered
    /// by it, so a machine that reports no caches at all does not make every
    /// answer an upper bound -- that is a complete description of a machine
    /// without caches, and treating it as incomplete would be the same
    /// conflation [D-13](../DESIGN-NOTES.md) exists to remove.
    fn kinds_covering(&self, processor: ProcessorId) -> Vec<&'static str> {
        let mut missing = Vec::new();
        for kind in Self::REPORTED_KINDS {
            let mut kind_exists = false;
            let mut covers = false;
            for domain in &self.domains {
                if Self::kind_name(&domain.kind) != kind {
                    continue;
                }
                kind_exists = true;
                if domain
                    .processors
                    .contains(processor.group, processor.number)
                {
                    covers = true;
                    break;
                }
            }
            if kind_exists && !covers {
                missing.push(kind);
            }
        }
        missing
    }

    /// The kinds a coverage gap is meaningful for.
    ///
    /// Deliberately not every [`DomainKind`]: `Memory` is excluded because a
    /// processor in no memory domain is `M4+.3`'s *unplaced* case, which has
    /// its own answer and is not evidence that something finer went unreported.
    const REPORTED_KINDS: [&'static str; 5] = ["cache", "core", "module", "die", "package"];

    fn kind_name(kind: &DomainKind) -> &'static str {
        match kind {
            DomainKind::Group => "group",
            DomainKind::Package => "package",
            DomainKind::Die => "die",
            DomainKind::Module => "module",
            DomainKind::Core { .. } => "core",
            DomainKind::Cache { .. } => "cache",
            DomainKind::Memory { .. } => "memory",
            DomainKind::Other { .. } => "other",
        }
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

/// What a set of processors shares, and how far the answer can be trusted.
///
/// The answer to [`MachineMemoryTopology::proximity`], and the reason it is a
/// struct rather than a bare list: a caller needs to know not only what was
/// found but whether something finer might exist and simply was not reported.
#[derive(Clone, Debug, PartialEq)]
pub struct Proximity<'a> {
    /// The tightest granularities all the processors share.
    ///
    /// A **set**, because inclusion is a partial order and two granularities
    /// can be incomparable -- almost always one element, but not by
    /// construction (M2+.4). Exactly [`Granularity::Machine`] when no reported
    /// relation covers them all, which is what makes the query total (M2+.3).
    pub shared: Vec<Granularity<'a>>,
    /// Whether a **finer** granularity may exist that was never reported.
    ///
    /// When true, [`Self::shared`] is an **upper bound** rather than the
    /// answer: the machine describes some kind of relation that one of these
    /// processors appears in *no* instance of, so the platform has said nothing
    /// about whether they share it.
    ///
    /// This is the third of [EP-D-2]'s requirements, and the one a naive design
    /// drops. A caller told "the tightest shared thing is L3" when in truth L2
    /// was never reported for this processor would choose a slower channel than
    /// the machine can support and never learn why -- and under this crate's own
    /// bar it cannot go and measure to find out.
    ///
    /// [EP-D-2]: ../../topology-planner/DESIGN-NOTES.md
    pub finer_unobserved: bool,
}

impl<'a> Proximity<'a> {
    /// The single shared granularity, when there is exactly one.
    ///
    /// `None` when the answer is a tie, so a caller that cannot handle one is
    /// forced to say so rather than silently taking the first.
    #[must_use]
    pub fn only(&self) -> Option<Granularity<'a>> {
        match self.shared.as_slice() {
            [single] => Some(*single),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;
