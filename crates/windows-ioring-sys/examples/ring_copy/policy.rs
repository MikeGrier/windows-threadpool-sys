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
    /// Every policy, for `--compare` to price each in turn.
    ///
    /// Listed here rather than derived, because a sample that silently gained
    /// an arm when a variant was added would change what a recorded comparison
    /// means without anyone choosing that. [`Policy::listed_exactly_once`]
    /// makes the list impossible to get wrong in either direction.
    pub const ALL: [Policy; 5] = [
        Policy::ByCache,
        Policy::ByNode,
        Policy::ByPackage,
        Policy::ByCore,
        Policy::Single,
    ];

    /// Where this policy sits in [`Policy::ALL`] -- and the build-time proof
    /// that it sits there exactly once.
    ///
    /// The `match` is exhaustive, so adding a variant fails to compile until it
    /// is given an arm here, which is the intentional update. Each arm is an
    /// inline `const` block, and those are evaluated at compile time whether
    /// or not anything calls this, so each fails the build unless its variant
    /// is in `ALL` exactly once. Between them, `ALL` can neither omit a
    /// variant, repeat one, nor silently gain one. (Stable Rust cannot count an
    /// enum's variants, so this is the route that reaches the build rung.)
    #[allow(
        dead_code,
        reason = "its value is the compile-time evaluation of its arms, not any call"
    )]
    const fn listed_exactly_once(self) -> usize {
        match self {
            Self::ByCache => const { position_in_all(Policy::ByCache) },
            Self::ByNode => const { position_in_all(Policy::ByNode) },
            Self::ByPackage => const { position_in_all(Policy::ByPackage) },
            Self::ByCore => const { position_in_all(Policy::ByCore) },
            Self::Single => const { position_in_all(Policy::Single) },
        }
    }

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

/// The index of `policy` in [`Policy::ALL`]; a compile error, when evaluated in
/// a `const` context, if it is absent or listed more than once.
const fn position_in_all(policy: Policy) -> usize {
    let mut found = None;
    let mut i = 0;
    while i < Policy::ALL.len() {
        if Policy::ALL[i] as u8 == policy as u8 {
            assert!(found.is_none(), "a policy is listed in Policy::ALL twice");
            found = Some(i);
        }
        i += 1;
    }
    match found {
        Some(index) => index,
        None => panic!("a policy is missing from Policy::ALL, so --compare would not price it"),
    }
}

#[cfg(test)]
mod tests;
