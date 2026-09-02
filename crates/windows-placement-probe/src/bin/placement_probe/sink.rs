// Copyright (c) 2026 Mike Grier
//! The one place this tool writes.
//!
//! # Why an abstraction for something as simple as printing
//!
//! Every line this binary produces used to go straight to `println!` or
//! `eprintln!` from wherever it was composed, across roughly fifty sites. That
//! ties two unrelated concerns together: *what the text says* and *where it
//! goes*. Neither can then be exercised without the other, so the only way to
//! check the wording of the collection notice -- the paragraph a runner reads
//! before consenting to publish facts about their machine -- was to run the
//! process and capture its stdout.
//!
//! That matters more here than in most tools. The notice is a disclosure, its
//! exact wording is the thing under review, and this crate already renders its
//! *record* report to a `String` for the same reason. Output was the one part
//! that had not caught up.
//!
//! # What this is, and what it deliberately is not
//!
//! A sink, not a logging framework. [`Sink`] has two methods because this tool
//! writes to two streams and the distinction is real: the report goes to stdout
//! where a runner can pipe it, and problems go to stderr so a pipeline does not
//! swallow them. There are no levels, no filtering, and no formatting policy --
//! the callers still own their text.
//!
//! Content is composed by `render_*` functions that append to a `&mut String`
//! and never touch a stream, matching the idiom the record report already uses.
//! A test calls those directly and asserts on the result; only `main` holds a
//! [`Stdio`], and the test-only `Captured` stands in for it where a test needs
//! to observe what a whole path emitted rather than what one renderer returned.

/// Somewhere this tool's output can go.
pub trait Sink {
    /// Emit one line of the report proper.
    fn line(&mut self, text: &str);

    /// Emit one line describing something that went wrong.
    ///
    /// Separate from [`Sink::line`] because the two streams are separate: a
    /// runner pipes the report somewhere, and a problem that travelled with it
    /// would be pasted into a discussion thread instead of being seen.
    fn problem(&mut self, text: &str);
}

/// The real streams.
pub struct Stdio;

impl Sink for Stdio {
    fn line(&mut self, text: &str) {
        println!("{text}");
    }

    fn problem(&mut self, text: &str) {
        eprintln!("{text}");
    }
}

/// A sink that keeps what it was given.
///
/// The two streams are kept *separately*, because a test asserting that a
/// problem was reported must not be satisfied by the same words appearing in
/// the report -- which is the confusion the two streams exist to prevent.
///
/// Test-only, and gated rather than merely unused in a release build: a second
/// [`Sink`] implementation that ships is a destination this tool can be pointed
/// at, and this one silently swallows everything. Compiling it out says plainly
/// that it is a stand-in for the streams during a test and not an alternative
/// to them.
#[cfg(test)]
#[derive(Debug, Default)]
pub struct Captured {
    /// Lines written to the report stream, in order.
    pub lines: Vec<String>,
    /// Lines written to the problem stream, in order.
    pub problems: Vec<String>,
}

#[cfg(test)]
impl Sink for Captured {
    fn line(&mut self, text: &str) {
        self.lines.push(text.to_owned());
    }

    fn problem(&mut self, text: &str) {
        self.problems.push(text.to_owned());
    }
}

#[cfg(test)]
impl Captured {
    /// The report stream as one string, as a reader would see it.
    #[must_use]
    pub fn report(&self) -> String {
        self.lines.join("\n")
    }
}

/// Write a rendered block to `sink`, one line at a time.
///
/// The `render_*` functions produce a whole block with embedded newlines and a
/// [`Sink`] speaks in lines, so this is the join between them. Splitting rather
/// than passing the block through keeps the capturing sink line-addressable,
/// which is what lets a test say "the third line is the topology" instead of
/// matching a substring against the whole document.
///
/// A trailing newline on `block` does not produce an extra empty line, because
/// `str::lines` does not yield one -- so a renderer may end its block either way
/// without changing what a reader sees.
pub fn emit(sink: &mut impl Sink, block: &str) {
    for line in block.lines() {
        sink.line(line);
    }
}
