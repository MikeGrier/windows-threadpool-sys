// Copyright (c) 2026 Mike Grier
//! Policy -> domain selection (M7.1): named code, not data.

use windows_topology_sys::{Domain, DomainKind, MachineMemoryTopology, ProcessorSet};

/// How to partition the machine into `IoRing` execution domains (M7.1).
///
/// Named code rather than a data-driven partitioning scheme: the library
/// deliberately owns no partitioning policy (D-8 in the crate's
/// `DESIGN-NOTES.md`), so a small, fixed set of named strategies belongs
/// here, in the sample, not as extensible policy data nobody asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    /// One domain per **outermost cache level that actually partitions the
    /// machine** -- the default heuristic `DESIGN-NOTES.md` recommends.
    ///
    /// Not "L3". The rule is asked of
    /// [`MachineMemoryTopology::outermost_partitioning_cache`], which is the
    /// one definition of which level partitions a host, and *which* level that
    /// is varies: on EPYC it is the L3/CCX boundary, and on a shipping
    /// Snapdragon X2 Elite there is no L3 at all and the natural boundary is
    /// L2 ([D-48](../../DESIGN-NOTES.md#d-48)).
    ///
    /// This used to match `level: 3` here, which is the consumer-side twin of
    /// the platform-integrity failure: binding to the level number that
    /// happens to be right on today's hardware instead of to the specified
    /// primitive. It could also produce **overlapping** ring domains where two
    /// cache kinds are reported at the same level, which `select` has no way
    /// to detect and a ring runtime has no way to survive.
    ByCache,
    /// One domain per NUMA node.
    ByNode,
    /// One domain per physical package (socket).
    ByPackage,
    /// One domain per physical core (not per SMT sibling).
    ByCore,
    /// One domain covering the whole machine.
    Single,
}

impl Policy {
    /// Parse a policy name (case-insensitive), for the sample's `--policy` switch.
    ///
    /// `byl3` and `l3` are deliberately **not** accepted. They named a rule
    /// this sample no longer implements, and silently mapping them onto
    /// [`Policy::ByCache`] would let a script keep asking for L3 and keep
    /// believing it got L3 -- on a machine whose partitioning level is L2,
    /// that is a wrong answer delivered quietly. An unknown name prints the
    /// usage line instead, which is a question rather than a wrong answer.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "bycache" | "cache" => Some(Self::ByCache),
            "bynode" | "node" => Some(Self::ByNode),
            "bypackage" | "package" => Some(Self::ByPackage),
            "bycore" | "core" => Some(Self::ByCore),
            "single" => Some(Self::Single),
            _ => None,
        }
    }

    /// The domains this policy selects out of `topology`, and whether the
    /// result is a degraded fallback rather than what was actually asked for.
    ///
    /// Degrades to one whole-machine domain when the policy's preferred
    /// relation is not reported at all -- for example `ByNode` on a machine
    /// reporting zero NUMA nodes, or [`Policy::ByCache`] on one whose caches
    /// partition nothing -- the same "one ring is correct when the answer is
    /// unknowable" degradation `DESIGN-NOTES.md` describes, generalized to
    /// every policy here (M7.5 depends on knowing when this happened, to
    /// report it honestly rather than silently).
    #[must_use]
    pub fn select(self, topology: &MachineMemoryTopology) -> (Vec<Domain>, bool) {
        let matched: Vec<Domain> = match self {
            Self::Single => Vec::new(),
            // Asked, not restated. `outermost_partitioning_cache` owns the
            // rule -- including that "outermost" is decided by inclusion
            // rather than by the level number, and that a level qualifies only
            // when its blocks are pairwise disjoint. Both are properties this
            // consumer would otherwise have to re-derive, and the second is
            // the one whose absence let the old code emit overlapping domains.
            Self::ByCache => topology
                .outermost_partitioning_cache()
                .map(|(_level, domains)| domains.into_iter().cloned().collect())
                .unwrap_or_default(),
            Self::ByNode => topology
                .domains
                .iter()
                .filter(|domain| {
                    matches!(domain.kind, DomainKind::Memory { .. })
                        && !domain.processors.is_empty()
                })
                .cloned()
                .collect(),
            Self::ByPackage => topology
                .domains
                .iter()
                .filter(|domain| matches!(domain.kind, DomainKind::Package))
                .cloned()
                .collect(),
            Self::ByCore => topology
                .domains
                .iter()
                .filter(|domain| matches!(domain.kind, DomainKind::Core { .. }))
                .cloned()
                .collect(),
        };

        if self != Self::Single && !matched.is_empty() {
            return (matched, false);
        }

        let mut processors = ProcessorSet::empty();
        for processor in &topology.processors {
            if processor.online {
                processors.insert(processor.id.group, processor.id.number);
            }
        }
        let degraded = self != Self::Single;
        let kind = DomainKind::Other {
            name: "whole-machine".to_string(),
            attributes: Default::default(),
        };
        (
            vec![Domain {
                kind,
                processors,
                observations: Vec::new(),
            }],
            degraded,
        )
    }
}

#[cfg(test)]
mod tests;
