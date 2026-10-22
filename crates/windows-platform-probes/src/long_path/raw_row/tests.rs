// Copyright (c) Mike Grier.
//! Tests for the long-path probes' row.

use super::*;

fn attempt(shape: Shape, resolved_len: usize, opened: bool, error: u32) -> Attempt {
    Attempt {
        shape,
        resolved_len,
        over_max_path: resolved_len > MAX_PATH_CONTENT,
        opened,
        error,
    }
}

fn observation(manifest_aware: bool) -> Observation {
    Observation {
        manifest_aware,
        registry_enabled: Some(true),
        attempts: vec![
            attempt(Shape::Plain, 120, true, 0),
            attempt(Shape::DotDot, 125, true, 0),
            attempt(Shape::ForwardSlash, 121, true, 0),
            attempt(Shape::Plain, 300, true, 0),
            attempt(Shape::DotDot, 305, false, ERROR_PATH_NOT_FOUND),
            attempt(Shape::ForwardSlash, 301, false, ERROR_FILE_NOT_FOUND),
        ],
        apparatus_error: None,
    }
}

fn parsed(row: &Row) -> serde_json::Value {
    serde_json::from_str(&row.render()).expect("the row is JSON")
}

#[test]
fn the_row_names_itself_and_carries_both_halves_of_the_opt_in() {
    let row = parsed(&raw_row(&observation(true), "v1 abc [ci]"));
    assert_eq!(row["reason"], REASON);
    assert_eq!(REASON, "x-probe-long-path");
    assert_eq!(row["arch"], std::env::consts::ARCH);
    assert_eq!(row["build"], "v1 abc [ci]");
    assert_eq!(row["manifest_aware"], true);
    assert_eq!(row["registry_enabled"], true);
    assert_eq!(row["ceiling"], 259);
    assert!(row.get("declined").is_some_and(serde_json::Value::is_null));
}

#[test]
fn the_two_binaries_differ_in_the_manifest_field_and_nowhere_else() {
    // The finding is the difference between the two runs, so the row must let
    // a reader tell them apart by one field, with nothing else moving.
    let aware = parsed(&raw_row(&observation(true), "b"));
    let unaware = parsed(&raw_row(&observation(false), "b"));
    assert_eq!(aware["manifest_aware"], true);
    assert_eq!(unaware["manifest_aware"], false);
    let mut aware = aware;
    aware["manifest_aware"] = false.into();
    assert_eq!(aware, unaware);
}

#[test]
fn every_attempt_is_published_with_its_error_only_when_it_failed() {
    let row = parsed(&raw_row(&observation(false), "b"));
    assert_eq!(
        row["attempts"],
        serde_json::json!([
            {"shape": "plain", "resolved_len": 120, "opened": true, "error": null},
            {"shape": "dot_dot", "resolved_len": 125, "opened": true, "error": null},
            {"shape": "forward_slash", "resolved_len": 121, "opened": true, "error": null},
            {"shape": "plain", "resolved_len": 300, "opened": true, "error": null},
            {"shape": "dot_dot", "resolved_len": 305, "opened": false, "error": ERROR_PATH_NOT_FOUND},
            {"shape": "forward_slash", "resolved_len": 301, "opened": false, "error": ERROR_FILE_NOT_FOUND},
        ])
    );
}

#[test]
fn a_registry_read_that_was_off_is_false_and_one_never_made_is_null() {
    let mut off = observation(true);
    off.registry_enabled = Some(false);
    assert_eq!(parsed(&raw_row(&off, "b"))["registry_enabled"], false);

    let mut unread = observation(true);
    unread.registry_enabled = None;
    let row = parsed(&raw_row(&unread, "b"));
    assert!(
        row.get("registry_enabled")
            .is_some_and(serde_json::Value::is_null)
    );
}

#[test]
fn a_refused_run_says_why_and_publishes_no_attempts() {
    let refused = Observation {
        manifest_aware: true,
        registry_enabled: None,
        attempts: Vec::new(),
        apparatus_error: Some("TEMP is \"too\" long\nsecond line".to_owned()),
    };
    let row = parsed(&raw_row(&refused, "b"));
    assert_eq!(row["declined"], "TEMP is \"too\" long\nsecond line");
    assert_eq!(row["attempts"], serde_json::json!([]));
    assert!(row["registry_enabled"].is_null());
}

#[test]
fn every_shape_has_a_distinct_record_name_that_is_not_its_label() {
    let all = [Shape::Plain, Shape::DotDot, Shape::ForwardSlash];
    let mut names: Vec<&str> = all.iter().map(|shape| shape.name()).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), all.len());
    // `plain` is both, legitimately; the other two must differ, since their
    // labels carry punctuation a record key should not.
    assert_ne!(Shape::DotDot.name(), Shape::DotDot.label());
    assert_ne!(Shape::ForwardSlash.name(), Shape::ForwardSlash.label());
}
