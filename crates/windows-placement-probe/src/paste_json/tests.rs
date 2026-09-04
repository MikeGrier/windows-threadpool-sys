// Copyright (c) Mike Grier.

//! Tests for the paste layout.
//!
//! The one that matters is the round trip: layout must never change data. The
//! rest describe the shape a reader is promised, and several are written
//! against a machine larger than the host they run on, because the whole reason
//! this module exists is the submission from a big NUMA server.

use serde_json::{Value, json};

use super::{MAX_WIDTH, to_paste_json};

/// Parse laid-out text back, so a test can compare data rather than characters.
fn round_trip(value: &Value) -> Value {
    let text = to_paste_json(value).expect("must lay out");
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("not valid JSON: {error}\n{text}"))
}

#[test]
fn layout_never_changes_the_data() {
    // The claim the module makes about itself. Awkward values on purpose: a
    // string that needs escaping, floats that must keep their digits, an empty
    // container of each kind, and a null.
    let value = json!({
        "escaping": "a \"quoted\" \\ back\\slash\nand a newline\ttab",
        "unicode": "em dash \u{2014} and an emoji \u{1f600}",
        "float": 10.672_85,
        "small_float": 0.000_000_1,
        "big": 18_446_744_073_709_551_615_u64,
        "negative": -42,
        "nothing": null,
        "yes": true,
        "empty_object": {},
        "empty_array": [],
        "nested": [[1, 2], [3, 4]],
        "objects": [{"a": 1}, {"b": [1, 2, 3]}],
    });

    assert_eq!(round_trip(&value), value);
}

#[test]
fn a_short_array_of_scalars_is_one_line() {
    // The case that prompted this: eight cache domains should not be eight
    // lines saying `2`.
    let text = to_paste_json(&json!({"cache_domain_sizes": [2, 2, 2, 2, 2, 2, 2, 2]}))
        .expect("must lay out");

    assert!(
        text.contains("\"cache_domain_sizes\": [2, 2, 2, 2, 2, 2, 2, 2]"),
        "the array was not collapsed:\n{text}"
    );
}

#[test]
fn an_array_of_short_arrays_is_one_line() {
    let text = to_paste_json(&json!({"efficiency_classes": [[0, 16]]})).expect("must lay out");

    assert!(
        text.contains("\"efficiency_classes\": [[0, 16]]"),
        "the nested array was not collapsed:\n{text}"
    );
}

#[test]
fn a_long_array_of_scalars_fills_lines_instead_of_one_per_element() {
    // The eight-socket case. One element per line would be 64 lines; this
    // asserts it is a handful, and asserts the count rather than a fixed
    // rendering so the width budget can move without editing the test.
    let value = json!({"numa_node_sizes": vec![16; 64]});

    let text = to_paste_json(&value).expect("must lay out");

    let lines = text.lines().count();
    assert!(
        lines < 12,
        "64 elements took {lines} lines, which is close to one per element:\n{text}"
    );
    assert_eq!(round_trip(&value), value);
}

#[test]
fn no_line_exceeds_the_width_budget() {
    // Holds for values with nothing unbreakable in them. A long string is the
    // documented exception and is covered separately.
    let value = json!({
        "numa_node_sizes": vec![1024; 64],
        "cache_domain_sizes": vec![2; 40],
        "nested": vec![vec![7, 8]; 30],
    });

    let text = to_paste_json(&value).expect("must lay out");

    for line in text.lines() {
        assert!(
            line.chars().count() <= MAX_WIDTH,
            "a {}-character line exceeds the budget: {line:?}",
            line.chars().count()
        );
    }
}

#[test]
fn an_unbreakable_value_is_emitted_whole_rather_than_broken() {
    // A string longer than the budget has no split point that keeps the JSON
    // valid, so the budget yields. Stated as a test because the alternative --
    // breaking it -- would produce something that no longer parses.
    let long = "x".repeat(MAX_WIDTH * 2);
    let value = json!({ "slice": long });

    let text = to_paste_json(&value).expect("must lay out");

    assert!(text.contains(&long), "the long value was altered:\n{text}");
    assert_eq!(round_trip(&value), value);
}

#[test]
fn an_object_always_expands_even_when_it_would_fit() {
    // Rule 1. A reader scanning for a field should find it in a predictable
    // place, so objects do not collapse just because they are short.
    let value = json!({"build": {"dirty": true, "source": "local"}});

    let text = to_paste_json(&value).expect("must lay out");

    assert!(
        text.contains("\"dirty\": true,\n"),
        "a short object was collapsed onto one line:\n{text}"
    );
}

