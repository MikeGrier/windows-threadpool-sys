// Copyright (c) Mike Grier.

//! Tests for the placement classifier.
//!
//! These cover the pure logic -- classification and pair selection -- and
//! deliberately not the measurement, which needs two real cores and several
//! seconds. What is worth testing here is that the probe cannot silently
//! mislabel a pair, because every conclusion it prints is keyed on that label.

use super::{Placement, RunPlan, classify, memory_placements, node_pairs, representative_pairs};
use crate::fingerprint::ProcessorPlace;

/// A processor on its own physical core, which is the non-SMT case.
///
/// Single-node, matching every host measured so far; use [`on_node`] to move
/// one onto another NUMA node.
fn place(number: u8, efficiency_class: u8, cache_domain: Option<u32>) -> ProcessorPlace {
    ProcessorPlace {
        group: 0,
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

/// The same processor, relocated to another processor group.
///
/// Its `number` is deliberately unchanged, because that is the configuration
/// that breaks a number-keyed implementation: two distinct processors sharing
/// one number across groups.
fn in_group(place: ProcessorPlace, group: u16) -> ProcessorPlace {
    ProcessorPlace { group, ..place }
}

/// Two processors sharing one physical core: SMT siblings.
fn sibling(
    number: u8,
    core: u32,
    efficiency_class: u8,
    cache_domain: Option<u32>,
) -> ProcessorPlace {
    ProcessorPlace {
        group: 0,
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

// ---------------------------------------------------------------------------
// Synthetic multi-node hosts.
//
// No machine available to this workspace has more than one NUMA node, so the
// classification and *selection* logic for a node crossing would otherwise
// first execute on scarce hardware, where a mis-selection reads as a surprising
// measurement rather than as a bug. These fixtures exercise that logic offline.
//
// What they can and cannot establish, stated plainly: `classify` and
// `representative_pairs` are pure functions of a processor list, so feeding
// them a synthetic list tests them exactly as the real thing would. The
// *timings* are not testable this way and are not attempted -- `measure` pins
// threads to real processors, and pinning to a processor that does not exist
// fails loudly rather than returning a fabricated number.
// ---------------------------------------------------------------------------

/// The shape of a machine, in the terms the classifier actually uses.
struct HostSpec {
    nodes: u32,
    /// Cache domains per node, at the outermost level that partitions the host.
    cache_domains_per_node: u32,
    cores_per_cache_domain: u32,
    threads_per_core: u32,
}

/// Build the processor list such a machine would present.
///
/// Deliberately small. The shape is what the classifier reads; a 128-processor
/// version would exercise the identical code paths and would not fit in `u8`.
fn synthesize(spec: &HostSpec) -> Vec<ProcessorPlace> {
    let mut places = Vec::new();
    let mut number = 0_u8;
    let mut core = 0_u32;
    let mut cache_domain = 0_u32;

    for node in 0..spec.nodes {
        for _ in 0..spec.cache_domains_per_node {
            for _ in 0..spec.cores_per_cache_domain {
                for _ in 0..spec.threads_per_core {
                    places.push(ProcessorPlace {
                        group: 0,
                        number,
                        core,
                        efficiency_class: 0,
                        cache_domain: Some(cache_domain),
                        numa_node: node,
                    });
                    number += 1;
                }
                core += 1;
            }
            cache_domain += 1;
        }
    }
    places
}

/// Assert that every chosen pair genuinely satisfies the placement it is filed
/// under.
///
/// This is the check that matters. A table with the right *row labels* and the
/// wrong *pairs* behind them is worse than a missing row, because it reports a
/// number for a placement that was never measured.
fn assert_pairs_are_faithful(places: &[ProcessorPlace]) {
    for (placement, (producer, consumer)) in representative_pairs(places) {
        assert_eq!(
            classify(producer, consumer),
            placement,
            "pair {producer} / {consumer} filed under {}",
            placement.label()
        );
        assert_ne!(
            producer.number, consumer.number,
            "a placement was measured against a single processor"
        );
        match placement {
            Placement::SameCoreSiblings => {
                assert_eq!(producer.core, consumer.core);
                assert_eq!(producer.numa_node, consumer.numa_node);
            }
            Placement::CrossNumaNode => {
                assert_ne!(producer.numa_node, consumer.numa_node);
            }
            // Every non-NUMA placement must stay inside one node, or it would
            // have classified as a node crossing instead.
            _ => assert_eq!(
                producer.numa_node,
                consumer.numa_node,
                "{} spans two NUMA nodes",
                placement.label()
            ),
        }
    }
}

/// A two-socket machine whose outermost partitioning cache sits *inside* each
/// node -- several cache domains per node, as an EPYC's CCX layout gives.
fn two_socket_many_cache_domains() -> Vec<ProcessorPlace> {
    synthesize(&HostSpec {
        nodes: 2,
        cache_domains_per_node: 2,
        cores_per_cache_domain: 2,
        threads_per_core: 2,
    })
}

/// A two-socket machine with a single cache domain per node, which is what a
/// classic server presents when its last-level cache is per-socket.
fn two_socket_one_cache_domain_per_node() -> Vec<ProcessorPlace> {
    synthesize(&HostSpec {
        nodes: 2,
        cache_domains_per_node: 1,
        cores_per_cache_domain: 4,
        threads_per_core: 2,
    })
}

#[test]
fn a_two_socket_host_expresses_siblings_both_cache_rows_and_the_node_crossing() {
    let places = two_socket_many_cache_domains();
    let pairs = representative_pairs(&places);
    let mut found: Vec<_> = pairs.keys().copied().collect();
    found.sort_unstable();

    assert_eq!(
        found,
        vec![
            Placement::SameCoreSiblings,
            Placement::SameCacheSameClass,
            Placement::CrossCacheSameClass,
            Placement::CrossNumaNode,
        ],
        "unexpected placement set for a two-socket host"
    );
    assert_pairs_are_faithful(&places);
}

#[test]
fn adding_a_second_node_does_not_cannibalise_the_cross_cache_row() {
    // The subtle regression the NUMA variant could have introduced: classifying
    // the node first must not swallow same-node cache crossings, which are a
    // different and still-interesting measurement.
    let one_node = synthesize(&HostSpec {
        nodes: 1,
        cache_domains_per_node: 2,
        cores_per_cache_domain: 2,
        threads_per_core: 2,
    });
    let two_nodes = two_socket_many_cache_domains();

    let single = representative_pairs(&one_node);
    let dual = representative_pairs(&two_nodes);

    assert!(single.contains_key(&Placement::CrossCacheSameClass));
    assert!(
        dual.contains_key(&Placement::CrossCacheSameClass),
        "the cross-cache row vanished once a second node existed"
    );
    assert!(!single.contains_key(&Placement::CrossNumaNode));
    assert!(dual.contains_key(&Placement::CrossNumaNode));
}

#[test]
fn one_cache_domain_per_node_has_no_cross_cache_row_at_all() {
    // Not a defect, and worth pinning down before a real multi-socket run makes
    // it look like one. When the outermost partitioning cache *is* the socket,
    // two cores either share it (same node) or are on different nodes -- so
    // "cross cache, same class" has no members and the node crossing is the
    // only way out of a cache domain. A future run on such a host will show
    // that row as inexpressible, and that is the correct reading.
    let places = two_socket_one_cache_domain_per_node();
    let pairs = representative_pairs(&places);
    let mut found: Vec<_> = pairs.keys().copied().collect();
    found.sort_unstable();

    assert_eq!(
        found,
        vec![
            Placement::SameCoreSiblings,
            Placement::SameCacheSameClass,
            Placement::CrossNumaNode,
        ]
    );
    assert_pairs_are_faithful(&places);
}

#[test]
fn a_node_crossing_is_expressible_without_smt() {
    // A multi-socket host with hyper-threading disabled, which is a common
    // server configuration and the one where a sibling-shaped assumption would
    // break.
    let places = synthesize(&HostSpec {
        nodes: 2,
        cache_domains_per_node: 1,
        cores_per_cache_domain: 4,
        threads_per_core: 1,
    });
    let pairs = representative_pairs(&places);

    assert!(!pairs.contains_key(&Placement::SameCoreSiblings));
    assert!(pairs.contains_key(&Placement::CrossNumaNode));
    assert_pairs_are_faithful(&places);
}

#[test]
fn a_four_node_host_still_reports_exactly_one_node_crossing_row() {
    // More than two nodes must not multiply the row: the probe measures one
    // representative pair per placement, and "cross NUMA" is one placement
    // however many nodes exist. Distance between specific nodes is a real
    // effect this deliberately does not model, and saying so here stops the
    // single row being over-read later.
    let places = synthesize(&HostSpec {
        nodes: 4,
        cache_domains_per_node: 1,
        cores_per_cache_domain: 2,
        threads_per_core: 1,
    });
    let pairs = representative_pairs(&places);

    assert_eq!(
        pairs
            .keys()
            .filter(|p| **p == Placement::CrossNumaNode)
            .count(),
        1
    );
    assert_pairs_are_faithful(&places);
}

// ---------------------------------------------------------------------------
// Inter-node distance selection.
//
// `CrossNumaNode` is one category however many nodes exist, so on a host with
// three or more it reports one hop and implies the rest are like it. Real
// multi-node hardware does not work that way. These cover the selection that
// measures each hop separately; as above, only selection is testable offline.
// ---------------------------------------------------------------------------

/// Every chosen pair must genuinely span the two nodes it is filed under, with
/// the producer on the node the key names first.
fn assert_node_pairs_are_faithful(places: &[ProcessorPlace]) {
    for ((from, to), (producer, consumer)) in node_pairs(places) {
        assert_ne!(from, to, "key ({from}, {to}) is not a crossing");
        assert_eq!(
            producer.numa_node, from,
            "producer {producer} is not on node {from}"
        );
        assert_eq!(
            consumer.numa_node, to,
            "consumer {consumer} is not on node {to}"
        );
        assert_eq!(
            classify(producer, consumer),
            Placement::CrossNumaNode,
            "a node pair did not classify as a node crossing"
        );
    }
}

#[test]
fn a_single_node_host_has_no_node_pairs() {
    let places = synthesize(&HostSpec {
        nodes: 1,
        cache_domains_per_node: 2,
        cores_per_cache_domain: 2,
        threads_per_core: 2,
    });

    assert!(
        node_pairs(&places).is_empty(),
        "a single-node host produced a node pair"
    );
}

#[test]
fn a_two_node_host_measures_its_one_edge_in_both_directions() {
    let places = two_socket_many_cache_domains();
    let pairs = node_pairs(&places);

    // Two rows for one edge: the producer on node 0 and the producer on
    // node 1 are different measurements of it.
    assert_eq!(pairs.len(), 2);
    assert!(pairs.contains_key(&(0, 1)));
    assert!(pairs.contains_key(&(1, 0)));
    assert_node_pairs_are_faithful(&places);
}

#[test]
fn a_four_node_host_measures_every_hop_in_both_directions() {
    // The whole reason this exists: twelve distinct hops -- six edges, each
    // measured both ways -- not one row standing in for all of them.
    let places = synthesize(&HostSpec {
        nodes: 4,
        cache_domains_per_node: 1,
        cores_per_cache_domain: 2,
        threads_per_core: 1,
    });
    let pairs = node_pairs(&places);

    let mut keys: Vec<_> = pairs.keys().copied().collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec![
            (0, 1),
            (0, 2),
            (0, 3),
            (1, 0),
            (1, 2),
            (1, 3),
            (2, 0),
            (2, 1),
            (2, 3),
            (3, 0),
            (3, 1),
            (3, 2),
        ]
    );
    assert_node_pairs_are_faithful(&places);
}

#[test]
fn node_pairs_are_directed_so_both_ends_take_a_turn_producing() {
    // The correction this replaced: an earlier version selected one direction
    // per edge, reasoning that both traverse the same link. The link is
    // symmetric; the workload is not. The producer writes and the consumer
    // reads, so `0 -> 1` and `1 -> 0` measure a remote write and a remote read
    // over that link, which are different quantities and on some interconnects
    // not close ones.
    let places = synthesize(&HostSpec {
        nodes: 3,
        cache_domains_per_node: 1,
        cores_per_cache_domain: 2,
        threads_per_core: 1,
    });
    let pairs = node_pairs(&places);

    assert_eq!(pairs.len(), 6);
    for (from, to) in pairs.keys() {
        assert!(
            pairs.contains_key(&(*to, *from)),
            "({from}, {to}) was selected but its reverse was not"
        );
    }
}

#[test]
fn the_hop_count_is_every_ordered_pair_of_distinct_nodes() {
    // A property rather than a fixture, so a host size nobody wrote a test for
    // is still covered. `n * (n - 1)`, not the triangular number: every ordered
    // pair, because order is what decides who writes.
    for nodes in 1..=8_u32 {
        let places = synthesize(&HostSpec {
            nodes,
            cache_domains_per_node: 1,
            cores_per_cache_domain: 1,
            threads_per_core: 1,
        });
        let expected = (nodes * nodes.saturating_sub(1)) as usize;

        assert_eq!(
            node_pairs(&places).len(),
            expected,
            "wrong hop count for {nodes} nodes"
        );
        assert_node_pairs_are_faithful(&places);
    }
}

#[test]
fn node_pair_selection_is_stable_across_calls() {
    // The producer is always on the lower-numbered node, so a run is
    // reproducible rather than dependent on enumeration order. Without this a
    // re-run could silently measure a different pair and the difference would
    // read as drift in the hardware.
    let places = synthesize(&HostSpec {
        nodes: 3,
        cache_domains_per_node: 2,
        cores_per_cache_domain: 2,
        threads_per_core: 2,
    });

    let first = node_pairs(&places);
    let second = node_pairs(&places);

    assert_eq!(first, second);
}

#[test]
fn a_node_pair_is_still_selected_when_the_nodes_are_not_numbered_from_zero() {
    // Node ids are opaque identifiers from the topology, not indices. A host
    // that reports nodes 2 and 5 must still produce the hop between them.
    let mut places = synthesize(&HostSpec {
        nodes: 2,
        cache_domains_per_node: 1,
        cores_per_cache_domain: 2,
        threads_per_core: 1,
    });
    for place in &mut places {
        place.numa_node = if place.numa_node == 0 { 2 } else { 5 };
    }

    let pairs = node_pairs(&places);

    assert_eq!(pairs.len(), 2);
    assert!(pairs.contains_key(&(2, 5)));
    assert!(pairs.contains_key(&(5, 2)));
    assert_node_pairs_are_faithful(&places);
}

// ---------------------------------------------------------------------------
// Processor groups.
//
// Windows splits a machine with more than 64 logical processors into groups,
// each numbering from zero, so every group has a processor 5. No host available
// to this workspace has more than one group, and the machines this tool is
// written for -- large multi-socket servers -- all do. These fixtures are the
// only way that path executes before it meets such a machine.
//
// The failure being guarded against is silent: numbers stay below 64 within a
// group, so no bound check fires. A number-keyed implementation simply reports
// fewer processors than the machine has and prints a confident table describing
// a topology that does not exist.
// ---------------------------------------------------------------------------

mod processor_groups {
    use super::{classify, in_group, node_pairs, place, representative_pairs, sibling};
    use crate::core_affinity::Placement;
    use crate::fingerprint::ProcessorPlace;

    /// Two groups of four processors whose numbers deliberately overlap.
    ///
    /// Group 0 and group 1 both contain numbers 0..4. A map keyed on the number
    /// alone keeps four of the eight.
    ///
    /// Cache domain ids are distinct per group, matching the real conversion:
    /// they come from a machine-wide enumeration, so two groups never share
    /// one. An earlier version of this fixture reused them across groups and
    /// thereby described a cache shared between processor groups, which no
    /// machine does.
    fn two_groups() -> Vec<ProcessorPlace> {
        let mut places = Vec::new();
        for group in 0..2_u16 {
            for number in 0..4_u8 {
                let domain = u32::from(group) * 2 + u32::from(number) / 2;
                let base = place(number, 0, Some(domain));
                places.push(in_group(base, group));
            }
        }
        places
    }

    #[test]
    fn processors_sharing_a_number_across_groups_are_distinct() {
        let places = two_groups();

        let ids: std::collections::BTreeSet<(u16, u8)> = places.iter().map(|p| p.id()).collect();

        assert_eq!(
            ids.len(),
            8,
            "two groups of four collapsed to {} distinct processors",
            ids.len()
        );
    }

    #[test]
    fn a_pair_sharing_a_number_across_groups_is_still_a_pair() {
        // The self-pair guard exists to stop a processor being measured against
        // itself. Written as `producer.number == consumer.number` it also
        // discards every pair whose two processors merely *share* a number in
        // different groups -- which on a two-group machine is a large fraction
        // of the cross-group pairs, and on a machine with more groups, more.
        //
        // Checked through `representative_pairs` rather than on the predicate,
        // because the predicate is private and the selection is what the run
        // consumes.
        // Two processors, one per group, both numbered 0 -- so the machine's
        // only possible pair is one the number-only guard would discard, and
        // the whole selection comes back empty. Deliberately minimal: on a
        // larger fixture the loss hides, because another cross-group pair with
        // differing numbers lands in the same category and fills it. That is
        // why this is not tested through `two_groups`, and why the defect
        // survived the group work: it is invisible unless the discarded pair is
        // the only representative of its placement.
        let mut here = place(0, 0, Some(0));
        here.numa_node = 0;
        let mut there = in_group(place(0, 0, Some(1)), 1);
        there.numa_node = 1;

        let pairs = representative_pairs(&[here, there]);

        assert_eq!(
            pairs.len(),
            1,
            "the only pair this machine can express was discarded: {pairs:?}"
        );
        let (producer, consumer) = pairs[&Placement::CrossNumaNode];
        assert_ne!(
            producer.id(),
            consumer.id(),
            "a placement measures one processor against itself"
        );
        assert_ne!(producer.group, consumer.group);
    }

    #[test]
    fn a_group_is_part_of_the_rendered_identity() {
        // The slice string is how a measurement's provenance travels into a
        // checklist or a submitted record. If it omits the group, two different
        // processors render identically and the record cannot be read back.
        let zero = place(5, 0, Some(0));
        let one = in_group(zero, 1);

        assert_ne!(zero.to_string(), one.to_string());
        assert!(zero.to_string().starts_with("g0/cpu5/"), "{zero}");
        assert!(one.to_string().starts_with("g1/cpu5/"), "{one}");
    }

    #[test]
    fn same_number_in_different_groups_is_not_the_same_core() {
        // `core` is what `SameCoreSiblings` is decided on, so if the fallback
        // core id were derived from the number alone, two processors in
        // different groups would be classified as SMT siblings -- physically
        // impossible, since a core cannot span a group.
        let zero = sibling(5, 5, 0, Some(0));
        let one = in_group(zero, 1);

        assert_ne!(
            classify(zero, one),
            Placement::SameCoreSiblings,
            "processors in different groups were classified as siblings of one core"
        );
    }

    #[test]
    fn selection_across_groups_files_every_pair_under_a_placement_it_satisfies() {
        // Note what is *not* asserted: that a pair is drawn from every group.
        // `representative_pairs` returns one pair per placement *category*, and
        // "in a different group" is not one -- a cross-group pair on one node
        // is an ordinary cross-cache pair. Requiring group coverage would be
        // asserting a promise the function does not make, and an earlier
        // revision of this test did exactly that.
        //
        // What must hold is that groups do not corrupt the classification: each
        // chosen pair genuinely satisfies the row it is filed under.
        let places = two_groups();

        for (placement, (producer, consumer)) in representative_pairs(&places) {
            assert_eq!(
                classify(producer, consumer),
                placement,
                "pair {producer} / {consumer} filed under {}",
                placement.label()
            );
            if placement == Placement::SameCoreSiblings {
                assert_eq!(
                    producer.group, consumer.group,
                    "siblings were selected across a group boundary"
                );
            }
        }
    }

    #[test]
    fn groups_do_not_by_themselves_imply_a_numa_crossing() {
        // A group boundary and a node boundary are different things, and
        // Windows may split a single node across groups. Classifying by group
        // would invent node crossings the machine does not have.
        let places = two_groups();
        let hops = node_pairs(&places);

        assert!(
            hops.is_empty(),
            "a single-node machine with two groups reported a node crossing: {hops:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The plan.
//
// The plan is printed before a run starts, and someone decides whether to spend
// their afternoon on the strength of it. It has been wrong twice: once quoting
// 18 timed handoffs against a run that performed 12, and once quoting a floor of
// 1 second against a run that took 0.6. Both had the same cause -- the plan
// counted independently of the loop it describes -- so these tests check the
// plan against the same functions the run asks.
// ---------------------------------------------------------------------------

/// What the hop loop will do, derived rather than restated.
fn expected_hop_selections(places: &[ProcessorPlace]) -> usize {
    node_pairs(places)
        .values()
        .map(|(producer, consumer)| memory_placements(*producer, *consumer).len())
        .sum()
}

#[test]
fn a_single_node_plan_promises_no_hops() {
    let places = synthesize(&HostSpec {
        nodes: 1,
        cache_domains_per_node: 2,
        cores_per_cache_domain: 2,
        threads_per_core: 2,
    });

    let plan = RunPlan::for_processors(&places);

    assert_eq!(plan.node_hops, 0, "a single-node host promised a crossing");
    assert_eq!(
        plan.memory_placements_per_hop, 0,
        "a host with no hops promised memory placements for them"
    );
}

#[test]
fn the_plan_counts_a_hop_once_per_memory_placement() {
    // The count that was missed: an edge measured in both directions at both
    // ring placements is four selections, not one.
    let places = two_socket_many_cache_domains();

    let plan = RunPlan::for_processors(&places);

    assert_eq!(
        plan.node_hops, 2,
        "both directions of the edge were not planned"
    );
    assert_eq!(
        plan.memory_placements_per_hop, 2,
        "the plan did not expect both ring placements"
    );
    assert_eq!(
        plan.node_hops * plan.memory_placements_per_hop,
        expected_hop_selections(&places),
        "the plan's hop selections do not match what the run will perform"
    );
}

#[test]
fn the_plan_counts_every_hop_selection_on_hosts_of_every_size() {
    // A property rather than a fixture: the machines this tool was written for
    // are larger than anything available to write a fixture against, and the
    // plan's error grows with the node count, so the untested sizes are exactly
    // the ones where being wrong costs the most.
    for nodes in 1..=8_u32 {
        let places = synthesize(&HostSpec {
            nodes,
            cache_domains_per_node: 1,
            cores_per_cache_domain: 2,
            threads_per_core: 1,
        });

        let plan = RunPlan::for_processors(&places);

        assert_eq!(
            plan.node_hops * plan.memory_placements_per_hop,
            expected_hop_selections(&places),
            "wrong hop selection count for {nodes} nodes"
        );
    }
}

#[test]
fn every_timed_handoff_the_run_performs_is_in_the_plan() {
    // Ties the headline number to the loops rather than to a hand-derived
    // constant. A constant would need editing whenever the run changes, which
    // is precisely the edit that gets forgotten.
    let places = two_socket_many_cache_domains();

    let plan = RunPlan::for_processors(&places);

    let selections =
        representative_pairs(&places).len() + plan.classes + expected_hop_selections(&places);
    assert_eq!(
        plan.timed_runs(),
        selections * plan.strategies * plan.repetitions,
        "the promised handoff count does not match the run"
    );
}

#[test]
fn memory_placements_names_both_endpoints() {
    // Both, and in this order: the producer's node first, so the first row of a
    // hop is the one where the producer writes locally.
    let places = two_socket_many_cache_domains();
    let (producer, consumer) = *node_pairs(&places)
        .get(&(0, 1))
        .expect("the two-socket fixture has a 0 -> 1 hop");

    assert_eq!(
        memory_placements(producer, consumer),
        [producer.numa_node, consumer.numa_node]
    );
}

#[test]
fn a_longer_run_is_never_promised_as_shorter() {
    // The estimate must not shrink when the machine grows. It is read as a
    // worst case, and a bigger machine that promises less is the one failure
    // mode a reader cannot detect from the output.
    let mut previous = 0.0_f64;
    for nodes in 1..=6_u32 {
        let places = synthesize(&HostSpec {
            nodes,
            cache_domains_per_node: 1,
            cores_per_cache_domain: 2,
            threads_per_core: 1,
        });

        let seconds = RunPlan::for_processors(&places).estimated_seconds();

        assert!(
            seconds >= previous,
            "{nodes} nodes promised {seconds}s, less than the {previous}s promised for fewer"
        );
        previous = seconds;
    }
}

/// One `by_node_pair` row, with the request and the result stated separately.
fn hop_row(
    pair: (u32, u32),
    strategy: super::Strategy,
    requested: Option<u32>,
    achieved: Option<u32>,
    nanos: f64,
) -> super::Measurement {
    let mut producer = place(0, 0, Some(0));
    let mut consumer = place(1, 0, Some(0));
    producer.numa_node = pair.0;
    consumer.numa_node = pair.1;
    super::Measurement {
        slice: super::Slice::pair(producer, consumer),
        producer,
        consumer,
        placement: Placement::CrossNumaNode,
        strategy,
        nanos_per_item: nanos,
        consumer_batch: 1.0,
        producer_batch: 1.0,
        memory_node: achieved,
        requested_memory_node: requested,
    }
}

/// An observation carrying only the given node-pair rows.
fn observation_of(rows: Vec<super::Measurement>) -> super::Observation {
    super::Observation {
        processors: Vec::new(),
        by_class: Vec::new(),
        measurements: Vec::new(),
        by_node_pair: rows,
    }
}

#[test]
fn a_node_pair_lookup_finds_each_requested_placement_when_both_were_redirected() {
    // **The defect this guards.** Windows may satisfy a NUMA allocation on a
    // node other than the one requested. Keyed on the achieved node, the two
    // rows of a pair become indistinguishable: one requested placement is
    // unfindable and a lookup for the other returns whichever row comes first,
    // which silently pairs a baseline taken at one placement against a cached
    // run taken at the other.
    let observation = observation_of(vec![
        hop_row((0, 1), super::Strategy::Cached, Some(0), Some(0), 10.0),
        hop_row((0, 1), super::Strategy::Cached, Some(1), Some(0), 20.0),
    ]);

    let asked_for_zero = observation
        .node_pair((0, 1), super::Strategy::Cached, Some(0))
        .expect("the row that asked for node 0 must be findable");
    let asked_for_one = observation
        .node_pair((0, 1), super::Strategy::Cached, Some(1))
        .expect("the row that asked for node 1 must be findable");

    assert!(
        (asked_for_zero.nanos_per_item - 10.0).abs() < f64::EPSILON,
        "got {}",
        asked_for_zero.nanos_per_item
    );
    assert!(
        (asked_for_one.nanos_per_item - 20.0).abs() < f64::EPSILON,
        "got {}",
        asked_for_one.nanos_per_item
    );
}

#[test]
fn a_node_pair_lookup_distinguishes_rows_that_share_a_requested_node() {
    // The converse, so the key is not merely coarser in the other direction:
    // rows differing by strategy must still be told apart.
    let observation = observation_of(vec![
        hop_row((0, 1), super::Strategy::Baseline, Some(1), Some(1), 30.0),
        hop_row((0, 1), super::Strategy::Cached, Some(1), Some(1), 40.0),
    ]);

    let baseline = observation
        .node_pair((0, 1), super::Strategy::Baseline, Some(1))
        .expect("baseline row");
    let cached = observation
        .node_pair((0, 1), super::Strategy::Cached, Some(1))
        .expect("cached row");

    assert!((baseline.nanos_per_item - 30.0).abs() < f64::EPSILON);
    assert!((cached.nanos_per_item - 40.0).abs() < f64::EPSILON);
}
