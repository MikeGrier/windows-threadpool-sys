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
    /// One domain per last-level (L3) cache -- the default heuristic
    /// `DESIGN-NOTES.md` recommends.
    ByL3,
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
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "byl3" | "l3" => Some(Self::ByL3),
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
    /// reporting zero NUMA nodes -- the same "one ring is correct when the
    /// answer is unknowable" degradation `DESIGN-NOTES.md` describes for L3
    /// domains, generalized to every policy here (M7.5 depends on knowing
    /// when this happened, to report it honestly rather than silently).
    #[must_use]
    pub fn select(self, topology: &MachineMemoryTopology) -> (Vec<Domain>, bool) {
        let matched: Vec<Domain> = match self {
            Self::Single => Vec::new(),
            Self::ByL3 => topology
                .domains
                .iter()
                .filter(|domain| matches!(domain.kind, DomainKind::Cache { level: 3, .. }))
                .cloned()
                .collect(),
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
                id: 0,
                processors,
            }],
            degraded,
        )
    }
}
