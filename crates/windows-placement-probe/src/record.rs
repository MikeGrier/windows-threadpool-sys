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
///
/// # That rule starts at the first release, and this crate has not had one
///
/// The freeze exists to protect records held by other people. Until the tool is
/// released there are none, so a version bump would archive a shape that never
/// reached anyone and leave the first public release already carrying dead
/// numbers.
///
/// This crate reached version 4 during development that way, and was collapsed
/// back to version 1.
///
/// **After the first release the rule above applies without exception**: the
/// next shape change raises this to 2 and adds `schema/v2.txt`, and `v1.txt` is
/// never touched again. See this crate's `DESIGN-NOTES.md`, "The schema freezes
/// at the first release, not before".
pub const SCHEMA_VERSION: u32 = 1;

/// Seconds in a minute, which is the resolution a submitted record keeps.
const SECONDS_PER_MINUTE: u64 = 60;

/// One run's complete output.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct SubmissionRecord {
    /// Which shape this record has. See [`SCHEMA_VERSION`].
    pub schema_version: u32,
    /// When the run finished, as an ISO-8601 UTC timestamp **floored to the
    /// minute**.
    ///
    /// The seconds field is therefore always `00`, and that is deliberate
    /// rather than an artefact. A submitted record describes someone's own
    /// machine, and a second-precision timestamp links two submissions from one
    /// host to each other even after every identifying field is withheld.
    /// Nothing in the analysis needs finer: these measure a machine's shape,
    /// not an ordering of events.
    ///
    /// UTC with no local offset, because an offset narrows the submitter to a
    /// band of longitudes and buys nothing.
    pub recorded_at: String,
    /// The same minute in seconds since the Unix epoch, and so always a
    /// multiple of 60.
    ///
    /// Carried beside the formatted form because a collector should never have
    /// to parse prose to sort records, and because it survives any later change
    /// to how the string is rendered.
    pub recorded_at_epoch_seconds: u64,
    /// Milliseconds past that second, for naming a file and nothing else.
    ///
    /// **Deliberately not serialized, and deliberately not a second clock
    /// reading.** The backup file's name needs finer resolution than the record
    /// does: two runs in one second would otherwise want the same file. Taking
    /// a fresh timestamp when the name is built would solve that while creating
    /// a worse problem -- the name would state a different instant from the
    /// record inside it -- so this is the same reading as the two fields above,
    /// just the part of it they discard.
    ///
    /// `serde(skip)` keeps it out of the schema: precision a *file name* wants
    /// is not a change to what a *record* promises, and the archived shape
    /// stays as published.
    #[cfg_attr(feature = "serde", serde(skip))]
    pub recorded_at_subsecond_millis: u32,
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
    /// One entry per *directed* node pair, per ring placement, per strategy.
    ///
    /// **The ring placement is the dimension a collector is most likely to
    /// miss.** Each directed hop is measured twice, once with the ring asked
    /// for on the producer's node and once on the consumer's, and
    /// `requested_memory_node` on each row says which. Rows that agree on every
    /// other field are therefore not duplicates, and averaging them together
    /// would erase exactly the asymmetry -- remote write against remote read --
    /// that measuring both placements exists to expose.
    ///
    /// **Key on the request, not on `memory_node`.** That field records what
    /// the allocation actually got, and Windows may satisfy a request on
    /// another node, so both rows of a pair can carry the same achieved node
    /// while describing different placements. A row whose two nodes disagree
    /// did not measure the placement it names.
    ///
    /// Empty on a single-node machine. **That emptiness is the finding this
    /// tool most wants from a large host**, so it is an empty list rather than
    /// an omitted field: a collector can then tell "measured, none exist" from
    /// "this version did not report them".
    pub node_hops: Vec<MeasurementRecord>,
    /// One entry per efficiency class, per strategy, comparing like with like.
    ///
    /// Two rows per class. The pair is chosen once and measured under each
    /// strategy, so rows that differ only in `strategy` are the comparison
    /// rather than duplicates.
    pub by_class: Vec<MeasurementRecord>,
}

/// One measurement, flattened into the shape a record carries.
///
/// Flattened rather than nested because the nesting in
/// [`Measurement`] serves the code, and a
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
    /// The producer's efficiency class.
    ///
    /// **Without this a `by_class` row cannot be attributed to the class it
    /// measures.** That list holds one same-class pair per class, so two rows
    /// agreeing on `placement` and `strategy` are the comparison rather than
    /// duplicates -- and a reader with no class field has no way to tell which
    /// row describes the fast cores. Carried on every row, not only those, so
    /// a heterogeneous pair in `placements` or `node_hops` is legible too:
    /// Windows will schedule across classes, and a hop between a performance
    /// core and an efficiency core is a different measurement from one between
    /// two peers.
    pub producer_efficiency_class: u8,
    /// The consumer's processor group.
    pub consumer_group: u16,
    /// The consumer's processor number within its group.
    pub consumer_number: u8,
    /// The consumer's NUMA node.
    pub consumer_numa_node: u32,
    /// The consumer's efficiency class. See
    /// [`Self::producer_efficiency_class`]; in a `by_class` row the two are
    /// equal by construction, and their being equal is what makes the row a
    /// like-for-like comparison.
    pub consumer_efficiency_class: u8,
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
    /// Which NUMA node the run *asked* for, independent of what it got.
    ///
    /// **This, not `memory_node`, is what tells two `node_hops` rows apart.**
    /// Each directed hop is measured once per ring placement, so its two rows
    /// agree on every other field -- and Windows may satisfy an allocation on
    /// a node other than the one requested. Keyed on the achieved node alone,
    /// the producer-local and consumer-local rows can serialise identically,
    /// and a collector would reasonably read them as duplicates.
    ///
    /// A row whose requested and observed nodes disagree **did not measure the
    /// placement it names**, and is a caveat rather than a result for that hop.
    ///
    /// `null` means nothing was requested, which is the normal case in
    /// `placements` and `by_class`.
    pub requested_memory_node: Option<u32>,
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
            producer_efficiency_class: measurement.producer.efficiency_class,
            consumer_group: measurement.consumer.group,
            consumer_number: measurement.consumer.number,
            consumer_numa_node: measurement.consumer.numa_node,
            consumer_efficiency_class: measurement.consumer.efficiency_class,
            nanos_per_item: measurement.nanos_per_item,
            consumer_batch: measurement.consumer_batch,
            producer_batch: measurement.producer_batch,
            memory_node: measurement.memory_node,
            requested_memory_node: measurement.requested_memory_node,
        }
    }
}

