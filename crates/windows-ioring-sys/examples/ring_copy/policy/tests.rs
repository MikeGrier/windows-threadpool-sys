// Copyright (c) 2026 Mike Grier
//! Tests for [`Policy::select`]'s degraded fallback (M20.3).
//!
//! # Why these exist
//!
//! The whole-machine fallback is the branch **every zero-relation machine
//! takes**, and it cannot be reached by running the sample on a machine that
//! reports its relations. A synthetic topology reaches it; until M20.3 nothing
//! did, and the first confirmation it ran at all came from a design session
//! rather than from a test.
//!
//! # Both directions, because one alone proves nothing
//!
//! The item that queued these asked for two halves on purpose: that a policy
//! whose relation is **absent** degrades, and that a policy whose relation is
//! **present** does not. The second is what makes the first mean anything --
//! a test of the absent case alone passes just as happily against a `select`
//! that degrades unconditionally. [`degrading_unconditionally_would_fail_a_test_here`]
//! states that dependency in code rather than leaving it to this comment.
//!
//! # Why no case uses `ByL3`
//!
//! `SH-4.12` is queued to rewrite that arm: it replaces the hardcoded
//! `level: 3` match with `outermost_partitioning_cache()`, and renames the
//! policy, because `byl3` is a user-facing CLI value that would stop
//! describing what it does. Asserting through `ByL3` here would pin behaviour
//! that is about to change.
//!
//! The fallback tail itself is *shared by every policy* and is not what that
//! item touches, so exercising it through `ByNode` and `ByPackage` tests the
//! same branch without standing in the way. `ByL3`'s own degradation
//! condition belongs to `SH-4.12`'s verification, where the rule it degrades
//! on is the new one.

use std::collections::BTreeMap;

use windows_topology_sys::{
    CacheKind, Domain, DomainKind, MachineMemoryTopology, Observed, Processor, ProcessorId,
    ProcessorSet,
};

use super::Policy;

/// A processor that is present and usable.
fn online(group: u16, number: u8) -> Processor {
    Processor {
        id: ProcessorId { group, number },
        online: true,
        capacity: 1,
    }
}

/// A processor slot Windows reserved but that is not usable -- a group can
/// carry these up to its maximum count, and the fallback must not offer them
/// to a ring.
fn offline(group: u16, number: u8) -> Processor {
    Processor {
        id: ProcessorId { group, number },
        online: false,
        capacity: 0,
    }
}

fn processors(ids: &[(u16, u8)]) -> ProcessorSet {
    let mut set = ProcessorSet::empty();
    for &(group, number) in ids {
        set.insert(group, number);
    }
    set
}

fn domain(kind: DomainKind, ids: &[(u16, u8)]) -> Domain {
    Domain {
        kind,
        processors: processors(ids),
        observations: Vec::new(),
    }
}

fn package(ids: &[(u16, u8)]) -> Domain {
    domain(DomainKind::Package, ids)
}

fn memory(ids: &[(u16, u8)]) -> Domain {
    // NotObserved, not Known(0): nothing measured this synthetic node's
    // size, and claiming it has zero bytes would be the fabricated record
    // Observed exists to prevent.
    domain(
        DomainKind::Memory {
            memory_bytes: Observed::NotObserved,
        },
        ids,
    )
}

fn core(ids: &[(u16, u8)]) -> Domain {
    domain(
        DomainKind::Core {
            simultaneous_multithreading: false,
            efficiency_class: 0,
        },
        ids,
    )
}

fn cache(level: u8, ids: &[(u16, u8)]) -> Domain {
    domain(
        DomainKind::Cache {
            level,
            associativity: 0,
            line_size: 0,
            size_bytes: 0,
            cache_type: CacheKind::Unified,
        },
        ids,
    )
}

/// A machine with four online processors and whichever relations are given.
fn machine(domains: Vec<Domain>) -> MachineMemoryTopology {
    MachineMemoryTopology {
        processors: vec![online(0, 0), online(0, 1), online(0, 2), online(0, 3)],
        domains,
        ..Default::default()
    }
}

