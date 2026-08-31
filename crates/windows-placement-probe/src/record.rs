// Copyright (c) 2026 Mike Grier
//! The record a run produces and a runner sends back.
//!
//! # One record, one run
//!
//! Everything a reader needs to interpret the numbers travels with them: which
//! build measured, what the machine was, where the topology came from, and when.
//! A number that arrives without those cannot be compared against one taken on
//! another machine six months later, and comparing across machines is the entire
//! purpose of collecting these.
//!
//! # The record is meant to be read before it is sent
//!
//! It is text, its field names mean something without a schema in hand, and it
//! contains no opaque blobs. That is a deliberate constraint rather than a
//! convenience: "open it and read it, and if you are unhappy with anything in
//! there, do not send it" is only an honest instruction if the file can actually
//! be read.

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use windows_topology_sys::Provenance;

use crate::build_identity::BuildIdentity;
use crate::core_affinity::{Measurement, Observation};
use crate::fingerprint::Fingerprint;
use crate::machine::MachineDescription;

/// The version of the record's shape.
///
/// # How this is kept honest
///
/// A linearly increasing integer, so a collector can compare it (`schema >= 2`)
/// without understanding anything else. The hazard is not the counter, it is
/// **forgetting to raise it** when the shape changes -- which no amount of care
/// reliably prevents.
///
/// So the shape is *archived* rather than restated: `schema/vN.txt` lists this
/// record's key paths, and a test regenerates them and compares. Changing the
/// shape without bumping fails that test, and the diff shows exactly what
/// changed.
///
/// A digest was considered and rejected. With a table of `version -> hash` only
/// the current version's hash can ever be recomputed, so every earlier row is a
/// frozen constant nobody can verify, and the hash function silently becomes an
/// unversioned contract. A digest also reports only *that* the shape moved,
/// never *what* moved, so a review cannot tell an added field from a removed
/// one.
///
/// **The golden files are append-only and a published version is never
/// redefined.** Once a record exists in the wild claiming schema N, N's meaning
/// is fixed, because that record cannot be regenerated.
pub const SCHEMA_VERSION: u32 = 2;

/// One run's complete output.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct SubmissionRecord {
    /// Which shape this record has. See [`SCHEMA_VERSION`].
    pub schema_version: u32,
    /// When the run finished, as an ISO-8601 UTC timestamp.
    pub recorded_at: String,
    /// The same instant in seconds since the Unix epoch.
    ///
    /// Carried beside the formatted form because a collector should never have
    /// to parse prose to sort records, and because it survives any later change
    /// to how the string is rendered.
    pub recorded_at_epoch_seconds: u64,
    /// Which build measured.
    pub build: BuildIdentity,
    /// What the machine was, beyond its measurable shape.
    pub machine: MachineDescription,
    /// The machine's shape, in the terms that decide which placements exist.
    pub host: Fingerprint,
    /// Where the topology came from.
    ///
    /// Repeated from [`Fingerprint::provenance`] deliberately: a collector
    /// filtering out synthetic submissions should not have to know that the
    /// fingerprint carries it, and this is the field they will look for.
    pub topology_provenance: Provenance,
    /// One entry per placement this machine could express, per strategy.
    pub placements: Vec<MeasurementRecord>,
    /// One entry per distinct pair of NUMA nodes, per strategy.
    ///
    /// Empty on a single-node machine. **That emptiness is the finding this
    /// tool most wants from a large host**, so it is an empty list rather than
    /// an omitted field: a collector can then tell "measured, none exist" from
    /// "this version did not report them".
    pub node_hops: Vec<MeasurementRecord>,
    /// One entry per efficiency class, comparing like with like.
    pub by_class: Vec<MeasurementRecord>,
}

