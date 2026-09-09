// Copyright (c) Mike Grier.
//! Tests for the report sink.
//!
//! These are small, and that is the point: the sink's whole job is to be the
//! seam that lets a probe's *findings* be asserted rather than eyeballed. What
//! is worth pinning here is the seam's own behaviour, so that a test written
//! against a probe's report can trust what it is reading.

use std::fmt::Write as _;
use std::panic::{AssertUnwindSafe, catch_unwind};

use super::{Captured, LineSink, Report, emit, emit_report_to};

// --- the fmt::Write sink ----------------------------------------------------
//
// `LineSink` exists so a renderer can `writeln!` straight into a `Report`, and
// the whole of its difficulty is that `fmt::Write` does not speak in lines
// while `Report` does. `write_str` receives whatever slices the formatting
// machinery happens to produce, so these pin the reassembly rather than the
// happy path: fragments below a line, several lines in one call, and the tail
// that arrives without a newline behind it.

#[test]
fn a_line_sink_emits_one_line_per_newline() {
    // The ordinary case, and the one every renderer relies on.
    let mut captured = Captured::default();
    {
        let mut sink = LineSink::new(&mut captured);
        writeln!(sink, "header").expect("writing to a line sink cannot fail");
        writeln!(sink, "row {}", 1).expect("writing to a line sink cannot fail");
        writeln!(sink, "row {}", 2).expect("writing to a line sink cannot fail");
        sink.finish();
    }

    assert_eq!(captured.lines, ["header", "row 1", "row 2"]);
}

#[test]
fn a_line_sink_joins_fragments_written_below_a_line() {
    // `write!` without a newline, repeatedly, is how a renderer builds a row
    // from parts -- and it is what `fmt::Arguments` does internally for a
    // format string with interpolations. Each fragment must accumulate rather
    // than becoming a line of its own.
    let mut captured = Captured::default();
    {
        let mut sink = LineSink::new(&mut captured);
        write!(sink, "one ").expect("writing to a line sink cannot fail");
        write!(sink, "two ").expect("writing to a line sink cannot fail");
        writeln!(sink, "three").expect("writing to a line sink cannot fail");
        sink.finish();
    }

    assert_eq!(captured.lines, ["one two three"]);
}

#[test]
fn a_line_sink_splits_a_multi_line_write() {
    // One `write_str` carrying several newlines must still produce several
    // lines, because a renderer may hand over a pre-composed block.
    let mut captured = Captured::default();
    {
        let mut sink = LineSink::new(&mut captured);
        write!(sink, "a\nb\nc\n").expect("writing to a line sink cannot fail");
        sink.finish();
    }

    assert_eq!(captured.lines, ["a", "b", "c"]);
}

#[test]
fn a_line_sink_emits_a_final_line_that_has_no_newline() {
    // The case that decides whether this adapter can be trusted at all. A
    // renderer whose last call is `write!` rather than `writeln!` has a
    // complete line sitting in the buffer with nothing to flush it, and
    // dropping it would truncate the report by exactly one row -- a defect
    // visible only as a missing last line.
    let mut captured = Captured::default();
    {
        let mut sink = LineSink::new(&mut captured);
        writeln!(sink, "kept").expect("writing to a line sink cannot fail");
        write!(sink, "also kept").expect("writing to a line sink cannot fail");
        sink.finish();
    }

    assert_eq!(captured.lines, ["kept", "also kept"]);
}

#[test]
fn finishing_a_line_sink_twice_adds_nothing() {
    // `emit_report_to` finishes the sink, and a renderer may reasonably finish
    // its own; the second call must not invent an empty line.
    let mut captured = Captured::default();
    {
        let mut sink = LineSink::new(&mut captured);
        writeln!(sink, "only").expect("writing to a line sink cannot fail");
        sink.finish();
        sink.finish();
    }

    assert_eq!(captured.lines, ["only"]);
}

#[test]
fn a_line_sink_preserves_a_blank_line() {
    // Blank lines are structure in these reports -- they separate a table from
    // the prose reading it -- so an empty line between two newlines is content,
    // not noise to collapse.
    let mut captured = Captured::default();
    {
        let mut sink = LineSink::new(&mut captured);
        writeln!(sink, "section").expect("writing to a line sink cannot fail");
        writeln!(sink).expect("writing to a line sink cannot fail");
        writeln!(sink, "next").expect("writing to a line sink cannot fail");
        sink.finish();
    }

    assert_eq!(captured.lines, ["section", "", "next"]);
}

#[test]
fn a_line_sink_matches_what_emit_produces_for_the_same_text() {
    // The correspondence that lets M1.2 be a mechanical conversion: writing
    // through the sink must give a reader exactly what composing a `String` and
    // handing it to `emit` gives today. If these disagreed, converting a
    // renderer would silently change its report.
    let block = "header\n\nrow 1\nrow 2\ntrailing";

    let mut through_emit = Captured::default();
    emit(&mut through_emit, block);

    let mut through_sink = Captured::default();
    {
        let mut sink = LineSink::new(&mut through_sink);
        write!(sink, "{block}").expect("writing to a line sink cannot fail");
        sink.finish();
    }

    assert_eq!(through_sink.lines, through_emit.lines);
}

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
