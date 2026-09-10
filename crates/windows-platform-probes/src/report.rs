// Copyright (c) Mike Grier.
//! The one place a probe writes.
//!
//! # Why an abstraction for something as simple as printing
//!
//! The repository's rule is that a tool introduces an output abstraction at its
//! *first* output site, so that the storage target and the formatting stay
//! separable from the call sites that compose the content. A probe that calls
//! `println!` from fifty places has welded the two together: its findings can
//! only be observed by running the process and capturing a stream, so nothing
//! about its report can be asserted, diffed against a previous run, or written
//! anywhere but a terminal.
//!
//! That is a real loss for a probe specifically. These binaries exist so a
//! claim in a design note can be **re-run rather than re-argued**, which means
//! their output is evidence -- and evidence that can only be eyeballed is
//! weaker than evidence a test can read.
//!
//! # What this is, and what it deliberately is not
//!
//! A sink, not a logging framework: one method, no levels, no filtering, no
//! formatting policy. Callers still own their text.
//!
//! Only one stream, unlike the placement probe's near-identical sink, because
//! nothing a probe puts in its *report* is a diagnostic: every line of it is a
//! finding, so a `problem` method would have no callers here.
//!
//! The crate does emit one diagnostic, and it is the exception that shows why
//! the split is unnecessary rather than one that undermines it.
//! `Impersonation::drop` warns on stderr when `RevertToSelf` fails during an
//! unwind, because panicking from `Drop` mid-unwind would abort and replace a
//! diagnosable failure with one that explains nothing. That warning is not part
//! of any report and must not be: it belongs to the process, not to the
//! measurement, and stderr already separates it. Routing it through this sink
//! would mix it into the evidence stdout carries.
//!
//! # Every probe routes through this
//!
//! Every probe in this crate, with no exceptions -- stated without a count on
//! purpose, so the claim stays true as probes are added. Each conversion was
//! checked by capturing the probe's output before and after and requiring every
//! pre-existing line to match, in the same order. Not byte-for-byte: the
//! conversion also prepends the host banner, which is the one deliberate
//! difference and the only one permitted.
//!
//! **That check has to be positional**, which is worth recording because the
//! obvious tool is not. The defect a conversion introduces is a helper that
//! still writes to stdout while its caller composes a string: every line still
//! appears, but the helper's lines arrive *first*, so the report is reordered.
//! PowerShell's `Compare-Object` compares collections as sets and calls that
//! identical -- it passed three genuinely broken probes here before the
//! comparison was redone line-by-line.

/// Somewhere a probe's report can go.
pub trait Report {
    /// Emit one line.
    fn line(&mut self, text: &str);
}

/// The real stream.
pub struct Stdout;

impl Report for Stdout {
    fn line(&mut self, text: &str) {
        println!("{text}");
    }
}

/// A report that keeps what it was given.
///
/// The point of the whole abstraction: a probe's findings become a value a test
/// can read, rather than bytes only a terminal ever sees.
#[derive(Debug, Default)]
pub struct Captured {
    /// Lines written, in order.
    pub lines: Vec<String>,
}

impl Report for Captured {
    fn line(&mut self, text: &str) {
        self.lines.push(text.to_owned());
    }
}

impl Captured {
    /// The report as one string, as a reader would see it.
    #[must_use]
    pub fn text(&self) -> String {
        self.lines.join("\n")
    }
}

/// A [`Report`] a renderer can `writeln!` into directly.
///
/// is arithmetic. Every renderer writes through `writeln!(out, ...)` against a
/// `String`, at **332 sites** across this crate; a sink method taking
/// `fmt::Arguments` would have been explicit but would have rewritten every one
/// of them, while `String` already implements `fmt::Write`, so a sink that does
/// too lets those sites stand untouched and moves only 18 renderer signatures.
/// signatures. The recorded reasoning is in
/// [DESIGN-NOTES.md](../DESIGN-NOTES.md#d-streaming-report).
///
/// # Lines are reassembled here, because `fmt::Write` does not speak in them
///
/// `write_str` receives whatever slices the formatting machinery hands it: a
/// fragment of a line, several lines at once, or a bare `"\n"`. [`Report`]
/// speaks in whole lines and [`Captured`] is addressable by line, so this holds
/// a partial line until a `\n` arrives and emits exactly the completed ones.
///
/// **A renderer that ends without a trailing newline still has its last line
/// emitted**, by [`LineSink::finish`], which `emit_report_to` calls. Dropping
/// that trailing fragment would silently truncate any report whose final
/// `write!` was not a `writeln!` -- a defect that would show only as a missing
/// last row.
pub struct LineSink<'a> {
    report: &'a mut dyn Report,
    partial: String,
}

