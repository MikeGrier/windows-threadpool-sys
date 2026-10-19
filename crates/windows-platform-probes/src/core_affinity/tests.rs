// Copyright (c) Mike Grier.
//! Tests for the core-affinity probe's row.

use windows_placement_probe::core_affinity::Placement;
use windows_placement_probe::fingerprint::Slice;
use windows_placement_probe::peer_index_cache::Strategy;
use windows_topology_sys::Provenance;

use super::*;

fn host(processors: usize) -> Fingerprint {
    Fingerprint {
        arch: "x86_64",
        processors,
        cores: processors / 2,
        smt: true,
        partitioning_cache_level: Some(3),
        cache_domain_sizes: vec![processors],
        efficiency_classes: vec![(0, processors)],
        numa_node_sizes: vec![processors],
        provenance: Provenance::Measured,
    }
}

fn place(group: u16, number: u8, cache_domain: Observed<u32>) -> ProcessorPlace {
    ProcessorPlace {
        group,
        number,
        core: u32::from(number) / 2,
        efficiency_class: number % 2,
        cache_domain,
        numa_node: u32::from(group),
    }
}

/// A measurement with distinct figures in every raw field.
fn measured(
    producer: ProcessorPlace,
    consumer: ProcessorPlace,
    placement: Placement,
    strategy: Strategy,
    elapsed: f64,
    reads: (u64, u64),
    nodes: (Option<u32>, Option<u32>),
) -> Measurement {
    Measurement {
        slice: Slice::pair(producer, consumer),
        producer,
        consumer,
        placement,
        strategy,
        nanos_per_item: elapsed / ITEMS as f64,
        elapsed_nanos: elapsed,
        consumer_refreshes: reads.0,
        producer_refreshes: reads.1,
        consumer_batch: ITEMS as f64 / reads.0.max(1) as f64,
        producer_batch: ITEMS as f64 / reads.1.max(1) as f64,
        requested_memory_node: nodes.0,
        memory_node: nodes.1,
    }
}

fn observation() -> Observation {
    let a = place(0, 0, Observed::Known(4));
    let b = place(0, 1, Observed::Absent);
    let c = place(1, 2, Observed::NotObserved);
    Observation {
        host: host(3),
        processors: vec![a, b, c],
        measurements: vec![
            measured(
                a,
                b,
                Placement::SameCoreSiblings,
                Strategy::Baseline,
                40_000_000.0,
                (1_900_000, 1_800_000),
                (None, None),
            ),
            measured(
                a,
                b,
                Placement::SameCoreSiblings,
                Strategy::Cached,
                20_000_000.0,
                (7_000, 300),
                (None, Some(0)),
            ),
        ],
        by_class: vec![measured(
            a,
            b,
            Placement::SameCacheSameClass,
            Strategy::Cached,
            30_000_001.0,
            (11, 13),
            (None, None),
        )],
        by_node_pair: vec![measured(
            a,
            c,
            Placement::CrossNumaNode,
            Strategy::Baseline,
            90_000_000.0,
            (2_000_017, 2_000_019),
            (Some(1), Some(0)),
        )],
    }
}

fn parsed(row: &Row) -> serde_json::Value {
    serde_json::from_str(&row.render()).expect("the row is JSON")
}

#[test]
fn the_row_names_itself_and_carries_the_settings_and_the_build() {
    let before = host(3);
    let row = parsed(&raw_row(
        &observation(),
        Some(&before),
        Some(&before),
        "v1 abc [ci]",
    ));
    assert_eq!(row["reason"], REASON);
    assert_eq!(REASON, "x-probe-core-affinity");
    assert_eq!(row["arch"], std::env::consts::ARCH);
    assert_eq!(row["build"], "v1 abc [ci]");
    assert_eq!(row["items"], ITEMS);
    assert_eq!(row["capacity"], CAPACITY);
    assert_eq!(row["repetitions"], REPETITIONS);
    assert!(row.get("declined").is_some_and(serde_json::Value::is_null));
}

#[test]
fn the_three_host_readings_are_published_as_taken() {
    let before = host(4);
    let after = host(5);
    let row = parsed(&raw_row(&observation(), Some(&before), Some(&after), "b"));
    assert_eq!(row["host_before"], before.to_string());
    assert_eq!(row["host"], host(3).to_string());
    assert_eq!(row["host_after"], after.to_string());
    // Three different machines here, so a swap of any two is visible.
    assert_ne!(row["host_before"], row["host"]);
    assert_ne!(row["host"], row["host_after"]);
}

