// Copyright (c) 2026 Mike Grier
use super::*;

#[test]
fn discover_succeeds_on_the_host() {
    // The narrowest possible claim: the call itself works. Everything else
    // below reasons about the shape of what it returned.
    discover().expect("GetLogicalProcessorInformationEx should succeed on any real Windows host");
}

#[test]
fn every_group_relation_processor_count_matches_its_processor_set() {
    let relations = discover().expect("discover");
    for group in &relations.groups {
        assert_eq!(
            group.active_processors.len(),
            group.active_processor_count as usize,
            "group {} reported {} active processors but its set has {}",
            group.group,
            group.active_processor_count,
            group.active_processors.len()
        );
    }
}

#[test]
fn every_core_processor_is_a_member_of_some_active_group() {
    let relations = discover().expect("discover");
    let all_active: ProcessorSet = relations
        .groups
        .iter()
        .fold(ProcessorSet::empty(), |acc, g| {
            acc.union(&g.active_processors)
        });

    for core in &relations.cores {
        for (group, number) in core.processors.iter() {
            assert!(
                all_active.contains(group, number),
                "core processor ({group}, {number}) is not a member of any reported group"
            );
        }
    }
}

#[test]
fn every_numa_node_processor_is_a_member_of_some_active_group() {
    let relations = discover().expect("discover");
    let all_active: ProcessorSet = relations
        .groups
        .iter()
        .fold(ProcessorSet::empty(), |acc, g| {
            acc.union(&g.active_processors)
        });

    for node in &relations.numa_nodes {
        for (group, number) in node.processors.iter() {
            assert!(
                all_active.contains(group, number),
                "NUMA node {} processor ({group}, {number}) is not a member of any reported group",
                node.node_number
            );
        }
    }
}

#[test]
fn every_cache_processor_is_a_member_of_some_active_group() {
    let relations = discover().expect("discover");
    let all_active: ProcessorSet = relations
        .groups
        .iter()
        .fold(ProcessorSet::empty(), |acc, g| {
            acc.union(&g.active_processors)
        });

    for cache in &relations.caches {
        for (group, number) in cache.processors.iter() {
            assert!(
                all_active.contains(group, number),
                "level-{} cache processor ({group}, {number}) is not a member of any reported group",
                cache.level
            );
        }
    }
}

#[test]
fn at_least_one_processor_core_and_one_package_are_reported() {
    // A real machine always has at least one core and one package; an empty
    // result here would mean the walk silently dropped every record.
    let relations = discover().expect("discover");
    assert!(!relations.cores.is_empty(), "no processor cores reported");
    assert!(
        !relations.packages.is_empty(),
        "no processor packages reported"
    );
    assert!(!relations.groups.is_empty(), "no processor groups reported");
}

#[test]
fn core_from_reads_the_smt_flag() {
    let smt_on = walk::ProcessorBody {
        flags: super::LTP_PC_SMT,
        efficiency_class: 0,
        group_masks: vec![GroupAffinity {
            group: 0,
            mask: 0b11,
        }],
    };
    let smt_off = walk::ProcessorBody {
        flags: 0,
        efficiency_class: 0,
        group_masks: vec![],
    };
    assert!(core_from(smt_on).simultaneous_multithreading);
    assert!(!core_from(smt_off).simultaneous_multithreading);
}

#[test]
fn groups_from_assigns_group_numbers_by_array_index() {
    let body = walk::GroupBody {
        group_info: vec![
            walk::GroupInfo {
                maximum_processor_count: 64,
                active_processor_count: 2,
                active_processor_mask: 0b11,
            },
            walk::GroupInfo {
                maximum_processor_count: 32,
                active_processor_count: 1,
                active_processor_mask: 0b1,
            },
        ],
    };
    let relations = groups_from(body);
    assert_eq!(relations.len(), 2);
    assert_eq!(relations[0].group, 0);
    assert_eq!(relations[1].group, 1);
    assert_eq!(relations[1].active_processors.group_mask(1), 0b1);
}

#[test]
fn to_processor_set_unions_every_group_affinity() {
    let masks = vec![
        GroupAffinity {
            group: 0,
            mask: 0b1,
        },
        GroupAffinity {
            group: 2,
            mask: 0b110,
        },
    ];
    let set = to_processor_set(&masks);
    assert_eq!(set.group_mask(0), 0b1);
    assert_eq!(set.group_mask(2), 0b110);
    assert_eq!(set.len(), 3);
}
