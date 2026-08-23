// Copyright (c) 2026 Mike Grier
use super::*;
use crate::domain::{Domain, DomainKind};
use crate::processor_set::ProcessorSet;
use crate::relation::CacheKind;

fn synthetic() -> Topology {
    let group0 = ProcessorSet::from_group_mask(0, 0b11);
    Topology {
        processors: vec![
            Processor {
                id: ProcessorId {
                    group: 0,
                    number: 0,
                },
                online: true,
                capacity: 0,
            },
            Processor {
                id: ProcessorId {
                    group: 0,
                    number: 1,
                },
                online: true,
                capacity: 10,
            },
        ],
        domains: vec![
            Domain {
                kind: DomainKind::Group,
                id: 0,
                processors: group0.clone(),
            },
            Domain {
                kind: DomainKind::Package,
                id: 0,
                processors: group0.clone(),
            },
            Domain {
                kind: DomainKind::Core {
                    simultaneous_multithreading: true,
                    efficiency_class: 10,
                },
                id: 0,
                processors: group0.clone(),
            },
            Domain {
                kind: DomainKind::Cache {
                    level: 3,
                    associativity: 16,
                    line_size: 64,
                    size_bytes: 32 * 1024 * 1024,
                    cache_type: CacheKind::Unified,
                },
                id: 0,
                processors: group0.clone(),
            },
            Domain {
                kind: DomainKind::Cache {
                    level: 2,
                    associativity: 8,
                    line_size: 64,
                    size_bytes: 512 * 1024,
                    cache_type: CacheKind::Unified,
                },
                id: 1,
                processors: group0,
            },
            Domain {
                kind: DomainKind::Memory { memory_bytes: None },
                id: 0,
                processors: ProcessorSet::empty(),
            },
        ],
        distances: None,
    }
}

#[test]
fn groups_returns_only_group_domains() {
    assert_eq!(synthetic().groups().count(), 1);
}

#[test]
fn packages_returns_only_package_domains() {
    assert_eq!(synthetic().packages().count(), 1);
}

#[test]
fn cores_returns_only_core_domains() {
    assert_eq!(synthetic().cores().count(), 1);
}

#[test]
fn caches_returns_every_cache_regardless_of_level() {
    assert_eq!(synthetic().caches().count(), 2);
}

#[test]
fn caches_at_level_filters_to_exactly_that_level() {
    let topo = synthetic();
    assert_eq!(topo.caches_at_level(3).count(), 1);
    assert_eq!(topo.caches_at_level(2).count(), 1);
    assert_eq!(topo.caches_at_level(1).count(), 0);
}

#[test]
fn memory_domains_includes_one_with_no_processors() {
    let topo = synthetic();
    let memory: Vec<_> = topo.memory_domains().collect();
    assert_eq!(memory.len(), 1);
    assert!(memory[0].processors.is_empty());
}

#[test]
fn processor_looks_up_by_id() {
    let topo = synthetic();
    let found = topo
        .processor(ProcessorId {
            group: 0,
            number: 1,
        })
        .expect("processor exists");
    assert_eq!(found.capacity, 10);
    assert!(
        topo.processor(ProcessorId {
            group: 9,
            number: 9
        })
        .is_none()
    );
}

#[test]
fn discover_succeeds_and_every_online_processor_is_in_some_group() {
    let topo = Topology::discover().expect("discover");
    assert!(!topo.processors.is_empty());
    assert!(topo.groups().count() >= 1);

    let all_group_processors: ProcessorSet = topo
        .groups()
        .fold(ProcessorSet::empty(), |acc, g| acc.union(&g.processors));
    for processor in topo.processors.iter().filter(|p| p.online) {
        assert!(
            all_group_processors.contains(processor.id.group, processor.id.number),
            "online processor {:?} is not a member of any group's processor set",
            processor.id
        );
    }
}

#[test]
fn discover_reports_a_processor_entry_for_every_slot_up_to_each_groups_maximum() {
    let topo = Topology::discover().expect("discover");
    let relations = crate::relation::discover().expect("discover relations");
    let expected: usize = relations
        .groups
        .iter()
        .map(|g| g.maximum_processor_count as usize)
        .sum();
    assert_eq!(topo.processors.len(), expected);
}