/// Whether `domains` is the single whole-machine domain the fallback builds.
fn is_whole_machine(domains: &[Domain]) -> bool {
    domains.len() == 1
        && matches!(&domains[0].kind, DomainKind::Other { name, .. } if name == "whole-machine")
}

// ---------------------------------------------------------------- absent ---

#[test]
fn a_policy_whose_relation_is_absent_degrades_to_one_whole_machine_domain() {
    // The zero-NUMA-node machine, which D-48 records as an ordinary consumer
    // shape rather than an exotic one.
    let topology = machine(vec![package(&[(0, 0), (0, 1), (0, 2), (0, 3)])]);
    let (domains, degraded) = Policy::ByNode.select(&topology);

    assert!(
        degraded,
        "a relation nobody reported must be reported as a fallback"
    );
    assert!(
        is_whole_machine(&domains),
        "and the fallback is one whole-machine domain, not zero domains"
    );
}

#[test]
fn a_machine_with_no_relations_at_all_degrades_for_every_relational_policy() {
    let topology = machine(Vec::new());
    for policy in [Policy::ByNode, Policy::ByPackage, Policy::ByCore] {
        let (domains, degraded) = policy.select(&topology);
        assert!(degraded, "{policy:?} has no relation to select on");
        assert!(is_whole_machine(&domains), "{policy:?}");
    }
}

#[test]
fn a_memory_domain_with_no_processors_does_not_count_as_a_node() {
    // A memory-only domain is legal (D-5) and carries no processors, so it
    // cannot host a ring. Selecting on it would produce a domain with nothing
    // to pin a thread to, which is worse than degrading honestly.
    let topology = machine(vec![memory(&[])]);
    let (domains, degraded) = Policy::ByNode.select(&topology);

    assert!(degraded, "a node with no processors is not a usable domain");
    assert!(is_whole_machine(&domains));
}

// --------------------------------------------------------------- present ---

#[test]
fn a_policy_whose_relation_is_present_is_not_degraded() {
    // The half that makes the absent cases mean something.
    let topology = machine(vec![package(&[(0, 0), (0, 1)]), package(&[(0, 2), (0, 3)])]);
    let (domains, degraded) = Policy::ByPackage.select(&topology);

    assert!(
        !degraded,
        "the relation was reported, so nothing was degraded"
    );
    assert_eq!(domains.len(), 2, "and every matching domain is returned");
    assert!(
        domains
            .iter()
            .all(|d| matches!(d.kind, DomainKind::Package)),
        "the domains returned are the ones asked for"
    );
}

#[test]
fn every_relational_policy_is_undegraded_when_its_own_relation_is_present() {
    let topology = machine(vec![
        memory(&[(0, 0), (0, 1), (0, 2), (0, 3)]),
        package(&[(0, 0), (0, 1), (0, 2), (0, 3)]),
        core(&[(0, 0), (0, 1)]),
        core(&[(0, 2), (0, 3)]),
    ]);
    for policy in [Policy::ByNode, Policy::ByPackage, Policy::ByCore] {
        let (domains, degraded) = policy.select(&topology);
        assert!(
            !degraded,
            "{policy:?} asked for a relation this machine has"
        );
        assert!(
            !is_whole_machine(&domains),
            "{policy:?} must return its own domains, not the fallback"
        );
    }
}

#[test]
fn one_policy_degrading_does_not_degrade_another_on_the_same_machine() {
    // Degradation is per-policy, not a property of the machine. A host that
    // reports packages but no nodes must answer differently to the two.
    let topology = machine(vec![package(&[(0, 0), (0, 1), (0, 2), (0, 3)])]);

    let (_, node_degraded) = Policy::ByNode.select(&topology);
    let (_, package_degraded) = Policy::ByPackage.select(&topology);

    assert!(node_degraded, "no memory domain was reported");
    assert!(!package_degraded, "but a package domain was");
}

// ------------------------------------------------- the guard on the guard ---