impl SubmissionRecord {
    /// Assemble a record from a completed run.
    ///
    /// `host` is the shape the runner was *shown* before consenting. It must
    /// equal [`Observation::host`], the shape the measurement actually ran on.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::InvalidData`](std::io::ErrorKind::InvalidData) if those two
    /// disagree, which means the machine changed between the announcement and
    /// the end of the measurement.
    ///
    /// # Why this is a refusal here rather than a check at the call site
    ///
    /// A record built from mismatched halves is a *splice*: its `host` describes
    /// one machine while every row was measured on another, and nothing in the
    /// file says so. A reader interpreting row sets through that host -- which is
    /// what the host is for -- interprets them through the wrong machine.
    ///
    /// The tool does check before calling, and reports the disagreement far
    /// better than an error type can. But a check at one call site is only as
    /// durable as the next call site's author remembering it, and this is the
    /// constructor every such author will reach for. Refusing here makes the
    /// splice unrepresentable rather than merely currently-avoided.
    pub fn new(
        observation: &Observation,
        host: Fingerprint,
        machine: MachineDescription,
    ) -> std::io::Result<Self> {
        if host != observation.host {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "the announced host and the measured host differ, so this record would \
                     describe two machines: announced {host}, measured {}",
                    observation.host
                ),
            ));
        }

        // One reading, split into the parts each consumer needs, so the record
        // and the file named after it can never describe different instants.
        let since_epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let now = since_epoch.as_secs();
        // **Floored to the minute, and the record carries only the floored
        // value.** A submitted record is a thing someone hands over about their
        // own machine, and a second-precision timestamp is close to a serial
        // number: it links two submissions from one host to each other even
        // when every other identifying field has been withheld. Nothing in the
        // analysis needs finer than a minute -- these are measurements of a
        // machine's shape, not of an event ordering.
        //
        // UTC, and no local offset anywhere, because an offset narrows the
        // submitter to a band of longitudes for no analytical gain.
        //
        // `recorded_at_subsecond_millis` is unaffected: it is `serde(skip)` and
        // exists only so two runs in one second get distinct *file names*. It
        // never reaches the record.
        let recorded_minute = now - (now % SECONDS_PER_MINUTE);

        Ok(Self {
            schema_version: SCHEMA_VERSION,
            recorded_at: iso8601_utc(recorded_minute),
            recorded_at_epoch_seconds: recorded_minute,
            recorded_at_subsecond_millis: since_epoch.subsec_millis(),
            build: BuildIdentity::current(),
            machine,
            topology_provenance: host.provenance,
            host,
            placements: observation.measurements.iter().map(Into::into).collect(),
            node_hops: observation.by_node_pair.iter().map(Into::into).collect(),
            by_class: observation.by_class.iter().map(Into::into).collect(),
        })
    }

    /// Whether every part of this record is trustworthy.
    ///
    /// A record that fails this is still worth sending -- it is not worth
    /// silently pooling with the rest, because a defect found later can only be
    /// traced through a build and a topology that can name themselves.
    ///
    /// # Both copies of the provenance are consulted, not just one
    ///
    /// `topology_provenance` deliberately duplicates the fingerprint's own
    /// provenance so a collector reading the record's top level need not reach
    /// into `host`. Both fields are public, so the duplication can be broken --
    /// by hand-assembling a record, or by editing one field of a deserialized
    /// one -- and consulting only the copy let a record whose fingerprint
    /// renders `!!SYNTHETIC!!` report itself fully trusted. The printed report
    /// would then contradict the very string beside it.
    ///
    /// Requiring both is the conservative reading: a record that disagrees with
    /// itself about where its topology came from is exactly the record not to
    /// pool, whichever field happens to be right.
    #[must_use]
    pub fn is_fully_trusted(&self) -> bool {
        self.build.is_official()
            && self.topology_provenance.is_measured()
            && self.host.provenance.is_measured()
    }
}

impl fmt::Display for SubmissionRecord {
    /// A one-line summary, for a banner rather than for a collector.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "schema {} | {} | {} | {} placement rows, {} node-hop rows",
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
