// Copyright (c) Mike Grier.
use super::*;
use windows_topology_sys::{Domain, Observation, Observed, Processor};

fn processor(number: u8) -> ProcessorId {
    ProcessorId { group: 0, number }
}

fn domain(kind: DomainKind, ids: &[ProcessorId], label: Option<u32>) -> Domain {
    Domain {
        kind,
        processors: ids.iter().map(|id| (id.group, id.number)).collect(),
        observations: label
            .map(|label| Observation::new(Source::RelationshipWalk, label))
            .into_iter()
            .collect(),
    }
}

fn machine(count: u8) -> MachineMemoryTopology {
    MachineMemoryTopology {
        processors: (0..count)
            .map(|number| Processor {
                id: processor(number),
                online: true,
                capacity: 0,
            })
            .collect(),
        ..Default::default()
    }
}

fn core(ids: &[ProcessorId]) -> Domain {
    domain(
        DomainKind::Core {
            simultaneous_multithreading: ids.len() > 1,
            efficiency_class: 0,
        },
        ids,
        None,
    )
}

fn memory(ids: &[ProcessorId], node: Option<u32>) -> Domain {
    domain(
        DomainKind::Memory {
            memory_bytes: Observed::NotObserved,
        },
        ids,
        node,
    )
}

fn cache(ids: &[ProcessorId], level: u8, cache_type: CacheKind) -> Domain {
    domain(
        DomainKind::Cache {
            level,
            associativity: 8,
            line_size: 64,
            size_bytes: 1024,
            cache_type,
        },
        ids,
        None,
    )
}

#[test]
fn synthetic_relationships_are_not_a_total_locality_order() {
    for scenario in 0..12 {
        let mut topology = machine(4);
        let ids: Vec<_> = topology
            .processors
            .iter()
            .map(|processor| processor.id)
            .collect();
        topology.domains.extend([core(&ids[..2]), core(&ids[2..])]);
        if scenario % 2 == 0 {
            topology.domains.push(memory(&ids, Some(7)));
        } else {
            topology
                .domains
                .extend([memory(&ids[..2], Some(7)), memory(&ids[2..], Some(19))]);
        }
        topology.domains.extend([
            cache(&ids[..2], 2, CacheKind::Unified),
            cache(&ids[2..], 2, CacheKind::Unified),
        ]);
        if scenario % 3 == 0 {
            topology.domains.push(cache(&ids, 3, CacheKind::Unified));
        }
        let plan = placement_plan(&topology);
        assert!(
            plan.pairs
                .iter()
                .any(|pair| pair.core_relationship == Relationship::Shared)
        );
        assert!(
            plan.pairs
                .iter()
                .any(|pair| pair.core_relationship == Relationship::Separate)
        );
        assert_eq!(
            plan.pairs
                .iter()
                .any(|pair| pair.memory_relationship == Relationship::Separate),
            scenario % 2 != 0
        );
        for pair in &plan.pairs {
            assert!(pair.processors[0] < pair.processors[1]);
            assert!(pair.memory_nodes.iter().all(Option::is_some));
        }
        topology.processors.reverse();
        assert_eq!(
            serde_json::to_value(&plan.pairs).unwrap(),
            serde_json::to_value(placement_plan(&topology).pairs).unwrap()
        );
    }
}

#[test]
fn unknown_ambiguous_offline_and_group_boundaries_remain_visible() {
    let mut topology = machine(2);
    let plan = placement_plan(&topology);
    assert_eq!(plan.pairs[0].core_relationship, Relationship::Unknown);
    assert_eq!(plan.pairs[0].memory_nodes, [None, None]);
    assert!(!plan.unavailable.is_empty());
    let ids = [processor(0), processor(1)];
    topology
        .domains
        .extend([core(&ids), core(&ids[..1]), memory(&ids, None)]);
    assert_eq!(
        placement_plan(&topology).pairs[0].core_relationship,
        Relationship::Ambiguous
    );
    topology
        .domains
        .push(cache(&ids, 1, CacheKind::Instruction));
    assert!(placement_plan(&topology).pairs[0].caches.is_empty());
    topology.processors[1].online = false;
    assert!(placement_plan(&topology).pairs.is_empty());
    topology.processors[1].online = true;
    topology.processors[1].id = ProcessorId {
        group: 2,
        number: 0,
    };
    assert!(placement_plan(&topology).pairs.is_empty());
    topology.domains.clear();
    let plan = placement_plan(&topology);
    assert_eq!(plan.pairs[0].processors[1].group, 2);
    topology.processors[1].id.number = u8::MAX;
    assert!(placement_plan(&topology).pairs.is_empty());
    assert!(placement_plan(&machine(0)).pairs.is_empty());
    assert!(placement_plan(&machine(1)).pairs.is_empty());
}

#[test]
fn missing_node_label_never_becomes_domain_index_or_node_zero() {
    let mut topology = machine(2);
    topology.domains.extend([
        memory(&[processor(0)], Some(9)),
        memory(&[processor(1)], None),
    ]);
    let pair = placement_plan(&topology).pairs.remove(0);
    assert_eq!(pair.memory_relationship, Relationship::Separate);
    assert_eq!(pair.memory_nodes, [Some(9), None]);
}

#[test]
fn unknown_pairs_do_not_hide_usable_cross_node_pairs() {
    let mut topology = machine(3);
    topology.domains.extend([
        memory(&[processor(0)], None),
        memory(&[processor(1)], Some(9)),
        memory(&[processor(2)], Some(17)),
    ]);
    let plan = placement_plan(&topology);
    assert!(
        plan.pairs
            .iter()
            .any(|pair| pair.memory_nodes == [Some(9), Some(17)])
    );
    assert!(
        !plan
            .unavailable
            .iter()
            .any(|reason| reason.starts_with("cross-NUMA"))
    );
}
