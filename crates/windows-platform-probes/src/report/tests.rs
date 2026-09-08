// Copyright (c) Mike Grier.
//! Tests for the report sink.
//!
//! These are small, and that is the point: the sink's whole job is to be the
//! seam that lets a probe's *findings* be asserted rather than eyeballed. What
//! is worth pinning here is the seam's own behaviour, so that a test written
//! against a probe's report can trust what it is reading.

use std::fmt::Write as _;
use std::panic::{AssertUnwindSafe, catch_unwind};

use super::{Captured, Report, emit, emit_report_to};

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
fn a_renderer_that_panics_still_has_its_finished_lines_emitted() {
    // The property `emit_report` exists for, exercised through the real
    // function rather than through a second copy of its shape.
    //
    // An earlier version of this test re-implemented catch-emit-resume against a
    // `Captured` and never called into `report` at all. It would have passed
    // with the `resume_unwind` deleted -- and a probe that swallowed its panic
    // would print a partial report and exit **0**, which is the failure that
    // looks most like success. Hence `emit_report_to`: same logic, injectable
    // sink.
    let mut captured = Captured::default();

    let outcome = catch_unwind(AssertUnwindSafe(|| {
        emit_report_to(&mut captured, |out| {
            let _ = writeln!(out, "measured before the failure");
            panic!("a measurement aborted");
        });
    }));

    // Both halves matter, and each fails a different mutation. Without the
    // first, deleting the `emit` leaves the test green; without the second,
    // deleting the `resume_unwind` does.
    assert_eq!(
        captured.lines,
        ["measured before the failure"],
        "what was already established must survive the abort"
    );
    assert!(
        outcome.is_err(),
        "the panic must reach the caller, or the probe exits 0 having failed"
    );
}

#[test]
fn a_renderer_that_returns_normally_reports_every_line_and_does_not_panic() {
    // The other side of the same function: the ordinary path must be unaffected
    // by the machinery that exists for the failing one.
    let mut captured = Captured::default();

    emit_report_to(&mut captured, |out| {
        let _ = writeln!(out, "first");
        let _ = writeln!(out, "second");
    });

    assert_eq!(captured.lines, ["first", "second"]);
}
