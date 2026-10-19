// Copyright (c) Mike Grier.

//! The error-mode probe's machine-readable row (`M8.7`).

use super::*;
use crate::row::{Row, Value};

#[cfg(test)]
mod tests;

/// What the row names itself.
pub const REASON: &str = "x-probe-error-mode";

/// What one run of the probe observed, gathered in the order it was measured.
///
/// The order is part of the measurement -- the last observation permanently
/// alters the process -- so the binary fills this as it renders rather than
/// measuring ahead of the report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observation {
    /// Each bit in [`PROBED_BITS`] attempted on its own.
    pub bits: Vec<BitOutcome>,
    /// The thread mode read back after attempting [`COMBINED_ATTEMPT`] from a
    /// zero baseline.
    pub combined_read_back: u32,
    /// The process mode against the thread mode, with a process bit installed.
    pub process_versus_thread: ProcessVersusThread,
    /// The process mode before setting the alignment bit, and after trying to
    /// restore it.
    pub alignment: (u32, u32),
}

/// The row a report carries.
///
/// **Raw data**, per `d-every-probe-emits-a-row` in DESIGN-NOTES.md, every mode
/// as the number Windows returned: for each bit on its own, whether
/// `SetThreadErrorMode` succeeded, `GetLastError` when it did not (`null` when
/// it succeeded, since none was read) and the mode read back; the mask the
/// combined call attempted and what was read back after it; the process bit,
/// process mode and thread mode read together; and the process mode before
/// and after the alignment bit's restore attempt.
///
/// Not the verdicts -- settable, silently dropped, independent, sticky -- nor
/// the settable mask, each a comparison over the figures here. The probe has
/// no way to decline: every call either answers or the probe panics, and a
/// panic prints no report.
#[must_use]
pub fn raw_row(observed: &Observation, build: &str) -> Row {
    let bits: Value = observed
        .bits
        .iter()
        .map(|outcome| {
            Value::Object(vec![
                ("bit", Value::from(outcome.bit)),
                ("set_ok", Value::from(outcome.set_ok)),
                (
                    "last_error",
                    Value::from((!outcome.set_ok).then_some(outcome.last_error)),
                ),
                ("read_back", Value::from(outcome.read_back)),
            ])
        })
        .collect();
    let versus = observed.process_versus_thread;
    let (before, after) = observed.alignment;
    Row::new(REASON)
        .with("arch", std::env::consts::ARCH)
        .with("build", build)
        .with("bits", bits)
        .with(
            "combined",
            Value::Object(vec![
                ("attempted", Value::from(COMBINED_ATTEMPT)),
                ("read_back", Value::from(observed.combined_read_back)),
            ]),
        )
        .with(
            "process_versus_thread",
            Value::Object(vec![
                ("process_bit", Value::from(versus.process_bit)),
                ("process_mode", Value::from(versus.process_mode)),
                ("thread_mode", Value::from(versus.thread_mode)),
            ]),
        )
        .with(
            "alignment_at_process_scope",
            Value::Object(vec![
                ("before", Value::from(before)),
                ("after_restore", Value::from(after)),
            ]),
        )
}
