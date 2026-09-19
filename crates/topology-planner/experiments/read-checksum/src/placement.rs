// Copyright (c) Mike Grier.
use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use windows_topology_sys::{
    CacheKind, DomainKind, MachineMemoryTopology, Observed, ProcessorId, Source,
};

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Relationship {
    Shared,
    Separate,
    Unknown,
    Ambiguous,
}

#[derive(Debug, Serialize)]
pub struct CacheComparison {
    pub level: u8,
    pub domains: [Vec<usize>; 2],
    pub relationship: Relationship,
}

#[derive(Debug, Serialize)]
pub struct PlacementPair {
    pub processors: [ProcessorId; 2],
    pub memory_nodes: [Option<u32>; 2],
    pub core_domains: [Vec<usize>; 2],
    pub core_relationship: Relationship,
    pub memory_relationship: Relationship,
    pub caches: Vec<CacheComparison>,
    pub efficiency_classes: [Observed<u8>; 2],
}

#[derive(Debug, Serialize)]
pub struct PlacementPlan {
    pub schema: &'static str,
    pub topology: MachineMemoryTopology,
    pub memory_nodes: BTreeSet<u32>,
    pub pairs: Vec<PlacementPair>,
    pub unavailable: Vec<&'static str>,
}

fn members(
    machine: &MachineMemoryTopology,
    pair: [ProcessorId; 2],
    predicate: impl Fn(&DomainKind) -> bool,
) -> [Vec<usize>; 2] {
    pair.map(|processor| {
        machine
            .domains
            .iter()
            .enumerate()
            .filter(|(_, domain)| {
                predicate(&domain.kind)
                    && domain
                        .processors
                        .contains(processor.group, processor.number)
            })
            .map(|(index, _)| index)
            .collect()
    })
}

fn relationship(domains: &[Vec<usize>; 2]) -> Relationship {
    if domains.iter().any(|set| set.len() > 1) {
        Relationship::Ambiguous
    } else if domains.iter().any(Vec::is_empty) {
        Relationship::Unknown
    } else if domains[0] == domains[1] {
        Relationship::Shared
    } else {
        Relationship::Separate
    }
}

fn data_cache(kind: &DomainKind) -> Option<u8> {
    match kind {
        DomainKind::Cache {
            level,
            cache_type: CacheKind::Data | CacheKind::Unified,
            ..
        } => Some(*level),
        _ => None,
    }
}

pub fn placement_plan(machine: &MachineMemoryTopology) -> PlacementPlan {
    let levels: BTreeSet<_> = machine
        .domains
        .iter()
        .filter_map(|domain| data_cache(&domain.kind))
        .collect();
    let mut processors: Vec<_> = machine
        .shard_set()
        .into_iter()
        .filter(|processor| processor.online && u32::from(processor.id.number) < usize::BITS)
        .collect();
    processors.sort_by_key(|processor| processor.id);
    let mut representatives = BTreeMap::new();
    for (index, first) in processors.iter().enumerate() {
        for second in &processors[index + 1..] {
            if first.id == second.id || first.efficiency_class != second.efficiency_class {
                continue;
            }
            let pair = [first.id, second.id];
            let core_domains = members(machine, pair, |kind| {
                matches!(kind, DomainKind::Core { .. })
            });
            let core_relationship = relationship(&core_domains);
            let memory_domains = members(machine, pair, |kind| {
                matches!(kind, DomainKind::Memory { .. })
            });
            let memory_relationship = relationship(&memory_domains);
            let memory_nodes = memory_domains.each_ref().map(|domains| {
                if domains.len() == 1 {
                    machine.domains[domains[0]].label_from(Source::RelationshipWalk)
                } else {
                    None
                }
            });
            let caches: Vec<_> = levels
                .iter()
                .map(|&level| {
                    let domains = members(machine, pair, |kind| data_cache(kind) == Some(level));
                    CacheComparison {
                        level,
                        relationship: relationship(&domains),
                        domains,
                    }
                })
                .collect();
            let key = (
                core_relationship,
                memory_relationship,
                memory_nodes.map(|node| node.is_some()),
                caches
                    .iter()
                    .map(|cache| (cache.level, cache.relationship))
                    .collect::<Vec<_>>(),
            );
            representatives.entry(key).or_insert(PlacementPair {
                processors: pair,
                memory_nodes,
                core_domains,
                core_relationship,
                memory_relationship,
                caches,
                efficiency_classes: [first.efficiency_class, second.efficiency_class],
            });
        }
    }
    let pairs: Vec<_> = representatives.into_values().collect();
    let mut unavailable = Vec::new();
    if pairs.is_empty() {
        unavailable.push("no active representable same-observed-class processor pair");
    }
    if !pairs
        .iter()
        .any(|pair| pair.core_relationship == Relationship::Shared)
    {
        unavailable.push("shared-core pair not observed");
    }
    if !pairs
        .iter()
        .any(|pair| pair.core_relationship == Relationship::Separate)
    {
        unavailable.push("distinct-core pair not established");
    }
    if levels.is_empty() {
        unavailable.push("data/unified cache relationships not observed");
    }
    if !pairs.iter().any(|pair| {
        pair.memory_relationship == Relationship::Separate
            && pair.memory_nodes.iter().all(Option::is_some)
            && pair.memory_nodes[0] != pair.memory_nodes[1]
    }) {
        unavailable
            .push("cross-NUMA pair with Windows node labels unavailable; cross-node timing unrun");
    }
    if pairs
        .iter()
        .any(|pair| pair.memory_nodes.iter().any(Option::is_none))
    {
        unavailable.push("some pairs lack unambiguous Windows memory-node labels; explicit placement unavailable for those endpoints");
    }
    PlacementPlan {
        schema: "read-checksum-placement-plan-v1",
        topology: machine.clone(),
        memory_nodes: crate::platform::memory_nodes(machine),
        pairs,
        unavailable,
    }
}

#[cfg(test)]
mod tests;
