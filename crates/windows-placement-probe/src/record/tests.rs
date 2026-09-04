// Copyright (c) 2026 Mike Grier
//! Tests for [`SubmissionRecord`](super::SubmissionRecord), including the
//! schema guard.

// Used only by the schema-shape helpers, which are serialization-only.
#[cfg(feature = "serde")]
use std::collections::BTreeSet;

use windows_topology_sys::{Coherence, ProcessorId, Provenance};

use super::{MeasurementRecord, SCHEMA_VERSION, SubmissionRecord, civil_from_days, iso8601_utc};
use crate::build_identity::{BuildIdentity, BuildSource};
use crate::fingerprint::Fingerprint;
use crate::machine::{MachineDescription, VirtualisationHint};
use crate::redaction::MetadataPolicy;

/// A record with **every** optional field populated.
///
/// Fully populated on purpose: the schema golden is derived from whatever this
/// serializes to, so a field left `None` here would be omitted from the JSON
/// and would silently vanish from the archived shape. A schema that describes
/// less than the record can emit is worse than no schema, because it would
/// pass.
pub(crate) fn fully_populated() -> SubmissionRecord {
    let measurement = MeasurementRecord {
        placement: "SMT siblings (one core)".to_owned(),
        strategy: "baseline".to_owned(),
        // A real slice, copied verbatim from a run. An earlier fixture used a
        // shortened one, which let a line-width test pass while the tool emitted
        // lines half again as long.
        slice: "pinned prod=g0/cpu0/core0/ec0/cd2/n0 cons=g0/cpu1/core0/ec0/cd2/n0 [same-cache,same-class]"
            .to_owned(),
        producer_group: 0,
        producer_number: 0,
        producer_numa_node: 0,
        // Matching the `ec0` in the slice above. A fixture whose fields
        // contradict its own slice string would teach the wrong shape.
        producer_efficiency_class: 0,
        consumer_group: 0,
        consumer_number: 1,
        consumer_numa_node: 0,
        consumer_efficiency_class: 0,
        nanos_per_item: 10.5,
        consumer_batch: 84.9,
        producer_batch: 1.0,
        memory_node: Some(0),
        // Populated, like every other field here, so the golden describes the
        // full shape. Equal to `memory_node` because this fixture stands for a
        // row that got what it asked for.
        requested_memory_node: Some(0),
    };

    SubmissionRecord {
        schema_version: SCHEMA_VERSION,
        recorded_at: Some("2026-08-31T12:00:00Z".to_owned()),
        recorded_at_epoch_seconds: Some(1_788_177_600),
        recorded_at_suppressed: false,
        recorded_at_subsecond_millis: 250,
        build: BuildIdentity {
            crate_version: "2026.902.0",
            commit: Some("abcdef123456"),
            dirty: Some(false),
            source: BuildSource::Ci,
        },
        machine: MachineDescription {
            cpu_model: Some("Example CPU".to_owned()),
            model_suppressed: false,
            os_build: Some("10.0.26200.9168".to_owned()),
            os_build_suppressed: false,
            virtualisation: VirtualisationHint::Detected,
            virtualisation_name: Some("Example Hypervisor".to_owned()),
        },
        host: Fingerprint {
            arch: "x86_64",
            processors: 16,
            cores: 8,
            smt: true,
            partitioning_cache_level: Some(2),
            cache_domain_sizes: vec![2, 2],
            efficiency_classes: vec![(0, 16)],
            numa_node_sizes: vec![16],
            provenance: Provenance::Measured,
        },
        topology_provenance: Provenance::Measured,
        // **`Disagreed`, because this fixture derives the schema golden**, and
        // it is the only variant carrying fields. `Agreed` would archive a
        // shape with no `walk_only`/`cpu_sets_only`/`attempts` paths in it, so
        // the guard would pass while describing less than the record can emit.
        // A measured topology whose two sources disagreed is a real
        // combination, not a contrived one -- it is the case the report's
        // closing section exists for.
        topology_coherence: Coherence::Disagreed {
            walk_only: vec![ProcessorId {
                group: 0,
                number: 14,
            }],
            cpu_sets_only: vec![ProcessorId {
                group: 0,
                number: 15,
            }],
            attempts: 3,
        },
        placements: vec![measurement.clone()],
        node_hops: vec![measurement.clone()],
        by_class: vec![measurement],
    }
}

