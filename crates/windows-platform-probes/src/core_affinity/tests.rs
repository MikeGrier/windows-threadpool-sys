// Copyright (c) Mike Grier.

//! Tests for the placement classifier.
//!
//! These cover the pure logic -- classification and pair selection -- and
//! deliberately not the measurement, which needs two real cores and several
//! seconds. What is worth testing here is that the probe cannot silently
//! mislabel a pair, because every conclusion it prints is keyed on that label.

use super::{Placement, ProcessorPlace, classify, representative_pairs};

fn place(number: u8, efficiency_class: u8, cache_domain: Option<u32>) -> ProcessorPlace {
    ProcessorPlace {
        number,
        efficiency_class,
        cache_domain,
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
