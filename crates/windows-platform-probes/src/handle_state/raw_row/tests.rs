// Copyright (c) Mike Grier.
//! Tests for the handle-state probe's row.

use super::*;

fn list(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

fn observation() -> Observation {
    Observation {
        ground_truth: list(&[".", "..", "f00.t", "f01.t", "f02.t"]),
        duplicate: CursorObservation {
            source_first: list(&[".", ".."]),
            other_next: Ok(list(&["f00.t"])),
        },
        separate_opens: CursorObservation {
            source_first: list(&["."]),
            other_next: Ok(list(&[".", ".."])),
        },
        source_survived_close: true,
        queries: vec![
            QueryObservation {
                query: SingleShot::IdInfo,
                on_duplicate: false,
                succeeded: true,
                cursor: CursorObservation {
                    source_first: list(&["f01.t"]),
                    other_next: Ok(list(&["f02.t"])),
                },
            },
            QueryObservation {
                query: SingleShot::BasicInfo,
                on_duplicate: true,
                succeeded: false,
                cursor: CursorObservation {
                    source_first: list(&["f00.t"]),
                    other_next: Err(18),
                },
            },
        ],
    }
}

fn parsed(row: &Row) -> serde_json::Value {
    serde_json::from_str(&row.render()).expect("the row is JSON")
}

#[test]
fn the_row_names_itself_and_carries_the_apparatus_sizes_and_the_build() {
    let row = parsed(&raw_row(&observation(), "v1 abc [ci]"));
    assert_eq!(row["reason"], REASON);
    assert_eq!(REASON, "x-probe-handle-state");
    assert_eq!(row["arch"], std::env::consts::ARCH);
    assert_eq!(row["build"], "v1 abc [ci]");
    assert_eq!(row["buffer_bytes"], BUFFER_BYTES);
    assert_eq!(row["fixture_files"], FIXTURE_FILES);
}

#[test]
fn the_ground_truth_and_both_two_call_observations_are_published_as_names() {
    let row = parsed(&raw_row(&observation(), "b"));
    assert_eq!(
        row["ground_truth"],
        serde_json::json!([".", "..", "f00.t", "f01.t", "f02.t"])
    );
    assert_eq!(
        row["duplicate"],
        serde_json::json!({"first": [".", ".."], "next": ["f00.t"], "next_error": null})
    );
    assert_eq!(
        row["separate_opens"],
        serde_json::json!({"first": ["."], "next": [".", ".."], "next_error": null})
    );
    assert_eq!(row["source_survived_close"], true);
}

#[test]
fn each_query_is_published_with_its_outcome_and_the_names_around_it() {
    let row = parsed(&raw_row(&observation(), "b"));
    assert_eq!(
        row["queries"],
        serde_json::json!([
            {"query": "id_info", "on_duplicate": false, "succeeded": true,
             "first": ["f01.t"], "next": ["f02.t"], "next_error": null},
            {"query": "basic_info", "on_duplicate": true, "succeeded": false,
             "first": ["f00.t"], "next": null, "next_error": 18},
        ])
    );
}

#[test]
fn a_source_broken_by_the_close_is_published_as_false() {
    let mut broken = observation();
    broken.source_survived_close = false;
    assert_eq!(
        parsed(&raw_row(&broken, "b"))["source_survived_close"],
        false
    );
}

#[test]
fn every_query_has_a_distinct_record_name() {
    let all = [SingleShot::BasicInfo, SingleShot::IdInfo, SingleShot::NonEx];
    let mut names: Vec<&str> = all.iter().map(|query| query.name()).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), all.len());
}
