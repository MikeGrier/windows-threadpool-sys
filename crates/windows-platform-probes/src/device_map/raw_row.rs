// Copyright (c) Mike Grier.

//! The device-map probe's machine-readable row (`M8.10`).

use super::*;
use crate::row::{Row, Value};

#[cfg(test)]
mod tests;

/// What the row names itself.
pub const REASON: &str = "x-probe-device-map";

/// Why a run declines: the one condition the probe checks before measuring.
pub const NO_FREE_DRIVE_LETTER: &str = "no free drive letter on this host";

/// The row a measured report carries.
///
/// **Raw data**, per `d-every-probe-emits-a-row` in DESIGN-NOTES.md: the
/// target this run defined the letter as, and for the process's own session
/// and the anonymous one, the letter, every target it resolved to (`null` when
/// it resolved to nothing) and the logon session of the token in effect.
///
/// The logon session is a LUID, published as the `high:low` hexadecimal text
/// the prose prints: it is an identifier, its high half is signed, and the row
/// carries no signed numbers. `null` when it could not be read.
///
/// Not whether impersonation changed the map, whether the sessions differ, or
/// whether the claim was exclusive; each compares the figures here.
#[must_use]
pub fn raw_row(finding: &DeviceMapFinding, build: &str) -> Row {
    preamble(build)
        .with("declined", Value::Null)
        .with("target", finding.target.as_str())
        .with("own_session", session(&finding.own_session))
        .with("anonymous_session", session(&finding.anonymous_session))
}

/// The row a report carries when the probe could not set up its fixture.
///
/// Follows topology's `report_unmeasured`: the same row with `declined` naming
/// why, and nothing the measurement would have produced.
#[must_use]
pub fn declined_row(reason: &str, build: &str) -> Row {
    preamble(build).with("declined", reason)
}

fn preamble(build: &str) -> Row {
    Row::new(REASON)
        .with("arch", std::env::consts::ARCH)
        .with("build", build)
}

fn session(observation: &MapObservation) -> Value {
    Value::Object(vec![
        ("letter", Value::from(observation.letter.as_str())),
        (
            "targets",
            if observation.is_found() {
                observation.entries().into_iter().collect()
            } else {
                Value::Null
            },
        ),
        (
            "logon_session",
            Value::from(
                observation
                    .logon_session
                    .map(|(low, high)| format!("{high:08x}:{low:08x}")),
            ),
        ),
    ])
}
