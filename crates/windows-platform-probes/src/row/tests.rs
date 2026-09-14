// Copyright (c) Mike Grier.

//! Tests for the row's value model and its writer.

use super::{Row, Value};

#[test]
#[should_panic(expected = "the row already carries `processors`")]
fn the_writer_refuses_to_render_a_key_twice() {
    // **The one malformation that survives a consumer's parse**, so the writer
    // is made unable to produce it rather than the oracle being left to notice.
    // `serde_json` and `JSON.parse` both accept a repeated key and keep the
    // last, which turns a broken row into an AMBIGUOUS one -- mined rather than
    // discarded. Reported by a review against this public builder.
    let _ = Row::new("x")
        .with("processors", 16_usize)
        .with("processors", 32_usize);
}

#[test]
fn a_row_renders_its_members_in_the_order_they_were_added() {
    let row = Row::new("x-probe-topology")
        .with("arch", "x86_64")
        .with("processors", 16_usize);

    assert_eq!(
        row.render(),
        r#"{"reason":"x-probe-topology","arch":"x86_64","processors":16}"#
    );
}

#[test]
fn the_key_set_is_derived_from_the_value() {
    // The property that lets the well-formedness check stop carrying a census.
    let row = Row::new("x-probe-topology")
        .with("arch", "x86_64")
        .with("cores", 8_usize);

    assert_eq!(row.keys(), vec!["reason", "arch", "cores"]);
}

#[test]
fn a_quote_in_caller_text_cannot_end_the_string_it_is_in() {
    // **The injection fix, on the value that carries caller text.** A failed
    // discovery interpolates an `io::Error`, and an OS message is free to
    // contain a quote.
    let row = Row::new("x-probe-topology").with("error", r#"he said "no" and left"#);

    assert_eq!(
        row.render(),
        r#"{"reason":"x-probe-topology","error":"he said \"no\" and left"}"#
    );
}

#[test]
fn a_brace_in_caller_text_cannot_start_a_second_row() {
    // Measured on PR #88: an `io::Error` containing `{` was selected as the
    // report's machine-readable row, so a reader checked the caller's text
    // instead of the probe's. A brace inside a string is inert -- this asserts
    // the rendering keeps it there.
    let row = Row::new("x-probe-topology").with("error", r#"failed at {"reason":"fake"}"#);
    let rendered = row.render();

    assert!(
        rendered.lines().count() == 1,
        "one line, so there is no second row to select: {rendered}"
    );
    assert_eq!(
        rendered,
        r#"{"reason":"x-probe-topology","error":"failed at {\"reason\":\"fake\"}"}"#
    );
}

#[test]
fn a_newline_in_caller_text_cannot_end_the_row() {
    // The row is one LINE, and a mining pass splits on lines. An unescaped
    // newline would put the rest of an error message on a line of its own,
    // where it is neither the row nor prose.
    let row = Row::new("x-probe-topology").with("error", "first\nsecond\r\nthird");
    let rendered = row.render();

    assert_eq!(rendered.lines().count(), 1, "{rendered}");
    assert_eq!(
        rendered,
        r#"{"reason":"x-probe-topology","error":"first\nsecond\r\nthird"}"#
    );
}

#[test]
fn a_backslash_is_escaped_so_it_cannot_escape_the_quote_after_it() {
    // The subtle one: a message ending in a backslash -- a Windows path, say --
    // would otherwise escape the closing quote and swallow the rest of the row.
    let row = Row::new("x-probe-topology").with("path", r"C:\temp\");

    assert_eq!(
        row.render(),
        r#"{"reason":"x-probe-topology","path":"C:\\temp\\"}"#
    );
}

#[test]
fn a_control_character_is_escaped_to_its_json_form() {
    // A localised OS message can carry one, and JSON forbids them unescaped.
    let row = Row::new("x-probe-topology").with("error", "bell\u{7}null\u{0}");

    assert_eq!(
        row.render(),
        r#"{"reason":"x-probe-topology","error":"bell\u0007null\u0000"}"#
    );
}

#[test]
fn a_tab_uses_its_named_escape_rather_than_the_numeric_one() {
    let row = Row::new("x").with("t", "a\tb");

    assert_eq!(row.render(), r#"{"reason":"x","t":"a\tb"}"#);
}

#[test]
fn the_absent_case_renders_as_null_rather_than_being_omitted() {
    // A consumer can tell `null` from a field this probe is too old to publish,
    // and cannot tell an omission from either.
    let row = Row::new("x").with("highest_numa_node", Option::<usize>::None);

    assert_eq!(row.render(), r#"{"reason":"x","highest_numa_node":null}"#);
}

#[test]
fn a_present_option_renders_as_its_value() {
    let row = Row::new("x").with("highest_numa_node", Some(2_usize));

    assert_eq!(row.render(), r#"{"reason":"x","highest_numa_node":2}"#);
}

#[test]
fn an_empty_list_renders_as_an_empty_list() {
    // Not omitted, for the same reason `null` is not: "no conditions" and "this
    // probe does not publish conditions" are different answers.
    let row = Row::new("x").with("parse_incomplete", Value::List(Vec::new()));

    assert_eq!(row.render(), r#"{"reason":"x","parse_incomplete":[]}"#);
}

#[test]
fn a_list_of_objects_renders_each_member_in_order() {
    let row = Row::new("x").with(
        "caches",
        Value::List(vec![
            Value::Object(vec![
                ("level", Value::Number(1)),
                ("domains", 8_usize.into()),
            ]),
            Value::Object(vec![
                ("level", Value::Number(3)),
                ("domains", 1_usize.into()),
            ]),
        ]),
    );

    assert_eq!(
        row.render(),
        r#"{"reason":"x","caches":[{"level":1,"domains":8},{"level":3,"domains":1}]}"#
    );
}

#[test]
fn a_list_collects_from_an_iterator_of_anything_a_value_accepts() {
    let row = Row::new("x").with(
        "efficiency_classes",
        [0_u8, 1].into_iter().collect::<Value>(),
    );

    assert_eq!(row.render(), r#"{"reason":"x","efficiency_classes":[0,1]}"#);
}

#[test]
fn a_key_is_escaped_too() {
    // Keys are `&'static str` minted in this crate, so none needs escaping
    // today -- but the writer has one path for strings, so a key that ever did
    // is handled rather than being a hole waiting for the first policy name
    // with a quote in it.
    let row = Row::new("x").with("odd\"name", 1_usize);

    assert_eq!(row.render(), r#"{"reason":"x","odd\"name":1}"#);
}