#[test]
fn degrading_unconditionally_would_fail_a_test_here() {
    // The dependency between the two halves, stated in code. If `select` ever
    // degrades unconditionally, at least one assertion in this file must go
    // red -- so this names the case that would catch it rather than trusting
    // that some other test happens to.
    let topology = machine(vec![package(&[(0, 0), (0, 1), (0, 2), (0, 3)])]);
    let (domains, degraded) = Policy::ByPackage.select(&topology);
    assert!(
        !degraded && !is_whole_machine(&domains),
        "an unconditional degrade is indistinguishable from a correct one \
         unless some case asserts the undegraded outcome"
    );
}

// ----------------------------------------------------- ByCache (SH-4.12) ---

#[test]
fn by_cache_selects_the_level_that_partitions_not_the_level_numbered_three() {
    // The regression this policy was rewritten for, and it is not
    // hypothetical: measured on the machine this repository is developed on,
    // which reports an L3 spanning all 16 processors and a real 8-way L2
    // partition underneath it. The old `level: 3` filter found that L3,
    // returned ONE whole-machine domain, and -- because it matched something
    // -- did not flag the result degraded. It reported success while
    // collapsing an 8-domain machine to a single ring.
    let topology = machine(vec![
        cache(2, &[(0, 0), (0, 1)]),
        cache(2, &[(0, 2), (0, 3)]),
        cache(3, &[(0, 0), (0, 1), (0, 2), (0, 3)]),
    ]);
    let (domains, degraded) = Policy::ByCache.select(&topology);

    assert!(!degraded);
    assert_eq!(
        domains.len(),
        2,
        "L2 partitions this machine; L3 covers all of it and partitions nothing"
    );
    assert!(
        domains
            .iter()
            .all(|d| matches!(d.kind, DomainKind::Cache { level: 2, .. })),
        "the level chosen is the one that splits the machine, not the larger number"
    );
}

#[test]
fn by_cache_degrades_when_the_only_cache_spans_the_whole_machine() {
    // A cache every processor shares offers no boundary to size a ring by, so
    // the honest answer is one ring *reported as degraded* -- not one ring
    // reported as a cache-aware partition, which is what the old filter did.
    let topology = machine(vec![cache(3, &[(0, 0), (0, 1), (0, 2), (0, 3)])]);
    let (domains, degraded) = Policy::ByCache.select(&topology);

    assert!(degraded, "one block is not a partition");
    assert!(is_whole_machine(&domains));
}

#[test]
fn by_cache_degrades_when_no_cache_is_reported_at_all() {
    let topology = machine(vec![package(&[(0, 0), (0, 1), (0, 2), (0, 3)])]);
    let (domains, degraded) = Policy::ByCache.select(&topology);

    assert!(degraded);
    assert!(is_whole_machine(&domains));
}

#[test]
fn by_cache_works_on_a_machine_whose_outermost_partition_is_l2() {
    // D-48's shape: a shipping ARM part with no L3 at all, whose natural
    // cluster boundary is L2. The old filter returned zero domains here and
    // degraded; this returns the partition that exists.
    let topology = machine(vec![
        cache(2, &[(0, 0), (0, 1)]),
        cache(2, &[(0, 2), (0, 3)]),
    ]);
    let (domains, degraded) = Policy::ByCache.select(&topology);

    assert!(
        !degraded,
        "this machine has a cache partition, it is just not L3"
    );
    assert_eq!(domains.len(), 2);
}

#[test]
fn by_cache_is_not_spelled_byl3_any_more() {
    // The rename is the point, not a side effect: `byl3` named a rule this
    // sample no longer implements, and accepting it would let a script keep
    // asking for L3 and keep believing it got L3.
    assert_eq!(Policy::parse("bycache"), Some(Policy::ByCache));
    assert_eq!(Policy::parse("cache"), Some(Policy::ByCache));
    assert_eq!(Policy::parse("byl3"), None, "byl3 must not silently map on");
    assert_eq!(Policy::parse("l3"), None, "nor l3");
}

// ------------------------------------------------------- Single, and edges ---

