// Copyright (c) 2026 Mike Grier
//! Tests for the report sink.
//!
//! These are small, and that is the point: the sink's whole job is to be the
//! seam that lets a probe's *findings* be asserted rather than eyeballed. What
//! is worth pinning here is the seam's own behaviour, so that a test written
//! against a probe's report can trust what it is reading.

use super::{Captured, Report, emit, writeln_to};

#[test]
fn a_captured_report_keeps_its_lines_in_order() {
    // Order is the property a probe's report depends on most: its tables are
    // rows under a header, and a sink that reordered them would turn a correct
    // measurement into a wrong one.
    let mut captured = Captured::default();
    captured.line("header");
    captured.line("row one");
    captured.line("row two");

    assert_eq!(captured.lines, ["header", "row one", "row two"]);
    assert_eq!(captured.text(), "header\nrow one\nrow two");
}

#[test]
fn emitting_a_block_gives_the_report_one_line_at_a_time() {
    // What makes a captured report addressable by line rather than by substring
    // search -- so a test can say "the third row" instead of hoping a phrase is
    // unique in the document.
    let mut captured = Captured::default();
    emit(&mut captured, "one\ntwo\nthree");

    assert_eq!(captured.lines, ["one", "two", "three"]);
}

#[test]
fn a_trailing_newline_does_not_become_an_extra_blank_line() {
    // A renderer may end its block with a newline or without one, and the two
    // must look the same to a reader. Otherwise every `render_*` function would
    // have to agree on a convention that nothing enforces, and the first one to
    // drift would add a blank line nobody could account for.
    let mut with = Captured::default();
    let mut without = Captured::default();
    emit(&mut with, "one\ntwo\n");
    emit(&mut without, "one\ntwo");

    assert_eq!(with.lines, without.lines);
}

#[test]
fn an_interior_blank_line_survives() {
    // The other side of the previous test, and the reason it cannot simply
    // filter empties: these reports use blank lines to separate sections, so a
    // sink that swallowed them would run the tables together.
    let mut captured = Captured::default();
    emit(&mut captured, "section\n\nnext");

    assert_eq!(captured.lines, ["section", "", "next"]);
}

#[test]
fn writeln_to_appends_a_line_rather_than_replacing_the_buffer() {
    // `writeln_to` exists so the `let _ =` on an infallible `write!` is stated
    // once rather than at every call site; this checks it composes, since a
    // renderer calls it dozens of times in sequence.
    let mut out = String::new();
    writeln_to(&mut out, "first");
    writeln_to(&mut out, "second");

    assert_eq!(out, "first\nsecond\n");
}