#[test]
fn an_array_holding_an_object_expands_one_element_per_line() {
    // Rule 2's exclusion. Measurement rows are the part a reader compares
    // against each other, and they are only comparable when aligned.
    let value = json!({"placements": [{"ns": 1.0}, {"ns": 2.0}]});

    let text = to_paste_json(&value).expect("must lay out");

    assert_eq!(
        text.matches("\"ns\"").count(),
        2,
        "expected both rows:\n{text}"
    );
    assert!(!text.contains("}, {"), "two objects shared a line:\n{text}");
}

#[test]
fn an_object_nested_inside_a_fitting_array_still_expands() {
    // The hole rule 1 would have if the check looked only at an array's
    // immediate elements: a tiny object could slip onto one line by being
    // wrapped in an array that fits the budget.
    let value = json!({"rows": [[{"a": 1}]]});

    let text = to_paste_json(&value).expect("must lay out");

    assert!(
        text.contains("\"a\": 1\n"),
        "an object nested in a short array was collapsed:\n{text}"
    );
}

#[test]
fn empty_containers_stay_on_one_line() {
    let value = json!({"node_hops": [], "by_class": {}});

    let text = to_paste_json(&value).expect("must lay out");

    assert!(text.contains("\"node_hops\": []"), "got:\n{text}");
    assert!(text.contains("\"by_class\": {}"), "got:\n{text}");
}

#[test]
fn the_layout_is_deterministic() {
    // A checksum is printed over this text, so identical input must produce
    // identical bytes or the digest would be worthless.
    let value = json!({"a": [1, 2, 3], "b": {"c": vec![9; 50]}});

    assert_eq!(
        to_paste_json(&value).expect("must lay out"),
        to_paste_json(&value).expect("must lay out")
    );
}

#[test]
fn a_field_name_is_counted_against_the_width_of_its_value() {
    // The off-by-a-key-width bug. If the value is measured from the indent
    // rather than from the column it starts at, a long name pushes its own
    // value past the budget.
    let name = "a".repeat(MAX_WIDTH - 20);
    let value = json!({ name: vec![100; 4] });

    let text = to_paste_json(&value).expect("must lay out");

    for line in text.lines() {
        assert!(
            line.chars().count() <= MAX_WIDTH,
            "a {}-character line exceeds the budget: {line:?}",
            line.chars().count()
        );
    }
}

// ---------------------------------------------------------------------------
// Field order.
//
// An earlier draft of this module laid out a `serde_json::Value`, whose object
// is a `BTreeMap`. Every test above still passed: the JSON was valid, the data
// round-tripped, the arrays collapsed. It was only reading the tool's actual
// output that showed `build` had taken first place from `schema_version` and
// every measurement row now opened with `consumer_batch`. These are the tests
// that would have caught it.
// ---------------------------------------------------------------------------

/// The top-level field names, in the order they appear.
fn top_level_keys(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let field = line.strip_prefix("  \"")?;
            // Two-space indent only, so nested objects are not collected.
            let (name, _) = field.split_once('"')?;
            Some(name.to_owned())
        })
        .collect()
}

#[test]
fn a_record_keeps_the_order_its_fields_are_declared_in() {
    // Not merely "unsorted": the exact order, because the order is a choice.
    // What a reader needs first -- which schema, when, from what build -- comes
    // first, and the measurements come last.
    let record = crate::record::tests::fully_populated();

    let text = to_paste_json(&record).expect("must lay out");

    assert_eq!(
        top_level_keys(&text),
        vec![
            "schema_version",
            "recorded_at",
            "recorded_at_epoch_seconds",
            "recorded_at_suppressed",
            "build",
            "machine",
            "host",
            "topology_provenance",
            "placements",
            "node_hops",
            "by_class",
        ]
    );
}

#[test]
fn a_measurement_row_leads_with_what_identifies_it() {
    // Alphabetical order puts `consumer_batch` first, which is a detail, and
    // buries `placement` and `strategy`, which are what the row is about.
    let record = crate::record::tests::fully_populated();

    let text = to_paste_json(&record).expect("must lay out");
    let placement = text.find("\"placement\":").expect("a row must be present");
    let strategy = text
        .find("\"strategy\":")
        .expect("a row must have a strategy");
    let batch = text
        .find("\"consumer_batch\":")
        .expect("a row must have a batch depth");

    assert!(
        placement < strategy && strategy < batch,
        "a measurement row is not in declaration order:\n{text}"
    );
}

#[test]
fn nested_objects_keep_their_order_too() {
    // The build stamp is the case where sorting is least obvious and most
    // annoying: `commit` would come before `crate_version`.
    let record = crate::record::tests::fully_populated();

    let text = to_paste_json(&record).expect("must lay out");
    let version = text.find("\"crate_version\":").expect("must be present");
    let commit = text.find("\"commit\":").expect("must be present");

    assert!(version < commit, "a nested object was reordered:\n{text}");
}
