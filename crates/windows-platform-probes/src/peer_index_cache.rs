// Copyright (c) Mike Grier.

//! The peer-index probe's machine-readable row (`M8.3`).
//!
//! The measurement is `windows_placement_probe::peer_index_cache`'s; this is
//! only what `probe-peer-index-cache` publishes of it.

use windows_placement_probe::peer_index_cache::{
    CAPACITY, ITEMS, Observation, REPETITIONS, Run, Strategy,
};

use crate::row::{Row, Value};

#[cfg(test)]
mod tests;

/// What the row names itself.
pub const REASON: &str = "x-probe-peer-index-cache";

/// The strategies in the order the row lists them.
const STRATEGIES: [Strategy; 3] = [Strategy::Baseline, Strategy::Cached, Strategy::Warmed];

/// The row a measured report carries: the raw data `observation` holds, and
/// the build that produced it.
///
/// **Raw data**, per `d-every-probe-emits-a-row` in DESIGN-NOTES.md. For the
/// shipping queue and for each model strategy, the median of [`REPETITIONS`]
/// timed passes as whole elapsed nanoseconds, and -- for the strategies -- how
/// many times that pass's consumer and producer read the other side's shared
/// position. Not the per-item cost, the speedups, the batch depths or the
/// verdicts the prose derives from them; a consumer that wants a per-item
/// figure divides by `items`.
///
/// **The shipping queue's read counts are `null`.** It has no counter: the
/// probe fills those two fields with [`ITEMS`] as a floor, which the report's
/// table prints and which is not an observation. Publishing it would put a
/// constant in the row under a name that says it was counted.
///
/// **The elapsed time is recovered from [`Run::nanos_per_item`]**, which is the
/// median pass's elapsed time divided by [`ITEMS`]. A pass is a whole number of
/// nanoseconds far below 2^53, so multiplying back and rounding returns it
/// exactly, which a test checks.
///
/// A strategy absent from `observation` is absent from the row. `measure`
/// always produces all three; an `Observation` built by hand need not.
#[must_use]
pub fn raw_row(observation: &Observation, build: &str) -> Row {
    let strategies: Value = STRATEGIES
        .into_iter()
        .filter_map(|strategy| {
            observation.get(strategy).map(|run| {
                Value::Object(vec![
                    ("strategy", Value::from(strategy.name())),
                    ("median_elapsed_ns", elapsed(&run)),
                    ("consumer_refreshes", count(run.consumer_refreshes)),
                    ("producer_refreshes", count(run.producer_refreshes)),
                ])
            })
        })
        .collect();
    preamble(build)
        .with("declined", Value::Null)
        .with(
            "calibration",
            Value::Object(vec![
                ("median_elapsed_ns", elapsed(&observation.calibration)),
                ("consumer_refreshes", Value::Null),
                ("producer_refreshes", Value::Null),
            ]),
        )
        .with("strategies", strategies)
}

/// The row a report carries when the host declined to be measured.
///
/// Follows topology's `report_unmeasured`: the same row, naming why, rather
/// than no row. `declined` carries the error's text, which is `null` in a
/// measured row; there is no `calibration` or `strategies`, because nothing
/// was timed. `measure` cannot return an error today -- it pins nothing -- so
/// this is the row for a path the type requires rather than one a host
/// reaches.
#[must_use]
pub fn declined_row(error: &std::io::Error, build: &str) -> Row {
    preamble(build).with("declined", error.to_string())
}

/// What both forms carry: where the run happened and what it was set to do.
fn preamble(build: &str) -> Row {
    Row::new(REASON)
        .with("arch", std::env::consts::ARCH)
        .with("build", build)
        .with("items", ITEMS)
        .with("capacity", CAPACITY)
        .with("repetitions", REPETITIONS)
}

/// The median pass's elapsed time, in whole nanoseconds.
fn elapsed(run: &Run) -> Value {
    Value::from((run.nanos_per_item * ITEMS as f64).round() as usize)
}

/// A read count. A side spinning on a full or empty ring re-reads on every
/// spin, so the count has no bound per item; on a 64-bit target the conversion
/// cannot fail, and on a 32-bit one a count past `usize::MAX` saturates.
fn count(reads: u64) -> Value {
    Value::from(usize::try_from(reads).unwrap_or(usize::MAX))
}
