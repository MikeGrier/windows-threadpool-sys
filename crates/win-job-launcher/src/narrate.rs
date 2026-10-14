// Copyright (c) 2026 Mike Grier
//! The launcher's one output sink.
//!
//! Every line the launcher prints goes through a [`Narrator`], so where the
//! text goes is decided once, by `main`, and tests can read it back. Writing is
//! best-effort: a narration that cannot be written must never stop the
//! launcher supervising, or killing, the command.

use std::fmt;
use std::io::Write;
use std::time::{Duration, Instant};

#[cfg(test)]
mod tests;

/// The prefix on every line, so the launcher's narration is distinguishable
/// from the command's output in a shared log.
pub const PREFIX: &str = "win-job-launcher";

/// Writes the launcher's trace and error lines to one writer.
pub struct Narrator<W: Write> {
    out: W,
    start: Instant,
    trace: bool,
}

impl<W: Write> Narrator<W> {
    /// A narrator whose clock starts now. Trace lines are written only when
    /// `trace` is set; errors always are.
    pub fn new(out: W, trace: bool) -> Self {
        Self {
            out,
            start: Instant::now(),
            trace,
        }
    }

    /// One step of the narration `--trace` asks for, stamped with the time
    /// since the narrator was made.
    pub fn trace(&mut self, message: fmt::Arguments<'_>) {
        if self.trace {
            let seconds = self.start.elapsed().as_secs_f64();
            let head = format!("{PREFIX} +{seconds:.3}s:");
            self.lines(&head, &head, message);
        }
    }

    /// A failure the caller must see, traced or not. A message of several lines
    /// is written as several lines, each with the prefix; only the first says
    /// `error`.
    pub fn error(&mut self, message: fmt::Arguments<'_>) {
        self.lines(&format!("{PREFIX}: error:"), &format!("{PREFIX}:"), message);
    }

    /// Writes `message` one physical line at a time, so no line reaches a
    /// shared log without its prefix. Best-effort, like every write here.
    fn lines(&mut self, first: &str, rest: &str, message: fmt::Arguments<'_>) {
        let text = message.to_string();
        let mut lines = text.lines();
        let _ = writeln!(self.out, "{first} {}", lines.next().unwrap_or_default());
        for line in lines {
            let _ = writeln!(self.out, "{rest} {line}");
        }
    }

    /// Time since the narrator was made.
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }

    /// The writer, for a test to read back.
    pub fn into_inner(self) -> W {
        self.out
    }
}