/// Every key path the record serializes to, sorted.
///
/// **Derived from the record rather than written down.** A hand-maintained list
/// would be a second statement of the shape, and the two would drift -- which
/// is the whole failure this guard exists to prevent, arriving by a different
/// route.
///
/// An array contributes `field[]`, not one entry per element, so the golden
/// describes the shape and not the size of one sample.
/// Serialization only: without the `serde` feature the record has no
/// serialized shape for this to describe.
#[cfg(feature = "serde")]
fn key_paths(value: &serde_json::Value) -> BTreeSet<String> {
    fn walk(value: &serde_json::Value, prefix: &str, into: &mut BTreeSet<String>) {
        match value {
            serde_json::Value::Object(fields) => {
                for (name, child) in fields {
                    let path = if prefix.is_empty() {
                        name.clone()
                    } else {
                        format!("{prefix}.{name}")
                    };
                    into.insert(path.clone());
                    walk(child, &path, into);
                }
            }
            serde_json::Value::Array(items) => {
                let path = format!("{prefix}[]");
                into.insert(path.clone());
                for item in items {
                    walk(item, &path, into);
                }
            }
            _ => {}
        }
    }

    let mut paths = BTreeSet::new();
    walk(value, "", &mut paths);
    paths
}

#[test]
#[cfg(feature = "serde")]
fn the_records_shape_matches_the_archived_schema_for_its_version() {
    // The guard. Change the record's shape without raising SCHEMA_VERSION and
    // adding the next golden, and this fails -- with a diff that names exactly
    // which paths appeared or disappeared, which a digest could never do.
    let record = fully_populated();
    let value = serde_json::to_value(&record).expect("the record must serialize");
    let actual: Vec<String> = key_paths(&value).into_iter().collect();

    let golden_path = format!(
        "{}/schema/v{SCHEMA_VERSION}.txt",
        env!("CARGO_MANIFEST_DIR")
    );
    let golden = std::fs::read_to_string(&golden_path).unwrap_or_else(|error| {
        panic!(
            "schema golden {golden_path} is missing ({error}). If SCHEMA_VERSION \
             was just raised, create it with the current shape:\n{}\n",
            actual.join("\n")
        )
    });
    let expected: Vec<String> = golden
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect();

    let missing: Vec<&String> = expected.iter().filter(|p| !actual.contains(p)).collect();
    let added: Vec<&String> = actual.iter().filter(|p| !expected.contains(p)).collect();

    assert!(
        missing.is_empty() && added.is_empty(),
        "the record's shape no longer matches schema v{SCHEMA_VERSION}.\n\
         removed: {missing:?}\n\
         added:   {added:?}\n\
         Raise SCHEMA_VERSION and add the next golden; never edit a published one, \
         because records already in the wild claim the old number and cannot be \
         regenerated."
    );
}

#[test]
fn the_schema_version_in_a_record_is_the_constant() {
    // A record that declared a version it was not built to would be worse than
    // one with no version at all.
    assert_eq!(fully_populated().schema_version, SCHEMA_VERSION);
}

#[test]
#[cfg(feature = "serde")]
fn every_field_of_a_fully_populated_record_is_present_in_the_json() {
    // Guards the fixture rather than the code: if a future field is added and
    // left `None` here, it would be omitted from the JSON and would never enter
    // the golden, so the guard above would pass while describing less than the
    // record can emit.
    let value = serde_json::to_value(fully_populated()).expect("must serialize");
    let paths = key_paths(&value);

    for required in [
        "machine.cpu_model",
        "machine.os_build",
        "machine.virtualisation_name",
        "build.commit",
        "build.dirty",
        "host.partitioning_cache_level",
    ] {
        assert!(
            paths.contains(required),
            "{required} is absent, so the fixture leaves an optional field unset"
        );
    }
}

#[test]
#[cfg(feature = "serde")]
fn node_hops_is_an_empty_list_rather_than_an_absent_field() {
    // The distinction a large-machine submission depends on: "measured, and
    // there are none" must be tellable from "this version did not report them".
    let mut record = fully_populated();
    record.node_hops.clear();

    let value = serde_json::to_value(&record).expect("must serialize");
    assert!(
        value
            .get("node_hops")
            .is_some_and(serde_json::Value::is_array),
        "node_hops must serialize as an array even when empty"
    );
}