/// One measurement, flattened into the shape a record carries.
///
/// Flattened rather than nested because the nesting in
/// [`Measurement`](crate::core_affinity::Measurement) serves the code, and a
/// record is read by someone who does not have the code.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct MeasurementRecord {
    /// How the two threads were placed relative to each other.
    pub placement: String,
    /// Which peer-index strategy ran.
    pub strategy: String,
    /// Exactly which processors this number came from.
    pub slice: String,
    /// The producer's processor group.
    pub producer_group: u16,
    /// The producer's processor number within its group.
    pub producer_number: u8,
    /// The producer's NUMA node.
    pub producer_numa_node: u32,
    /// The consumer's processor group.
    pub consumer_group: u16,
    /// The consumer's processor number within its group.
    pub consumer_number: u8,
    /// The consumer's NUMA node.
    pub consumer_numa_node: u32,
    /// Median nanoseconds per item handed across the ring.
    pub nanos_per_item: f64,
    /// How many items each consumer-side shared read was amortised over.
    pub consumer_batch: f64,
    /// The same for the producer side.
    pub producer_batch: f64,
    /// Which NUMA node held the ring's slots.
    ///
    /// **The third position.** A hop measured with the data on an unknown node
    /// is not a measurement of that hop, so this is recorded rather than left
    /// to whichever node the orchestrating thread happened to occupy.
    ///
    /// `null` means two different things, told apart by which array the row is
    /// in. In `node_hops` a placement was always arranged, so `null` there means
    /// one was attempted and **could not be achieved** -- a caveat on that row.
    /// In `placements` and `by_class` none is ever arranged, so `null` is the
    /// normal case and means the ring was left wherever the allocator put it.
    pub memory_node: Option<u32>,
}

impl From<&Measurement> for MeasurementRecord {
    fn from(measurement: &Measurement) -> Self {
        Self {
            placement: measurement.placement.label().to_owned(),
            strategy: measurement.strategy.name().to_owned(),
            slice: measurement.slice.to_string(),
            producer_group: measurement.producer.group,
            producer_number: measurement.producer.number,
            producer_numa_node: measurement.producer.numa_node,
            consumer_group: measurement.consumer.group,
            consumer_number: measurement.consumer.number,
            consumer_numa_node: measurement.consumer.numa_node,
            nanos_per_item: measurement.nanos_per_item,
            consumer_batch: measurement.consumer_batch,
            producer_batch: measurement.producer_batch,
            memory_node: measurement.memory_node,
        }
    }
}

impl SubmissionRecord {
    /// Assemble a record from a completed run.
    #[must_use]
    pub fn new(observation: &Observation, host: Fingerprint, machine: MachineDescription) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());

        Self {
            schema_version: SCHEMA_VERSION,
            recorded_at: iso8601_utc(now),
            recorded_at_epoch_seconds: now,
            build: BuildIdentity::current(),
            machine,
            topology_provenance: host.provenance,
            host,
            placements: observation.measurements.iter().map(Into::into).collect(),
            node_hops: observation.by_node_pair.iter().map(Into::into).collect(),
            by_class: observation.by_class.iter().map(Into::into).collect(),
        }
    }

    /// Whether every part of this record is trustworthy.
    ///
    /// A record that fails this is still worth sending -- it is not worth
    /// silently pooling with the rest, because a defect found later can only be
    /// traced through a build and a topology that can name themselves.
    #[must_use]
    pub fn is_fully_trusted(&self) -> bool {
        self.build.is_official() && self.topology_provenance.is_measured()
    }
}

impl fmt::Display for SubmissionRecord {
    /// A one-line summary, for a banner rather than for a collector.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "schema {} | {} | {} | {} placements, {} node hops",
            self.schema_version,
            self.build,
            self.host,
            self.placements.len(),
            self.node_hops.len()
        )
    }
}

/// Format seconds since the Unix epoch as `YYYY-MM-DDTHH:MM:SSZ`.
///
/// Hand-rolled rather than pulled from a date crate: one timestamp in one
/// format does not justify a dependency in a tool people are asked to download
/// and run, and the civil-from-days conversion is a settled algorithm that a
/// test can pin against known instants.
#[must_use]
fn iso8601_utc(epoch_seconds: u64) -> String {
    let days = (epoch_seconds / 86_400) as i64;
    let seconds_of_day = epoch_seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    let (hour, minute, second) = (
        seconds_of_day / 3_600,
        (seconds_of_day % 3_600) / 60,
        seconds_of_day % 60,
    );
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Days since 1970-01-01 to a civil `(year, month, day)`.
///
/// Howard Hinnant's `civil_from_days`, which shifts the epoch to 0000-03-01 so
/// the leap day lands at the end of the era and the month arithmetic becomes
/// branch-free.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * shifted_month + 2) / 5 + 1) as u32;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    } as u32;
    (year + i64::from(month <= 2), month, day)
}

#[cfg(test)]
pub(crate) mod tests;