impl<'a> LineSink<'a> {
    /// Wrap a [`Report`] so renderers can write formatted text into it.
    pub fn new(report: &'a mut dyn Report) -> Self {
        Self {
            report,
            partial: String::new(),
        }
    }

    /// Emit any text written since the last newline.
    ///
    /// Idempotent: a second call with nothing buffered emits nothing, so a
    /// caller that finishes a sink twice does not add a stray empty line.
    pub fn finish(&mut self) {
        if !self.partial.is_empty() {
            self.report.line(&self.partial);
            self.partial.clear();
        }
    }
}

impl std::fmt::Write for LineSink<'_> {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        // `split('\n')`, not `lines()`. `lines()` cannot distinguish "ends with
        // a newline" from "does not", which is exactly the distinction that
        // decides whether the tail is a completed line or a partial one still
        // being written. `split` always yields one more piece than there are
        // newlines, so the final piece is the remainder by construction --
        // empty when the text ended on a newline.
        let mut pieces = text.split('\n');
        let first = pieces.next().unwrap_or_default();
        self.partial.push_str(first);

        for piece in pieces {
            let line = std::mem::take(&mut self.partial);
            self.report.line(&line);
            self.partial.push_str(piece);
        }

        Ok(())
    }
}

/// Write a rendered block to `report`, one line at a time.
///
/// A `render_*` function produces a whole block with embedded newlines and a
/// [`Report`] speaks in lines, so this is the join between them. Splitting
/// rather than passing the block through keeps [`Captured`] line-addressable,
/// which is what lets a test name a row instead of searching the whole document
/// for a substring.
///
/// A trailing newline does not produce an extra empty line, because
/// `str::lines` does not yield one -- so a renderer may end its block either way
/// without changing what a reader sees.
pub fn emit(report: &mut impl Report, block: &str) {
    for line in block.lines() {
        report.line(line);
    }
}

/// Render a report straight to the sink, a line at a time as it is produced.
///
/// Every probe's `main` is one call to this. The renderer writes into a
/// [`LineSink`], so each completed line reaches the [`Report`] as it is
/// composed rather than when the renderer returns.
///
/// That matters because these renderers **interleave measurement with output**.
/// `probe-cancel-io` writes a heading, runs an attempt against a five-second
/// watchdog, writes its outcome, and repeats -- so with a buffered report a
/// reader who interrupts a run gets nothing, and a run is slowest to finish in
/// exactly the case it was hunting for. Streaming makes the finished lines a
/// reader's regardless of how the process ends.
///
/// # What replaced the catch-and-resume
///
/// This used to compose the whole report into a `String` inside `catch_unwind`,
/// emit the buffer, and resume the panic, which recovered the finished lines
/// for an unwinding panic **and only for that**. A Ctrl-C, which the default
/// Windows console handler serves by terminating the process, or an abort from
/// a panic raised while already unwinding, both discarded the buffer.
///
/// Streaming makes that machinery unnecessary rather than merely redundant: the
/// lines are already out, so there is no buffer to rescue and nothing for a
/// `catch_unwind` to do. Keeping it would have been machinery that no longer
/// earned its place -- and would have kept the misleading implication that
/// partial output depends on the panic unwinding.
///
/// **A panic still loses at most a partial final line**, meaning one on which a
/// renderer had called `write!` without a newline. That is deliberate: flushing
/// it would require a `Drop` on [`LineSink`], and a `Drop` that writes can panic
/// while unwinding, which aborts -- replacing a diagnosable failure with one
/// that explains nothing, the same hazard `Impersonation::drop` documents above.
/// An unterminated fragment is not a finding, so the trade is one-sided.
pub fn emit_report(render: impl FnOnce(&mut dyn std::fmt::Write)) {
    emit_report_to(&mut Stdout, render);
}

/// [`emit_report`] against an arbitrary sink.
///
/// Exists so the streaming behaviour is what a test executes, rather than a
/// second copy of that shape written in the test. That mattered more when this
/// wrapped a `catch_unwind`: the first version of the test re-implemented the
/// catch-emit-resume itself and so would have passed with the `resume_unwind`
/// deleted, leaving a probe printing a partial report and exiting **0** -- the
/// failure that looks most like success. The shape is simpler now, and the
/// reason to share it is unchanged.
pub fn emit_report_to(report: &mut impl Report, render: impl FnOnce(&mut dyn std::fmt::Write)) {
    let mut sink = LineSink::new(report);
    render(&mut sink);
    // Emits a final line the renderer left without a newline. Not reached when
    // `render` panics, which costs at most that fragment -- see `emit_report`.
    sink.finish();
}
#[cfg(test)]
mod tests;
