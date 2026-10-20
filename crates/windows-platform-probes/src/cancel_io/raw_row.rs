// Copyright (c) Mike Grier.

//! The cancel-io probe's machine-readable row (`M8.13`).

use super::*;
use crate::row::{Row, Value};

#[cfg(test)]
mod tests;

/// What the row names itself.
pub const REASON: &str = "x-probe-cancel-io";

/// The row a report carries.
///
/// **Raw data**, per `d-every-probe-emits-a-row` in DESIGN-NOTES.md: the
/// watchdog each attempt ran under, in milliseconds, then the idle case's
/// outcome and every busy-case attempt's, in order.
///
/// Each outcome is `cancelled`, `not_found` with the error code the call
/// reported, or **`wedged`** -- a call the watchdog abandoned because it had
/// not returned. A wedge is published like any other outcome rather than
/// dropped, because it is the finding; its `code` is `null`, since a call that
/// never returned reported nothing.
///
/// Not whether any attempt wedged, nor the conclusion; both read the outcomes
/// here.
#[must_use]
pub fn raw_row(idle: CancelOutcome, busy: &[CancelOutcome], build: &str) -> Row {
    Row::new(REASON)
        .with("arch", std::env::consts::ARCH)
        .with("build", build)
        .with(
            "watchdog_ms",
            usize::try_from(WATCHDOG.as_millis()).unwrap_or(usize::MAX),
        )
        .with("idle", outcome(idle))
        .with("busy", busy.iter().copied().map(outcome).collect::<Value>())
}

fn outcome(outcome: CancelOutcome) -> Value {
    let (name, code) = match outcome {
        CancelOutcome::Cancelled => ("cancelled", None),
        CancelOutcome::NotFound { code } => ("not_found", Some(code)),
        CancelOutcome::Wedged => ("wedged", None),
    };
    Value::Object(vec![
        ("outcome", Value::from(name)),
        ("code", Value::from(code)),
    ])
}
