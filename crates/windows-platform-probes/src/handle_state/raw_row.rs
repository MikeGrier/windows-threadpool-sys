// Copyright (c) Mike Grier.

//! The handle-state probe's machine-readable row (`M8.8`).

use super::*;
use crate::row::{Row, Value};

#[cfg(test)]
mod tests;

/// What the row names itself.
pub const REASON: &str = "x-probe-handle-state";

/// One interleaved query, as the probe binary ran it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueryObservation {
    /// The query.
    pub query: SingleShot,
    /// Whether it was sent to a duplicate of the enumerating handle.
    pub on_duplicate: bool,
    /// Whether the query itself succeeded.
    pub succeeded: bool,
    /// The names the handle produced before and after it.
    pub cursor: CursorObservation,
}

/// What one run of the probe observed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observation {
    /// Every name, read start to finish on one handle.
    pub ground_truth: Vec<String>,
    /// The source's first call, then its duplicate's continuation.
    pub duplicate: CursorObservation,
    /// The control: one open's first call, then a second open's.
    pub separate_opens: CursorObservation,
    /// Whether the source's next call succeeded after its duplicate closed.
    pub source_survived_close: bool,
    /// Each query in [`QUERIES`].
    pub queries: Vec<QueryObservation>,
}

/// The row a report carries.
///
/// **Raw data**, per `d-every-probe-emits-a-row` in DESIGN-NOTES.md: the
/// buffer size and fixture size that make a cursor stop part-way, the ground
/// truth's names, and for each two-call observation the first call's names and
/// the second's -- or, where the second call failed, `null` names and the Win32
/// error. For each query, whether it succeeded beside the names around it.
///
/// Not shared, independent, disturbed or any other verdict; each compares the
/// names here against the ground truth. The probe has no way to decline: a
/// fixture or handle it cannot create stops it with a panic, and a panic prints
/// no report.
#[must_use]
pub fn raw_row(observation: &Observation, build: &str) -> Row {
    let queries: Value = observation
        .queries
        .iter()
        .map(|query| {
            let mut members = vec![
                ("query", Value::from(query.query.name())),
                ("on_duplicate", Value::from(query.on_duplicate)),
                ("succeeded", Value::from(query.succeeded)),
            ];
            members.extend(cursor(&query.cursor));
            Value::Object(members)
        })
        .collect();
    Row::new(REASON)
        .with("arch", std::env::consts::ARCH)
        .with("build", build)
        .with("buffer_bytes", BUFFER_BYTES)
        .with("fixture_files", FIXTURE_FILES)
        .with("ground_truth", names(&observation.ground_truth))
        .with("duplicate", Value::Object(cursor(&observation.duplicate)))
        .with(
            "separate_opens",
            Value::Object(cursor(&observation.separate_opens)),
        )
        .with("source_survived_close", observation.source_survived_close)
        .with("queries", queries)
}

fn names(names: &[String]) -> Value {
    names.iter().map(String::as_str).collect()
}

/// The two calls of one observation, as members of the object holding them.
fn cursor(observation: &CursorObservation) -> Vec<(&'static str, Value)> {
    let (next, error) = match &observation.other_next {
        Ok(next) => (names(next), Value::Null),
        Err(code) => (Value::Null, Value::from(*code)),
    };
    vec![
        ("first", names(&observation.source_first)),
        ("next", next),
        ("next_error", error),
    ]
}
