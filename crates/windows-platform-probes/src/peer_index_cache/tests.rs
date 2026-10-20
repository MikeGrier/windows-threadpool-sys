// Copyright (c) Mike Grier.
//! Tests for the peer-index probe's row.

use super::*;

/// A run whose median pass took `elapsed` nanoseconds.
fn run(label: &'static str, elapsed: f64, consumer: u64, producer: u64) -> Run {
    Run {
        label,
        nanos_per_item: elapsed / ITEMS as f64,
        items_per_second: ITEMS as f64 / (elapsed / 1e9),
        consumer_refreshes: consumer,
        producer_refreshes: producer,
    }
}

/// An observation shaped as `measure` produces one, with distinct figures in
/// every field so no two can be swapped unnoticed.
fn observation() -> Observation {
    Observation {
        calibration: run("shipping spsc", 9_000_000.0, ITEMS as u64, ITEMS as u64),
        strategies: vec![
            run(
                Strategy::Baseline.label(),
                10_000_000.0,
                1_900_000,
                1_800_000,
            ),
            run(Strategy::Cached.label(), 4_000_000.0, 7_000, 300),
            run(Strategy::Warmed.label(), 11_000_000.0, 1_950_000, 1_850_000),
        ],
    }
}

fn parsed(row: &Row) -> serde_json::Value {
    serde_json::from_str(&row.render()).expect("the row is JSON")
}

#[test]
fn the_row_names_itself_and_carries_the_settings_and_the_build() {
    let row = parsed(&raw_row(&observation(), "v1.2.3 0123456789ab [ci]"));
    assert_eq!(row["reason"], REASON);
    assert_eq!(REASON, "x-probe-peer-index-cache");
    assert_eq!(row["arch"], std::env::consts::ARCH);
    assert_eq!(row["build"], "v1.2.3 0123456789ab [ci]");
    assert_eq!(row["items"], ITEMS);
    assert_eq!(row["capacity"], CAPACITY);
    assert_eq!(row["repetitions"], REPETITIONS);
    // Present and null, so a reader can tell a measured row from a declined
    // one by the field rather than by which other fields happen to exist.
    assert!(row.get("declined").is_some_and(serde_json::Value::is_null));
}

#[test]
fn each_strategy_is_published_under_its_record_name_with_its_own_figures() {
    let row = parsed(&raw_row(&observation(), "b"));
    assert_eq!(
        row["strategies"],
        serde_json::json!([
            {"strategy": "baseline", "median_elapsed_ns": 10_000_000,
             "consumer_refreshes": 1_900_000, "producer_refreshes": 1_800_000},
            {"strategy": "cached", "median_elapsed_ns": 4_000_000,
             "consumer_refreshes": 7_000, "producer_refreshes": 300},
            {"strategy": "warmed", "median_elapsed_ns": 11_000_000,
             "consumer_refreshes": 1_950_000, "producer_refreshes": 1_850_000},
        ])
    );
}

#[test]
fn the_strategies_are_listed_in_a_fixed_order_whatever_order_they_were_measured_in() {
    let mut reversed = observation();
    reversed.strategies.reverse();
    let names: Vec<String> = parsed(&raw_row(&reversed, "b"))["strategies"]
        .as_array()
        .expect("a list")
        .iter()
        .map(|entry| entry["strategy"].as_str().expect("a name").to_owned())
        .collect();
    assert_eq!(names, ["baseline", "cached", "warmed"]);
}

#[test]
fn a_strategy_that_was_not_measured_is_absent_rather_than_invented() {
    let mut partial = observation();
    partial
        .strategies
        .retain(|run| run.label != Strategy::Cached.label());
    let row = parsed(&raw_row(&partial, "b"));
    let names: Vec<&str> = row["strategies"]
        .as_array()
        .expect("a list")
        .iter()
        .map(|entry| entry["strategy"].as_str().expect("a name"))
        .collect();
    assert_eq!(names, ["baseline", "warmed"]);

    let mut none = observation();
    none.strategies.clear();
    assert_eq!(
        parsed(&raw_row(&none, "b"))["strategies"],
        serde_json::json!([])
    );
}

#[test]
fn the_shipping_queue_publishes_its_time_but_not_the_floor_it_was_given_as_counts() {
    let row = parsed(&raw_row(&observation(), "b"));
    assert_eq!(
        row["calibration"],
        serde_json::json!({
            "median_elapsed_ns": 9_000_000,
            "consumer_refreshes": null,
            "producer_refreshes": null,
        })
    );
}

#[test]
fn every_elapsed_time_is_recovered_exactly_from_the_per_item_figure() {
    // The claim the elapsed fields rest on: dividing by the items and
    // multiplying back loses nothing that rounding does not restore. Across
    // times from one nanosecond to well past any real pass, including ones the
    // item count does not divide. 249 and 507 are among the times whose round
    // trip lands just BELOW the whole nanosecond, found by searching, so they
    // are what tells rounding from truncation.
    for elapsed in [
        1.0,
        7.0,
        249.0,
        507.0,
        1_999_999.0,
        2_000_001.0,
        999_999_937.0,
        65_000_000_123.0,
        (1u64 << 50) as f64 + 3.0,
    ] {
        let mut sample = observation();
        sample.calibration = run("shipping spsc", elapsed, 0, 0);
        sample.strategies = vec![run(Strategy::Cached.label(), elapsed, 0, 0)];
        let row = parsed(&raw_row(&sample, "b"));
        assert_eq!(
            row["calibration"]["median_elapsed_ns"].as_u64(),
            Some(elapsed as u64),
            "calibration: {elapsed} ns"
        );
        assert_eq!(
            row["strategies"][0]["median_elapsed_ns"].as_u64(),
            Some(elapsed as u64),
            "strategy: {elapsed} ns"
        );
    }
}

#[test]
fn a_declined_run_says_why_and_publishes_no_figures() {
    // Quotes, a brace and a newline: the error text is the OS's, and has to
    // stay one field of one row whatever it contains.
    let error = std::io::Error::other("refused {\"reason\":\"x\"}\nsecond line");
    let row = parsed(&declined_row(&error, "b"));
    assert_eq!(row["reason"], REASON);
    assert_eq!(row["declined"], "refused {\"reason\":\"x\"}\nsecond line");
    assert_eq!(row["items"], ITEMS);
    assert_eq!(row["repetitions"], REPETITIONS);
    assert!(row.get("calibration").is_none());
    assert!(row.get("strategies").is_none());
}

#[test]
fn the_two_forms_share_every_key_but_the_figures() {
    // Both directions: the declined form carries nothing the measured form
    // lacks, and the measured form adds exactly the figures.
    let keys = |row: &serde_json::Value| -> Vec<String> {
        row.as_object()
            .expect("an object")
            .keys()
            .cloned()
            .collect()
    };
    let measured = keys(&parsed(&raw_row(&observation(), "b")));
    let declined = keys(&parsed(&declined_row(&std::io::Error::other("e"), "b")));
    let mut extra: Vec<&String> = measured
        .iter()
        .filter(|key| !declined.contains(key))
        .collect();
    extra.sort();
    assert_eq!(extra, ["calibration", "strategies"]);
    assert!(declined.iter().all(|key| measured.contains(key)));
}
