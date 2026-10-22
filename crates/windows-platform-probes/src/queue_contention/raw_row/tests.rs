// Copyright (c) Mike Grier.
//! Tests for the queue probe's row.

use super::*;
use crate::queue_contention::summarise;

/// A configuration whose passes all took `elapsed` nanoseconds.
fn run_taking(shape: &'static str, producers: usize, pushes: usize, elapsed: f64) -> Run {
    summarise(shape, producers, pushes, vec![(elapsed, 3)])
}

fn observation(isolated: Vec<Run>, drained: Vec<Run>, pushes: usize) -> Observation {
    Observation {
        isolated,
        drained,
        available_parallelism: Some(16),
        sampling: Sampling {
            pushes_per_producer: pushes,
            ..Sampling::default()
        },
    }
}

fn parsed(row: &Row) -> serde_json::Value {
    serde_json::from_str(&row.render()).expect("the row is JSON")
}

#[test]
fn the_row_names_itself_and_carries_the_sampling_and_the_build() {
    let row = parsed(&raw_row(
        &observation(Vec::new(), Vec::new(), 50_000),
        "v1.2.3 0123456789ab [ci]",
    ));
    assert_eq!(row["reason"], "x-probe-queue-contention");
    assert_eq!(row["arch"], std::env::consts::ARCH);
    assert_eq!(row["build"], "v1.2.3 0123456789ab [ci]");
    assert_eq!(row["pushes_per_producer"], 50_000);
    assert_eq!(row["repetitions"], REPETITIONS);
    assert_eq!(row["order"], "interleaved");
    assert_eq!(row["producer_counts"], serde_json::json!(PRODUCER_COUNTS));
    assert_eq!(row["available_parallelism"], 16);
    assert_eq!(row["isolated"], serde_json::json!([]));
    assert_eq!(row["drained"], serde_json::json!([]));
}

#[test]
fn an_unknown_processor_count_is_null_rather_than_absent() {
    let mut unknown = observation(Vec::new(), Vec::new(), 1);
    unknown.available_parallelism = None;
    let row = parsed(&raw_row(&unknown, "b"));
    assert!(row["available_parallelism"].is_null());
    assert!(row.get("available_parallelism").is_some());
}

#[test]
fn every_elapsed_time_is_recovered_exactly_from_the_per_operation_figures() {
    // The claim the row's elapsed fields rest on: dividing by the total pushes
    // and multiplying back loses nothing that rounding does not restore. Across
    // small and large times, push counts that do and do not divide them, and
    // producer counts from one to thirty-two.
    for elapsed in [
        1.0,
        7.0,
        999_999_937.0,
        65_000_000_123.0,
        (1u64 << 50) as f64 + 3.0,
    ] {
        for (producers, pushes) in [(1, 1), (1, 1_000), (3, 49_999), (32, 50_000), (16, 7)] {
            let row = parsed(&raw_row(
                &observation(
                    vec![run_taking(
                        shapes::SLOTWISE_MPSC,
                        producers,
                        pushes,
                        elapsed,
                    )],
                    Vec::new(),
                    pushes,
                ),
                "b",
            ));
            let entry = &row["isolated"][0];
            for field in [
                "median_elapsed_ns",
                "fastest_elapsed_ns",
                "slowest_elapsed_ns",
            ] {
                assert_eq!(
                    entry[field].as_u64(),
                    Some(elapsed as u64),
                    "{field}: {elapsed} ns over {producers} x {pushes} pushes"
                );
            }
        }
    }
}

#[test]
fn each_regime_lists_its_runs_in_order_with_their_refusals() {
    let isolated = vec![
        run_taking(shapes::BASELINE_FETCH_ADD, 1, 10, 100.0),
        run_taking(shapes::RESERVING_MPSC, 2, 10, 300.0),
    ];
    let drained = vec![run_taking(shapes::PERMIT_MPSC, 4, 10, 900.0)];
    let row = parsed(&raw_row(&observation(isolated, drained, 10), "b"));

    let shapes_in = |key: &str| -> Vec<(String, u64)> {
        row[key]
            .as_array()
            .expect("a list")
            .iter()
            .map(|entry| {
                (
                    entry["shape"].as_str().expect("a shape").to_owned(),
                    entry["producers"].as_u64().expect("a count"),
                )
            })
            .collect()
    };
    assert_eq!(
        shapes_in("isolated"),
        vec![
            (shapes::BASELINE_FETCH_ADD.to_owned(), 1),
            (shapes::RESERVING_MPSC.to_owned(), 2)
        ]
    );
    assert_eq!(
        shapes_in("drained"),
        vec![(shapes::PERMIT_MPSC.to_owned(), 4)]
    );
    assert_eq!(row["drained"][0]["refusals"], 3);
    assert_eq!(row["drained"][0]["median_elapsed_ns"], 900);
}

#[test]
fn a_configuration_that_did_not_measure_publishes_null_not_zero() {
    // Zero would read as an immeasurably fast pass; null says there was none.
    let unmeasured = Run {
        shape: shapes::CLAIM_WIDE,
        producers: 2,
        nanos_per_op: 0.0,
        ops_per_second: 0.0,
        refusals: 0,
        fastest_nanos_per_op: 0.0,
        slowest_nanos_per_op: 0.0,
    };
    let row = parsed(&raw_row(
        &observation(vec![unmeasured], Vec::new(), 10),
        "b",
    ));
    for field in [
        "median_elapsed_ns",
        "fastest_elapsed_ns",
        "slowest_elapsed_ns",
    ] {
        assert!(row["isolated"][0][field].is_null(), "{field}");
    }
    assert_eq!(row["isolated"][0]["shape"], shapes::CLAIM_WIDE);
}

#[test]
fn an_even_number_of_passes_publishes_three_distinct_passes_that_happened() {
    // Four passes, deliberately out of order, each with its own refusal count.
    // The median is the upper middle pass (300 ns, 7 refusals), never the 250
    // an average would give; the extremes are the first and last after sorting.
    let run = summarise(
        shapes::PERMIT_MPSC,
        2,
        10,
        vec![(400.0, 9), (100.0, 1), (300.0, 7), (200.0, 5)],
    );
    let row = parsed(&raw_row(&observation(Vec::new(), vec![run], 10), "b"));
    let entry = &row["drained"][0];
    assert_eq!(entry["median_elapsed_ns"], 300);
    assert_eq!(entry["fastest_elapsed_ns"], 100);
    assert_eq!(entry["slowest_elapsed_ns"], 400);
    assert_eq!(entry["refusals"], 7);
}
