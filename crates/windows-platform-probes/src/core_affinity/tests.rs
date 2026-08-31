// Copyright (c) Mike Grier.

//! Tests for the placement classifier.
//!
//! These cover the pure logic -- classification and pair selection -- and
//! deliberately not the measurement, which needs two real cores and several
//! seconds. What is worth testing here is that the probe cannot silently
//! mislabel a pair, because every conclusion it prints is keyed on that label.

use super::{Placement, classify, representative_pairs};
use crate::fingerprint::ProcessorPlace;

/// A processor on its own physical core, which is the non-SMT case.
///
/// Single-node, matching every host measured so far; use [`on_node`] to move
/// one onto another NUMA node.
fn place(number: u8, efficiency_class: u8, cache_domain: Option<u32>) -> ProcessorPlace {
    ProcessorPlace {
        number,
        core: u32::from(number),
        efficiency_class,
        cache_domain,
        numa_node: 0,
    }
}

/// The same processor, relocated to another NUMA node.
fn on_node(place: ProcessorPlace, numa_node: u32) -> ProcessorPlace {
    ProcessorPlace { numa_node, ..place }
}

/// Two processors sharing one physical core: SMT siblings.
fn sibling(
    number: u8,
    core: u32,
    efficiency_class: u8,
    cache_domain: Option<u32>,
) -> ProcessorPlace {
    ProcessorPlace {
        number,
        core,
        efficiency_class,
        cache_domain,
        numa_node: 0,
    }
}

#[test]
fn same_cache_and_same_class_is_classified_as_such() {
    let a = place(0, 1, Some(0));
    let b = place(1, 1, Some(0));
    assert_eq!(classify(a, b), Placement::SameCacheSameClass);
}

#[test]
fn a_differing_class_alone_is_cross_class() {
    let a = place(0, 1, Some(0));
    let b = place(1, 0, Some(0));
    assert_eq!(classify(a, b), Placement::SameCacheCrossClass);
}

#[test]
fn a_differing_cache_alone_is_cross_cache() {
    let a = place(0, 1, Some(0));
    let b = place(6, 1, Some(1));
    assert_eq!(classify(a, b), Placement::CrossCacheSameClass);
}

#[test]
fn differing_in_both_is_cross_cross() {
    let a = place(0, 1, Some(0));
    let b = place(6, 0, Some(1));
    assert_eq!(classify(a, b), Placement::CrossCacheCrossClass);
}

#[test]
fn classification_is_symmetric_in_the_two_ends() {
    // The placement describes a relationship, so naming which end produces must
    // not change it. The measurement may well differ by direction -- a fast
    // producer feeding a slow consumer is not the same experiment as the
    // reverse -- but that is a difference in the result, not in the label.
    let a = place(0, 1, Some(0));
    let b = place(6, 0, Some(1));
    assert_eq!(classify(a, b), classify(b, a));
}

#[test]
fn a_machine_with_no_partitioning_cache_still_classifies() {
    // Both `None`, which compares equal: a machine whose caches do not divide
    // it has every pair in the "same cache" category rather than in none.
    let a = place(0, 1, None);
    let b = place(1, 0, None);
    assert_eq!(classify(a, b), Placement::SameCacheCrossClass);
}

#[test]
fn a_homogeneous_single_cache_machine_offers_only_one_placement() {
    let places: Vec<_> = (0..4).map(|n| place(n, 0, Some(0))).collect();
    let pairs = representative_pairs(&places);

    assert_eq!(
        pairs.keys().copied().collect::<Vec<_>>(),
        vec![Placement::SameCacheSameClass],
        "a machine that cannot express a placement must report it absent, not \
         fabricate one"
    );
}

