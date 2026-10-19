// Copyright (c) Mike Grier.

//! The pool-growth probe's machine-readable row (`M8.5`).

use super::*;
use crate::row::{Row, Value};

#[cfg(test)]
mod tests;

/// What the row names itself.
pub const REASON: &str = "x-probe-pool-growth";

/// The row a report carries.
///
/// **Raw data**, per `d-every-probe-emits-a-row` in DESIGN-NOTES.md. For each
/// growth run, under its record name: the configuration (maximum, submitted,
/// runs-long) and what was observed -- callbacks started while blocked, the
/// distinct threads they ran on, and every arrival in microseconds from
/// submission. For the raise: the two maxima, the submissions, how many
/// callbacks had started when the maximum was raised, whether an extra one
/// started within the settle window, and the elapsed nanoseconds from the
/// raise to that start -- or to the window's end, when `took_effect` is false.
///
/// Not the verdicts the prose prints (saturated, one thread each) nor the
/// slowest arrival or largest gap; each is a comparison or a maximum over the
/// figures here.
///
/// The probe has no way to decline: every measurement either completes or
/// panics, and a panic prints no report.
#[must_use]
pub fn raw_row(
    growth: &[(GrowthRun, GrowthObservation)],
    raise: (RaiseRun, RaiseObservation),
    build: &str,
) -> Row {
    let growth: Value = growth
        .iter()
        .map(|(run, observed)| {
            Value::Object(vec![
                ("run", Value::from(run.name)),
                ("maximum", Value::from(observed.maximum)),
                ("submitted", Value::from(observed.submitted)),
                ("runs_long", Value::from(run.runs_long)),
                (
                    "started_while_blocked",
                    Value::from(observed.started_while_blocked),
                ),
                ("distinct_threads", Value::from(observed.distinct_threads)),
                (
                    "arrivals_us",
                    observed.arrivals_us.iter().copied().map(wide).collect(),
                ),
            ])
        })
        .collect();
    let (plan, observed) = raise;
    Row::new(REASON)
        .with("arch", std::env::consts::ARCH)
        .with("build", build)
        .with("growth", growth)
        .with(
            "raise",
            Value::Object(vec![
                ("base_max", Value::from(plan.base_max)),
                ("raised_max", Value::from(plan.raised_max)),
                ("submitted", Value::from(plan.submissions)),
                (
                    "started_before_raise",
                    Value::from(observed.started_before_raise),
                ),
                ("took_effect", Value::from(observed.took_effect)),
                ("elapsed_ns", wide(observed.delay.as_nanos())),
            ]),
        )
}

/// A `u128` time, as a count. Saturates past `u64::MAX` nanoseconds -- about
/// 584 years -- and then as `From<u64>` does.
fn wide(value: u128) -> Value {
    Value::from(u64::try_from(value).unwrap_or(u64::MAX))
}