#[test]
fn a_failed_reading_is_null_rather_than_absent() {
    let row = parsed(&raw_row(&observation(), None, None, "b"));
    assert!(
        row.get("host_before")
            .is_some_and(serde_json::Value::is_null)
    );
    assert!(
        row.get("host_after")
            .is_some_and(serde_json::Value::is_null)
    );
    assert_eq!(row["host"], host(3).to_string());
}

#[test]
fn every_processor_is_published_with_its_three_kinds_of_cache_answer() {
    let row = parsed(&raw_row(&observation(), None, None, "b"));
    assert_eq!(
        row["processors"],
        serde_json::json!([
            {"group": 0, "number": 0, "core": 0, "efficiency_class": 0,
             "cache_domain": 4, "numa_node": 0},
            {"group": 0, "number": 1, "core": 0, "efficiency_class": 1,
             "cache_domain": "absent", "numa_node": 0},
            {"group": 1, "number": 2, "core": 1, "efficiency_class": 0,
             "cache_domain": "not_observed", "numa_node": 1},
        ])
    );
}

#[test]
fn each_list_publishes_its_own_pairs_with_their_raw_figures() {
    let row = parsed(&raw_row(&observation(), None, None, "b"));
    assert_eq!(
        row["placements"],
        serde_json::json!([
            {"placement": "same_core_siblings", "strategy": "baseline",
             "producer_group": 0, "producer_number": 0,
             "consumer_group": 0, "consumer_number": 1,
             "requested_memory_node": null, "memory_node": null,
             "median_elapsed_ns": 40_000_000,
             "consumer_refreshes": 1_900_000, "producer_refreshes": 1_800_000},
            {"placement": "same_core_siblings", "strategy": "cached",
             "producer_group": 0, "producer_number": 0,
             "consumer_group": 0, "consumer_number": 1,
             "requested_memory_node": null, "memory_node": 0,
             "median_elapsed_ns": 20_000_000,
             "consumer_refreshes": 7_000, "producer_refreshes": 300},
        ])
    );
    assert_eq!(
        row["by_class"],
        serde_json::json!([
            {"placement": "same_cache_same_class", "strategy": "cached",
             "producer_group": 0, "producer_number": 0,
             "consumer_group": 0, "consumer_number": 1,
             "requested_memory_node": null, "memory_node": null,
             "median_elapsed_ns": 30_000_001,
             "consumer_refreshes": 11, "producer_refreshes": 13},
        ])
    );
    assert_eq!(
        row["node_pairs"],
        serde_json::json!([
            {"placement": "cross_numa_node", "strategy": "baseline",
             "producer_group": 0, "producer_number": 0,
             "consumer_group": 1, "consumer_number": 2,
             "requested_memory_node": 1, "memory_node": 0,
             "median_elapsed_ns": 90_000_000,
             "consumer_refreshes": 2_000_017, "producer_refreshes": 2_000_019},
        ])
    );
}

#[test]
fn a_host_with_nothing_to_measure_publishes_empty_lists() {
    let mut empty = observation();
    empty.measurements.clear();
    empty.by_class.clear();
    empty.by_node_pair.clear();
    let row = parsed(&raw_row(&empty, None, None, "b"));
    for key in ["placements", "by_class", "node_pairs"] {
        assert_eq!(row[key], serde_json::json!([]), "{key}");
    }
}

#[test]
fn a_declined_run_says_why_keeps_the_banner_reading_and_publishes_no_figures() {
    let error = std::io::Error::other("pin refused {\"reason\":\"x\"}\nsecond line");
    let before = host(4);
    let row = parsed(&declined_row(&error, Some(&before), "b"));
    assert_eq!(row["reason"], REASON);
    assert_eq!(
        row["declined"],
        "pin refused {\"reason\":\"x\"}\nsecond line"
    );
    assert_eq!(row["host_before"], before.to_string());
    for absent in [
        "host",
        "host_after",
        "processors",
        "placements",
        "by_class",
        "node_pairs",
    ] {
        assert!(row.get(absent).is_none(), "{absent}");
    }
}

#[test]
fn the_two_forms_share_every_key_but_the_measurement() {
    let keys = |row: &serde_json::Value| -> Vec<String> {
        row.as_object()
            .expect("an object")
            .keys()
            .cloned()
            .collect()
    };
    let measured = keys(&parsed(&raw_row(&observation(), None, None, "b")));
    let declined = keys(&parsed(&declined_row(
        &std::io::Error::other("e"),
        None,
        "b",
    )));
    let mut extra: Vec<&String> = measured
        .iter()
        .filter(|key| !declined.contains(key))
        .collect();
    extra.sort();
    assert_eq!(
        extra,
        [
            "by_class",
            "host",
            "host_after",
            "node_pairs",
            "placements",
            "processors"
        ]
    );
    assert!(declined.iter().all(|key| measured.contains(key)));
}