#[test]
fn a_heterogeneous_two_cache_machine_offers_all_four() {
    // A machine whose classes and caches cut differently, which is exactly what
    // the development host is NOT -- there the two coincide and only two of the
    // four placements exist. This case is what the probe would need to separate
    // the cache effect from the core-speed one.
    let mut places = Vec::new();
    let mut number = 0_u8;
    for cache in 0..2_u32 {
        for class in 0..2_u8 {
            for _ in 0..2 {
                places.push(place(number, class, Some(cache)));
                number += 1;
            }
        }
    }
    let pairs = representative_pairs(&places);

    assert_eq!(pairs.len(), 4, "all four placements must be expressible");
    for (placement, (producer, consumer)) in pairs {
        assert_eq!(
            classify(producer, consumer),
            placement,
            "the pair chosen for a placement must actually be that placement"
        );
    }
}

#[test]
fn a_machine_whose_classes_follow_its_caches_offers_only_two() {
    // The development host's shape: processors 0-5 are class 0 in cache domain
    // 0, and 6-11 are class 1 in cache domain 1. The two factors are perfectly
    // confounded, so the probe must report the mixed placements as absent
    // rather than inventing a pair for them -- "this host cannot test that" and
    // "that made no difference" are opposite findings.
    let mut places = Vec::new();
    for number in 0..12_u8 {
        let side = u32::from(number) / 6;
        places.push(place(number, side as u8, Some(side)));
    }
    let pairs = representative_pairs(&places);

    let mut found: Vec<_> = pairs.keys().copied().collect();
    found.sort_unstable();
    assert_eq!(
        found,
        vec![
            Placement::SameCacheSameClass,
            Placement::CrossCacheCrossClass
        ],
        "confounded classes and caches must yield exactly the two pure placements"
    );
}

#[test]
fn a_pair_never_puts_both_ends_on_one_processor() {
    let places: Vec<_> = (0..4)
        .map(|n| place(n, n % 2, Some(u32::from(n) / 2)))
        .collect();
    for (_, (producer, consumer)) in representative_pairs(&places) {
        assert_ne!(
            producer.number, consumer.number,
            "a queue with both ends on one core measures scheduling, not coherence"
        );
    }
}

#[test]
fn every_expressible_placement_is_chosen_exactly_once() {
    let places: Vec<_> = (0..8)
        .map(|n| place(n, n % 2, Some(u32::from(n) / 4)))
        .collect();
    let pairs = representative_pairs(&places);

    let mut labels: Vec<_> = pairs.keys().map(|p| p.label()).collect();
    labels.sort_unstable();
    let before = labels.len();
    labels.dedup();
    assert_eq!(before, labels.len(), "no placement may be measured twice");
}

#[test]
fn smt_siblings_are_their_own_placement() {
    // Two processors on one core share L1, which is a tighter coupling than
    // any cache domain expresses. Before this was distinguished, a sibling pair
    // and a two-core pair behind one cache landed in the same bucket, and the
    // probe reported whichever it happened to select first -- on an SMT host,
    // which is exactly where the distinction matters.
    let a = sibling(0, 0, 0, Some(0));
    let b = sibling(1, 0, 0, Some(0));
    assert_eq!(classify(a, b), Placement::SameCoreSiblings);
}

#[test]
fn siblings_outrank_the_cache_and_class_they_also_share() {
    let a = sibling(0, 0, 1, Some(3));
    let b = sibling(1, 0, 1, Some(3));
    assert_ne!(
        classify(a, b),
        Placement::SameCacheSameClass,
        "sharing a core must not be reported as merely sharing a cache"
    );
}

#[test]
fn an_smt_host_expresses_a_placement_a_non_smt_host_cannot() {
    // The 8C/16T homogeneous shape: two processors per core, one cache domain,
    // one efficiency class. It can express exactly two placements, and one of
    // them is unavailable on the non-SMT development host -- which is why the
    // two machines' results are not directly comparable.
    let mut places = Vec::new();
    for core in 0..8_u32 {
        for lane in 0..2_u8 {
            places.push(sibling(core as u8 * 2 + lane, core, 0, Some(0)));
        }
    }
    let pairs = representative_pairs(&places);

    let mut found: Vec<_> = pairs.keys().copied().collect();
    found.sort_unstable();
    assert_eq!(
        found,
        vec![Placement::SameCoreSiblings, Placement::SameCacheSameClass],
        "an SMT host must offer the sibling placement alongside the two-core one"
    );
}

