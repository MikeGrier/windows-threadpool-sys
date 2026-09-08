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

use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};

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

/// Compose a report and emit it, **including when composing it panics**.
///
/// Every probe's `main` is one call to this. The buffer is owned here rather
/// than inside the renderer so that a measurement which aborts part-way still
/// prints what it had already established.
///
/// That is not hypothetical bookkeeping. These probes call into measurements
/// documented to panic -- `worker_context`'s impersonating observation panics if
/// the token cannot be duplicated or applied, or if the worker never reports --
/// and each renderer composes several completed findings *before* reaching one.
/// Printing line-by-line used to make that automatic: whatever had been measured
/// was already on the terminal. Buffering the whole report to hand it to a
/// [`Report`] silently gave that up, and for an instrument the point of which is
/// that a failure be diagnosable, how far it got is exactly the information
/// worth keeping.
///
/// The panic is resumed afterwards, so the exit status and the message are
/// unchanged; the partial report is added to them, not substituted for them.
///
/// **This restores the streaming property for unwinding panics only, and that
/// bound is known rather than overlooked.** A termination that does not unwind
/// still loses the buffer, where printing line-by-line would have kept it: Ctrl-C
/// (the default Windows console handler terminates the process outright), and an
/// abort from a panic raised during unwinding. The case that costs most is
/// `probe-cancel-io`, which can run four attempts at a five-second watchdog --
/// so about twenty seconds, precisely when the wedge it hunts for occurs, which
/// is precisely when a reader interrupts it.
///
/// Fixing it properly means the renderers writing into a [`Report`] as they go
/// rather than into a `String`, which keeps [`Captured`] working for tests and
/// streams for real runs. That is a different design rather than an oversight in
/// this one -- it changes every renderer -- so it is queued as its own work:
/// milestone `M1` of [CHECKLIST.md](../CHECKLIST.md), with the reasoning in
/// [DESIGN-NOTES.md](../DESIGN-NOTES.md#d-buffered-report).
pub fn emit_report(render: impl FnOnce(&mut String)) {
    emit_report_to(&mut Stdout, render);
}

/// [`emit_report`] against an arbitrary sink.
///
/// Exists so the catch-emit-resume logic is what a test executes, rather than a
/// second copy of that shape written in the test. The first version of the test
/// re-implemented it against a [`Captured`] and so would have passed with the
/// `resume_unwind` below deleted -- which would leave a probe printing a partial
/// report and exiting **0**, the failure that looks most like success.
pub fn emit_report_to(report: &mut impl Report, render: impl FnOnce(&mut String)) {
    let mut out = String::new();

    // `AssertUnwindSafe` because the only state crossing the boundary is this
    // buffer, and a partially written report is precisely what is wanted here
    // rather than a hazard to be guarded against.
    let outcome = catch_unwind(AssertUnwindSafe(|| render(&mut out)));

    emit(report, &out);

    if let Err(payload) = outcome {
        resume_unwind(payload);
    }
}

#[cfg(test)]
mod tests;
