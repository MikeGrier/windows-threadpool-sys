// Copyright (c) Mike Grier.

//! Tests for the row's well-formedness check.
//!
//! Half of these assert ACCEPTANCE. A check that fires on a legal row costs a
//! reader more than one that misses an illegal one, because noise trains them to
//! ignore the instrument -- and the reports this runs against are the ones a
//! fleet survey mines, so a false alarm is a false finding about a host.

use super::{RowDefect, check, keys, row};

/// A well-formed row, in the shape the renderer emits.
fn clean_row() -> String {
    concat!(
        r#"{"reason":"x-probe-topology","arch":"x86_64","processors":16,"#,
        r#""efficiency_classes":[0],"caches":[{"level":1,"domains":8}],"#,
        r#""policies":{"single":1},"cross_check":"agree","disagreements":[],"#,
        r#""parse_incomplete":[]}"#
    )
    .to_owned()
}

/// A report carrying `row` under a line of prose.
fn report_with(row: &str) -> String {
    format!("host:  x86_64 16p/8c\nsome prose the reader gets\n{row}")
}

#[test]
fn a_well_formed_row_is_accepted() {
    assert_eq!(check(&report_with(&clean_row())), Vec::new());
}

#[test]
fn a_report_with_no_row_is_a_defect() {
    // Every report has one, including the unmeasured shape -- that is what lets
    // a survey tell a host where discovery failed from a job that never ran the
    // probe.
    assert_eq!(
        check("host:  x86_64 16p/8c\nprose only, no row"),
        vec![RowDefect::Missing]
    );
}

#[test]
fn a_report_with_two_rows_is_a_defect() {
    // A mining pass reads the first line that looks like a row, so a second is
    // not extra data -- it is an ambiguity about which line is the contract.
    let two = format!("{}\n{}", report_with(&clean_row()), clean_row());

    assert_eq!(check(&two), vec![RowDefect::Duplicated { count: 2 }]);
}

#[test]
fn an_unbalanced_row_is_a_defect() {
    let truncated = r#"{"reason":"x-probe-topology","caches":[{"level":1}"#;

    assert_eq!(
        check(&report_with(truncated)),
        vec![RowDefect::Unbalanced {
            row: truncated.to_owned()
        }]
    );
}

#[test]
fn a_bracket_inside_a_string_does_not_unbalance_a_row() {
    // **The acceptance half that matters most**, because the renderer does emit
    // brackets inside strings: a failed discovery's `io::Error` is interpolated
    // into a string value, and an OS message is free to contain one. A balance
    // check that counted them would report every such host as malformed.
    let with_brackets = r#"{"reason":"x-probe-topology","error":"failed at [0] {oops}"}"#;

    assert_eq!(check(&report_with(with_brackets)), Vec::new());
}

#[test]
fn an_escaped_quote_does_not_end_a_string() {
    // The other half of the same hazard: a `\"` inside an error message would
    // otherwise close the string early and put the rest of the message into
    // bracket-counting.
    let escaped = r#"{"reason":"x-probe-topology","error":"he said \"[\" and left"}"#;

    assert_eq!(check(&report_with(escaped)), Vec::new());
}

#[test]
fn an_escaped_quote_inside_a_value_does_not_forge_a_key() {
    // **A correct report crashed the probe, and this is the shape that did it.**
    // `keys` used `find('"')`, which takes `\"` for a terminator, so an escaped
    // quote shifted where it thought strings began and ended and text INSIDE a
    // value was emitted as a top-level key. Two equal ones read as a repeated
    // key, and `assert_corresponds` panicked from inside `report_unmeasured`.
    //
    // Reachable, not hypothetical: `discovery_error` carries a failed
    // discovery's `io::Error`, whose message is whatever the OS said.
    let forged = concat!(
        r#"{"reason":"x-probe-topology","arch":"x86_64","#,
        r#""discovery_error":"q\":1,\"q\":1,\"q"}"#
    );

    assert_eq!(
        keys(forged),
        vec!["reason", "arch", "discovery_error"],
        "the error's contents are a VALUE, however many quotes it contains"
    );
    assert_eq!(check(&report_with(forged)), Vec::new());
}

#[test]
fn a_backslash_before_the_closing_quote_does_not_swallow_the_rest_of_the_row() {
    // The other half: a value ending in an escaped backslash closes normally,
    // so the keys after it are still found. Getting this wrong in the other
    // direction would silently drop every key that follows.
    let row = r#"{"reason":"x","path":"C:\\temp\\","cross_check":"agree"}"#;

    assert_eq!(keys(row), vec!["reason", "path", "cross_check"]);
    assert_eq!(check(&report_with(row)), Vec::new());
}

#[test]
fn a_repeated_key_is_a_defect() {
    // Not a parse error in most readers -- they take the last -- so this is
    // precisely the malformation that survives a consumer's parse and changes
    // what it reads.
    let repeated = r#"{"reason":"x-probe-topology","processors":16,"processors":8}"#;

    assert_eq!(
        check(&report_with(repeated)),
        vec![RowDefect::RepeatedKey {
            key: "processors".to_owned()
        }]
    );
}

#[test]
fn a_key_repeated_inside_a_nested_object_is_not_the_rows_key() {
    // Top level only: a nested object's members are that object's keys, and
    // repeating one there is a different question. `policies` renders arbitrary
    // policy names, so a name colliding with a row key is possible.
    let nested = r#"{"reason":"x-probe-topology","processors":16,"policies":{"processors":2}}"#;

    assert_eq!(check(&report_with(nested)), Vec::new());
}

#[test]
fn the_rows_keys_are_read_at_the_top_level_only() {
    let row = clean_row();

    assert_eq!(
        keys(&row),
        vec![
            "reason",
            "arch",
            "processors",
            "efficiency_classes",
            "caches",
            "policies",
            "cross_check",
            "disagreements",
            "parse_incomplete",
        ],
        "the nested `level`, `domains` and `single` are not the ROW's keys"
    );
}

#[test]
fn the_row_accessor_declines_an_ambiguous_or_malformed_report() {
    let one = report_with(&clean_row());
    let two = format!("{}\n{}", report_with(&clean_row()), clean_row());
    let malformed = report_with(r#"{"unbalanced":["#);

    assert_eq!(row(&one), Some(clean_row().as_str()));
    assert_eq!(row("prose only"), None);
    assert_eq!(row(&two), None);
    assert_eq!(row(&malformed), None);
}

#[test]
fn a_defect_is_reported_once_per_extra_rendering_of_a_key() {
    // **One entry per EXTRA rendering, so the count reads as how many times the
    // row said it again.** Three renderings of `a` give two defects, not one and
    // not three.
    //
    // This was named `..._once_per_repeated_key_rather_than_per_occurrence` and
    // opened by claiming de-duplication the code does not do -- while asserting
    // the per-occurrence behaviour its own failure message describes. A reader
    // taking the name for the contract got it backwards. The assertion was
    // right; the name and the comment were the defect.
    let thrice = r#"{"a":1,"a":2,"a":3,"b":1,"b":2}"#;

    assert_eq!(
        check(&report_with(thrice)),
        vec![
            RowDefect::RepeatedKey {
                key: "a".to_owned()
            },
            RowDefect::RepeatedKey {
                key: "a".to_owned()
            },
            RowDefect::RepeatedKey {
                key: "b".to_owned()
            },
        ],
        "one entry per EXTRA rendering, so the count reads as how many times the \
         row said it again"
    );
}
