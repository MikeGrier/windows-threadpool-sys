// Copyright (c) Mike Grier.

//! The queue probe's machine-readable row (`M8.2`).

use super::*;
use crate::row::{Row, Value};

#[cfg(test)]
mod tests;

/// The row a report carries: the raw data `observation` holds, and the build
/// that produced it.
///
/// **Raw data**, per the decision that every probe's normal run emits one
/// (`d-every-probe-emits-a-row` in DESIGN-NOTES.md): what each configuration's
/// timed passes took, in whole nanoseconds -- the median pass, the fastest and
/// the slowest -- with the median pass's refusals, the sampling and the build.
/// The median is the upper of the two middle passes when the count is even, so
/// it is always a pass that happened rather than an average of two. Not the
/// per-operation figures, ratios or scaling the prose derives from them; a
/// consumer that wants those divides by `producers * pushes_per_producer`.
///
/// **The elapsed times are recovered from [`Run`]'s per-operation figures**,
/// each of which is an elapsed time divided by the run's total pushes. A timed
/// pass is a whole number of nanoseconds far below 2^53, so multiplying back and
/// rounding returns it exactly, which a test checks. A configuration that did
/// not measure ([`Run::is_measured`]) publishes `null` for all three.
///
/// A refused command line exits before anything is measured and prints no
/// report, so there is no refusal for this row to carry.
#[must_use]
pub fn raw_row(observation: &Observation, build: &str) -> Row {
    let pushes_per_producer = observation.sampling.pushes_per_producer;
    let runs = |runs: &[Run]| -> Value {
        runs.iter()
            .map(|run| {
                let total = run.producers * pushes_per_producer;
                let elapsed = |per_op: f64| -> Value {
                    if run.is_measured() {
                        Value::from((per_op * total as f64).round() as usize)
                    } else {
                        Value::Null
                    }
                };
                Value::Object(vec![
                    ("shape", Value::from(run.shape)),
                    ("producers", Value::from(run.producers)),
                    ("median_elapsed_ns", elapsed(run.nanos_per_op)),
                    ("fastest_elapsed_ns", elapsed(run.fastest_nanos_per_op)),
                    ("slowest_elapsed_ns", elapsed(run.slowest_nanos_per_op)),
                    // A pass refuses at most once per push, and the pushes are
                    // counted in a `usize`, so the fallback is unreachable.
                    (
                        "refusals",
                        Value::from(usize::try_from(run.refusals).unwrap_or(usize::MAX)),
                    ),
                ])
            })
            .collect()
    };
    Row::new("x-probe-queue-contention")
        .with("arch", std::env::consts::ARCH)
        .with("build", build)
        .with("pushes_per_producer", pushes_per_producer)
        .with("repetitions", observation.sampling.repetitions)
        .with("order", observation.sampling.order.name())
        .with(
            "producer_counts",
            observation
                .sampling
                .producer_counts
                .iter()
                .copied()
                .collect::<Value>(),
        )
        .with("available_parallelism", observation.available_parallelism)
        .with("isolated", runs(&observation.isolated))
        .with("drained", runs(&observation.drained))
}