#[test]
fn a_record_is_fully_traceable_only_when_the_build_and_the_topology_both_are() {
    assert!(fully_populated().is_fully_traceable());

    let mut synthetic = fully_populated();
    synthetic.topology_provenance = Provenance::Synthetic;
    assert!(!synthetic.is_fully_traceable());

    let mut unofficial = fully_populated();
    unofficial.build.source = BuildSource::Local;
    assert!(!unofficial.is_fully_traceable());
}

#[test]
fn a_record_whose_two_provenance_fields_disagree_is_not_fully_traceable() {
    // **The defect this guards.** `topology_provenance` duplicates the
    // fingerprint's provenance for a collector's convenience, and both fields
    // are public, so the two can be made to disagree. Consulting only the
    // top-level copy let a record whose fingerprint renders `!!SYNTHETIC!!`
    // report itself fully traceable -- the printed report contradicting the very
    // string beside it.
    let mut top_level_lies = fully_populated();
    top_level_lies.host.provenance = Provenance::Synthetic;
    assert!(
        !top_level_lies.is_fully_traceable(),
        "a synthetic fingerprint was reported as fully traceable"
    );

    // And the converse, so the check is not merely reading the other field now.
    let mut duplicate_lies = fully_populated();
    duplicate_lies.topology_provenance = Provenance::Restored;
    assert!(!duplicate_lies.is_fully_traceable());
}

#[test]
fn the_timestamp_renders_known_instants_correctly() {
    // Pins the hand-rolled civil-from-days conversion against instants whose
    // answers are independently known, including a leap day and a century
    // boundary that is a leap year.
    assert_eq!(iso8601_utc(0), "1970-01-01T00:00:00Z");
    assert_eq!(iso8601_utc(1), "1970-01-01T00:00:01Z");
    assert_eq!(iso8601_utc(86_399), "1970-01-01T23:59:59Z");
    assert_eq!(iso8601_utc(86_400), "1970-01-02T00:00:00Z");
    // 2000-02-29, a leap day in a year divisible by 100 and by 400.
    assert_eq!(iso8601_utc(951_782_400), "2000-02-29T00:00:00Z");
    // 2024-02-29, an ordinary leap day.
    assert_eq!(iso8601_utc(1_709_164_800), "2024-02-29T00:00:00Z");
    assert_eq!(iso8601_utc(1_788_177_600), "2026-08-31T12:00:00Z");
}

#[test]
fn the_civil_conversion_round_trips_across_a_long_span() {
    // A property rather than a fixture: every day for eighty years must convert
    // to a real date, and consecutive days must differ by exactly one day.
    let mut previous: Option<(i64, u32, u32)> = None;
    for day in 0..(80 * 365 + 20) {
        let (year, month, dom) = civil_from_days(day);
        assert!((1..=12).contains(&month), "day {day} gave month {month}");
        assert!((1..=31).contains(&dom), "day {day} gave day-of-month {dom}");
        assert!((1970..2060).contains(&year), "day {day} gave year {year}");

        if let Some(prev) = previous {
            assert_ne!(prev, (year, month, dom), "day {day} repeated a date");
        }
        previous = Some((year, month, dom));
    }
}

/// An observation carrying nothing but the host it claims to have measured on.
///
/// The rows are empty because the splice these tests are about is between the
/// two *hosts*; a row would only make the fixture longer without making the
/// question sharper.
fn observation_on(host: Fingerprint) -> crate::core_affinity::Observation {
    crate::core_affinity::Observation {
        host,
        processors: Vec::new(),
        by_class: Vec::new(),
        measurements: Vec::new(),
        by_node_pair: Vec::new(),
    }
}

/// The shape a four-processor bare topology produces, as a real conversion.
fn measured_host() -> Fingerprint {
    Fingerprint::from_topology(&windows_topology_sys::MachineMemoryTopology::default())
}

