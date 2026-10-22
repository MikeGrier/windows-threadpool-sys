// Copyright (c) Mike Grier.

//! The core-affinity probe's machine-readable row (`M8.4`).
//!
//! The measurement is `windows_placement_probe::core_affinity`'s; this is only
//! what `probe-core-affinity` publishes of it.

use windows_placement_probe::core_affinity::{Measurement, Observation, REPETITIONS};
use windows_placement_probe::fingerprint::{Fingerprint, ProcessorPlace};
use windows_placement_probe::peer_index_cache::{CAPACITY, ITEMS};
use windows_topology_sys::Observed;

use crate::row::{Row, Value};

#[cfg(test)]
mod tests;

/// What the row names itself.
pub const REASON: &str = "x-probe-core-affinity";

/// The row a measured report carries.
///
/// **Raw data**, per `d-every-probe-emits-a-row` in DESIGN-NOTES.md:
///
/// - **The three readings of the host** that bracket the run -- `host_before`
///   (the banner's), `host` (the one the measurement took) and `host_after` --
///   each as its fingerprint string, or `null` where that reading failed.
///   Whether they agree is the reader's comparison; the prose makes it too.
/// - **Every processor as discovered**: group, number, core, efficiency class,
///   cache domain and NUMA node. A cache domain is a number when the topology
///   named one, `"absent"` when no level partitions the machine, and
///   `"not_observed"` when this processor was left out of the level that does
///   -- three facts the prose keeps apart, so the row does too.
/// - **Every timed pair**, in the order measured, in `placements`, `by_class`
///   and `node_pairs`: the placement and strategy by record name, the two
///   processors, the ring's requested and achieved NUMA nodes (`null` where
///   none was requested, or the achieved one is unknown), and the median of
///   [`REPETITIONS`] passes as whole elapsed nanoseconds with that pass's
///   consumer and producer shared-read counts.
///
/// Not the per-item costs, batch depths, spreads, confounding or verdicts the
/// prose derives; a consumer wanting a per-item figure divides by `items`, and
/// one wanting a batch depth divides `items` by a read count.
#[must_use]
pub fn raw_row(
    observation: &Observation,
    before: Option<&Fingerprint>,
    after: Option<&Fingerprint>,
    build: &str,
) -> Row {
    let processors: Value = observation.processors.iter().map(processor).collect();
    let pairs = |rows: &[Measurement]| -> Value { rows.iter().map(measurement).collect() };
    preamble(before, build)
        .with("declined", Value::Null)
        .with("host", observation.host.to_string())
        .with("host_after", after.map(ToString::to_string))
        .with("processors", processors)
        .with("placements", pairs(&observation.measurements))
        .with("by_class", pairs(&observation.by_class))
        .with("node_pairs", pairs(&observation.by_node_pair))
}

/// The row a report carries when the host could not be measured.
///
/// Follows topology's `report_unmeasured`: the same row, naming why, rather
/// than no row. `declined` carries the error's text -- a topology that could
/// not be read, or a processor that could not be pinned -- and is `null` in a
/// measured row. `host_before` is kept, since the banner's reading was taken
/// before the measurement failed; nothing the measurement would have produced
/// is present.
#[must_use]
pub fn declined_row(error: &std::io::Error, before: Option<&Fingerprint>, build: &str) -> Row {
    preamble(before, build).with("declined", error.to_string())
}

/// What both forms carry: where the run happened and what it was set to do.
fn preamble(before: Option<&Fingerprint>, build: &str) -> Row {
    Row::new(REASON)
        .with("arch", std::env::consts::ARCH)
        .with("build", build)
        .with("items", ITEMS)
        .with("capacity", CAPACITY)
        .with("repetitions", REPETITIONS)
        .with("host_before", before.map(ToString::to_string))
}

fn processor(place: &ProcessorPlace) -> Value {
    Value::Object(vec![
        ("group", Value::from(place.group)),
        ("number", Value::from(place.number)),
        ("core", Value::from(place.core)),
        ("efficiency_class", Value::from(place.efficiency_class)),
        (
            "cache_domain",
            match place.cache_domain {
                Observed::Known(domain) => Value::from(domain),
                Observed::Absent => Value::from("absent"),
                Observed::NotObserved => Value::from("not_observed"),
            },
        ),
        ("numa_node", Value::from(place.numa_node)),
    ])
}

fn measurement(row: &Measurement) -> Value {
    Value::Object(vec![
        ("placement", Value::from(row.placement.name())),
        ("strategy", Value::from(row.strategy.name())),
        ("producer_group", Value::from(row.producer.group)),
        ("producer_number", Value::from(row.producer.number)),
        ("consumer_group", Value::from(row.consumer.group)),
        ("consumer_number", Value::from(row.consumer.number)),
        (
            "requested_memory_node",
            Value::from(row.requested_memory_node),
        ),
        ("memory_node", Value::from(row.memory_node)),
        (
            "median_elapsed_ns",
            Value::from(row.elapsed_nanos.round() as usize),
        ),
        ("consumer_refreshes", Value::from(row.consumer_refreshes)),
        ("producer_refreshes", Value::from(row.producer_refreshes)),
    ])
}
