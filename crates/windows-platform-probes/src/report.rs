// Copyright (c) 2026 Mike Grier
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
//! these probes have only ever written to stdout -- every one of their findings
//! is a finding, and none of them is a diagnostic competing with the report for
//! a reader's attention. Adding a second stream here would be inventing a
//! distinction the tools do not make.
//!
//! # This is not yet used by every probe
//!
//! The two probes added alongside this module route through it. The other
//! twelve predate it and still print directly; converting them is queued rather
//! than done here, so that this change stays reviewable and each conversion can
//! be checked against its probe's real output.

use std::fmt::Write as _;

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

/// Append `text` and a newline to `out`, discarding the impossible error.
///
/// Every `render_*` function in these probes writes into a `String`, whose
/// `fmt::Write` impl cannot fail, so each call site would otherwise carry a
/// `let _ =` that says nothing. This says it once.
pub fn writeln_to(out: &mut String, text: &str) {
    let _ = writeln!(out, "{text}");
}

#[cfg(test)]
mod tests;
