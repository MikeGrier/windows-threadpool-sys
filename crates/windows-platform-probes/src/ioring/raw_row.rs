// Copyright (c) Mike Grier.

//! The `IoRing` probe's machine-readable row (`M8.11`).

use super::*;
use crate::row::{Row, Value};

#[cfg(test)]
mod tests;

/// What the row names itself.
pub const REASON: &str = "x-probe-ioring";

/// Why a run declines: the one condition the probe checks before measuring.
pub const NO_USABLE_IORING: &str = "this host has no usable IoRing";

/// The row a measured report carries.
///
/// **Raw data**, per `d-every-probe-emits-a-row` in DESIGN-NOTES.md. For
/// registration: whether reads through index 0 and index 1 succeeded after a
/// second registration of one handle. For thread agnosticism: whether the read
/// was still outstanding when its submitter ended, its result code, and whether
/// it transferred the fill byte. Either is `null` when that measurement found
/// no ring after the host had reported one.
///
/// The result code is an HRESULT, published as the `0x` hexadecimal text the
/// prose prints: it is a code rather than a quantity, and it is signed, which
/// the row carries no number for.
///
/// Not replaces, appends or survives; each is a comparison over the figures
/// here.
#[must_use]
pub fn raw_row(
    registration: &IoRingSupport<RegistrationObservation>,
    agnosticism: &IoRingSupport<ThreadAgnosticism>,
    build: &str,
) -> Row {
    let registration = match registration {
        IoRingSupport::Unavailable => Value::Null,
        IoRingSupport::Measured(observed) => Value::Object(vec![
            (
                "index_zero_usable_after_second",
                Value::from(observed.index_zero_usable_after_second),
            ),
            (
                "index_one_usable_after_second",
                Value::from(observed.index_one_usable_after_second),
            ),
        ]),
    };
    let agnosticism = match agnosticism {
        IoRingSupport::Unavailable => Value::Null,
        IoRingSupport::Measured(observed) => Value::Object(vec![
            (
                "pending_at_submitter_exit",
                Value::from(observed.pending_at_submitter_exit),
            ),
            (
                "result_code",
                Value::from(format!("{:#010x}", observed.result_code)),
            ),
            ("filled", Value::from(observed.filled)),
        ]),
    };
    preamble(build)
        .with("declined", Value::Null)
        .with("registration", registration)
        .with("thread_agnosticism", agnosticism)
}

/// The row a report carries when the host has no ring to measure.
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
