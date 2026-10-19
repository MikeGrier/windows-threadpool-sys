// Copyright (c) Mike Grier.

//! The long-path probes' machine-readable row (`M8.6`).

use super::*;
use crate::row::{Row, Value};

#[cfg(test)]
mod tests;

/// What the row names itself -- **one name for both binaries**. Their finding
/// is the difference between two runs, so a reader pairs two rows of one shape
/// told apart by `manifest_aware`, rather than two shapes.
pub const REASON: &str = "x-probe-long-path";

/// The row a report carries.
///
/// **Raw data**, per `d-every-probe-emits-a-row` in DESIGN-NOTES.md: whether
/// this binary declares `longPathAware`, the machine's `LongPathsEnabled`
/// (`null` when the run was refused before it could be read), the content
/// ceiling the attempts are compared against, and every attempt -- its shape by
/// record name, the resolved length in UTF-16 code units, whether
/// `CreateFileW` opened it, and the Win32 error when it did not (`null` when it
/// opened, since none was read).
///
/// `declined` carries the apparatus error when there is one -- an over-long
/// temporary directory refused before anything ran, or a tree that could not be
/// built -- and is `null` otherwise. When it is set, the attempts say nothing
/// about the machine, which is what the prose says too.
///
/// Not whether an attempt was over the ceiling, which `resolved_len` and
/// `ceiling` give, nor the verdict the prose draws.
#[must_use]
pub fn raw_row(observation: &Observation, build: &str) -> Row {
    let attempts: Value = observation
        .attempts
        .iter()
        .map(|attempt| {
            Value::Object(vec![
                ("shape", Value::from(attempt.shape.name())),
                ("resolved_len", Value::from(attempt.resolved_len)),
                ("opened", Value::from(attempt.opened)),
                (
                    "error",
                    Value::from((!attempt.opened).then_some(attempt.error)),
                ),
            ])
        })
        .collect();
    Row::new(REASON)
        .with("arch", std::env::consts::ARCH)
        .with("build", build)
        .with("manifest_aware", observation.manifest_aware)
        .with("registry_enabled", observation.registry_enabled)
        .with("declined", observation.apparatus_error.clone())
        .with("ceiling", MAX_PATH_CONTENT)
        .with("attempts", attempts)
}