#[test]
fn a_non_smt_host_cannot_express_the_sibling_placement() {
    let places: Vec<_> = (0..12)
        .map(|n| place(n, u8::from(n >= 6), Some(u32::from(n) / 6)))
        .collect();
    let pairs = representative_pairs(&places);

    assert!(
        !pairs.contains_key(&Placement::SameCoreSiblings),
        "a machine with one processor per core has no siblings to measure"
    );
}

#[test]
fn different_numa_nodes_are_classified_as_a_node_crossing() {
    let a = place(0, 1, Some(0));
    let b = on_node(place(1, 1, Some(1)), 1);

    assert_eq!(classify(a, b), Placement::CrossNumaNode);
}

#[test]
fn a_node_crossing_outranks_the_cache_and_class_it_also_crosses() {
    // The whole point of the variant: without it this pair reports as
    // `CrossCacheCrossClass` and the node crossing is invisible, so an
    // expensive run on a real NUMA machine would be recorded as a cache
    // effect.
    let a = place(0, 1, Some(0));
    let b = on_node(place(1, 0, Some(1)), 1);

    assert_eq!(classify(a, b), Placement::CrossNumaNode);
}

#[test]
fn a_node_crossing_is_reported_even_when_cache_and_class_match() {
    // Same cache domain id and same class on two different nodes is not a
    // configuration real hardware offers, but the classifier must not depend on
    // that: it decides on the node, not on the fields the node happens to
    // correlate with.
    let a = place(0, 1, Some(0));
    let b = on_node(place(1, 1, Some(0)), 1);

    assert_eq!(classify(a, b), Placement::CrossNumaNode);
}

#[test]
fn siblings_outrank_a_node_crossing_because_one_core_cannot_span_nodes() {
    // Ordering check rather than a hardware claim. `SameCoreSiblings` is tested
    // before the node, so if a topology ever reported one core on two nodes the
    // sibling relationship would win. Pinning the order down here means a later
    // reordering of `classify` is caught by a test rather than by a confusing
    // table on a machine nobody has yet run.
    let a = sibling(0, 0, 1, Some(0));
    let b = on_node(sibling(1, 0, 1, Some(0)), 1);

    assert_eq!(classify(a, b), Placement::SameCoreSiblings);
}

#[test]
fn a_single_node_machine_never_produces_a_node_crossing() {
    // Every host measured so far is a VM slice presenting one node, so this is
    // the case that must stay quiet: the new variant must not appear where it
    // cannot apply.
    let places: Vec<_> = (0..8)
        .map(|number| place(number, u8::from(number < 4), Some(u32::from(number) / 2)))
        .collect();

    let pairs = representative_pairs(&places);

    assert!(
        !pairs.contains_key(&Placement::CrossNumaNode),
        "a single-node machine reported a node crossing: {:?}",
        pairs.keys().collect::<Vec<_>>()
    );
}

#[test]
fn a_two_node_machine_expresses_the_node_crossing() {
    let places: Vec<_> = (0..8)
        .map(|number| {
            let base = place(number, 1, Some(u32::from(number) / 2));
            on_node(base, u32::from(number) / 4)
        })
        .collect();

    let pairs = representative_pairs(&places);

    assert!(
        pairs.contains_key(&Placement::CrossNumaNode),
        "a two-node machine did not express a node crossing: {:?}",
        pairs.keys().collect::<Vec<_>>()
    );
    let (producer, consumer) = pairs[&Placement::CrossNumaNode];
    assert_ne!(
        producer.numa_node, consumer.numa_node,
        "the pair chosen for a node crossing is on one node"
    );
}