#[test]
fn single_returns_the_whole_machine_without_calling_it_degraded() {
    // The edge that a naive "returned the whole machine, so it degraded"
    // implementation gets wrong. `Single` asks for one domain, so one domain
    // is exactly what it wanted -- degrading is a statement about not getting
    // what was asked for.
    let topology = machine(vec![package(&[(0, 0), (0, 1), (0, 2), (0, 3)])]);
    let (domains, degraded) = Policy::Single.select(&topology);

    assert!(is_whole_machine(&domains));
    assert!(!degraded, "Single got precisely what it asked for");
}

#[test]
fn single_is_undegraded_even_on_a_machine_with_no_relations() {
    let topology = machine(Vec::new());
    let (domains, degraded) = Policy::Single.select(&topology);

    assert!(is_whole_machine(&domains));
    assert!(
        !degraded,
        "Single never depends on a relation being reported"
    );
}

#[test]
fn the_fallback_domain_covers_every_online_processor() {
    let topology = machine(Vec::new());
    let (domains, _) = Policy::ByNode.select(&topology);

    let covered = &domains[0].processors;
    for number in 0..4u8 {
        assert!(
            covered.contains(0, number),
            "processor {number} is online and must be in the fallback domain"
        );
    }
}

#[test]
fn the_fallback_domain_excludes_offline_processors() {
    // The other direction of the same rule. A reserved-but-absent slot counts
    // toward a group's maximum, and handing one to a ring would pin a thread
    // to a processor that does not exist.
    let topology = MachineMemoryTopology {
        processors: vec![online(0, 0), offline(0, 1), online(0, 2), offline(0, 3)],
        domains: Vec::new(),
        ..Default::default()
    };
    let (domains, degraded) = Policy::ByNode.select(&topology);

    assert!(degraded);
    let covered = &domains[0].processors;
    assert!(
        covered.contains(0, 0) && covered.contains(0, 2),
        "the online ones"
    );
    assert!(
        !covered.contains(0, 1) && !covered.contains(0, 3),
        "and not the reserved slots"
    );
}

#[test]
fn the_fallback_domain_spans_processor_groups() {
    // A machine above 64 logical processors reports more than one group, and
    // the fallback is meant to be the *whole* machine.
    let topology = MachineMemoryTopology {
        processors: vec![online(0, 0), online(0, 1), online(1, 0), online(1, 1)],
        domains: Vec::new(),
        ..Default::default()
    };
    let (domains, _) = Policy::ByNode.select(&topology);

    let covered = &domains[0].processors;
    assert!(
        covered.contains(0, 0) && covered.contains(1, 1),
        "both groups"
    );
}

#[test]
fn the_fallback_domain_carries_no_observations() {
    // Nothing observed this relation -- the sample built it. Claiming an
    // observation would be exactly the fabricated record `Observed` exists to
    // prevent.
    let topology = machine(Vec::new());
    let (domains, _) = Policy::ByNode.select(&topology);

    assert!(
        domains[0].observations.is_empty(),
        "a hand-built relation must not claim a source reported it"
    );
}

#[test]
fn a_policy_name_round_trips_through_parse() {
    for (name, expected) in [
        ("bycache", Policy::ByCache),
        ("cache", Policy::ByCache),
        ("bynode", Policy::ByNode),
        ("node", Policy::ByNode),
        ("bypackage", Policy::ByPackage),
        ("package", Policy::ByPackage),
        ("bycore", Policy::ByCore),
        ("core", Policy::ByCore),
        ("single", Policy::Single),
    ] {
        assert_eq!(Policy::parse(name), Some(expected), "for {name}");
        assert_eq!(
            Policy::parse(&name.to_ascii_uppercase()),
            Some(expected),
            "parsing is case-insensitive, for {name}"
        );
    }
    assert_eq!(Policy::parse("nonesuch"), None);
    assert_eq!(Policy::parse(""), None);
}

#[test]
fn an_unreported_attribute_map_is_empty_on_the_fallback() {
    let topology = machine(Vec::new());
    let (domains, _) = Policy::ByNode.select(&topology);

    match &domains[0].kind {
        DomainKind::Other { name, attributes } => {
            assert_eq!(name, "whole-machine");
            assert_eq!(
                *attributes,
                BTreeMap::new(),
                "nothing was measured about it"
            );
        }
        other => panic!("the fallback must be an Other domain, got {other:?}"),
    }
}