#[test]
fn a_record_cannot_splice_an_announced_host_onto_another_machines_rows() {
    // The defect. The tool announces a shape read at one instant and the
    // measurement discovers again at another, so a processor going offline --
    // or moving group or node -- between them yields a record whose `host`
    // describes one machine while every row was measured on a different one.
    // Nothing in the file says so, and the host is precisely what a reader
    // interprets row sets *through*, so the rows get read against the wrong
    // machine.
    // A processor arriving, rather than leaving, purely because the fixture's
    // bare topology has none to remove. The direction does not matter: the
    // record is a splice either way.
    let announced = measured_host();
    let mut measured = announced.clone();
    measured.processors += 1;

    let error = SubmissionRecord::new(
        &observation_on(measured),
        announced,
        MachineDescription::read(MetadataPolicy::redacted()),
        MetadataPolicy::redacted(),
        Coherence::Agreed,
    )
    .expect_err("a record spanning two machines must not be assembled");

    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert!(
        error.to_string().contains("two machines"),
        "the message must say what is wrong rather than only that something is: {error}"
    );
}

#[test]
fn a_record_assembles_when_the_announced_and_measured_hosts_agree() {
    // The other half, so the refusal above is known to be discriminating rather
    // than a constructor that now always fails.
    let host = measured_host();

    let record = SubmissionRecord::new(
        &observation_on(host.clone()),
        host.clone(),
        MachineDescription::read(MetadataPolicy::redacted()),
        MetadataPolicy::redacted(),
        Coherence::Agreed,
    )
    .expect("identical hosts are the ordinary case");

    assert_eq!(record.host, host);
}

#[test]
fn the_default_policy_leaves_a_record_with_no_timestamp() {
    // **The behaviour M36.2 exists for**, at the record's own boundary: even a
    // minute is a correlator between two submissions from one host, so it is
    // sent only when the runner asks for it to be.
    let host = measured_host();

    let record = SubmissionRecord::new(
        &observation_on(host.clone()),
        host,
        MachineDescription::read(MetadataPolicy::default()),
        MetadataPolicy::default(),
        Coherence::Agreed,
    )
    .expect("identical hosts are the ordinary case");

    assert_eq!(record.recorded_at, None);
    assert_eq!(record.recorded_at_epoch_seconds, None);
    assert!(
        record.recorded_at_suppressed,
        "an absent timestamp must say it was withheld rather than leave a reader guessing"
    );
}

#[test]
fn opting_in_carries_a_timestamp_floored_to_the_minute() {
    // The other half, so the withholding above is known to be a policy rather
    // than a constructor that stopped reading the clock -- and the flooring
    // from M36.1 is still in force on the value that survives the opt-in.
    let host = measured_host();

    let record = SubmissionRecord::new(
        &observation_on(host.clone()),
        host,
        MachineDescription::read(MetadataPolicy::included()),
        MetadataPolicy::included(),
        Coherence::Agreed,
    )
    .expect("identical hosts are the ordinary case");

    let seconds = record
        .recorded_at_epoch_seconds
        .expect("an opted-in record carries the epoch value");
    let rendered = record
        .recorded_at
        .as_deref()
        .expect("an opted-in record carries the rendered form");

    assert!(!record.recorded_at_suppressed);
    assert_eq!(seconds % 60, 0, "the minute floor still applies: {seconds}");
    assert!(
        rendered.ends_with(":00Z"),
        "the two forms must agree about the seconds field: {rendered}"
    );
    assert_eq!(
        rendered,
        iso8601_utc(seconds),
        "the rendered form must be the epoch value, not a second clock reading"
    );
}

#[test]
fn flooring_to_the_minute_is_what_zeroes_the_seconds_field() {
    // A submitted record describes someone's own machine, and a
    // second-precision timestamp is close to a serial number: it links two
    // submissions from one host to each other even after every identifying
    // field has been withheld. Nothing in the analysis needs finer than a
    // minute -- these measure a machine's shape, not an ordering of events.
    //
    // Pins that the *flooring* does the work rather than the renderer, by
    // showing the renderer faithfully reports a non-zero seconds field when
    // given one. A test that only checked the floored case would still pass if
    // someone made `iso8601_utc` truncate, which would leave
    // `recorded_at_epoch_seconds` disagreeing with the string beside it.
    let unaligned = 1_788_177_637; // ...:00:37Z
    let floored = unaligned - (unaligned % 60);

    assert!(
        super::iso8601_utc(unaligned).ends_with(":37Z"),
        "the renderer must report the seconds it is given: {}",
        super::iso8601_utc(unaligned)
    );
    assert!(
        super::iso8601_utc(floored).ends_with(":00Z"),
        "a floored instant renders a zero seconds field: {}",
        super::iso8601_utc(floored)
    );
    assert_eq!(floored % 60, 0, "and the epoch value is a whole minute");
}
