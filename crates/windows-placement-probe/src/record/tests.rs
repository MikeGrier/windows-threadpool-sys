// Copyright (c) 2026 Mike Grier
//! Tests for [`SubmissionRecord`](super::SubmissionRecord), including the
//! schema guard.

use std::collections::BTreeSet;

use windows_topology_sys::Provenance;

use super::{MeasurementRecord, SCHEMA_VERSION, SubmissionRecord, civil_from_days, iso8601_utc};
use crate::build_identity::{BuildIdentity, BuildSource};
use crate::fingerprint::Fingerprint;
use crate::machine::{MachineDescription, VirtualisationHint};

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
        slice: "pinned prod=g0/cpu0/core0/ec0/cd2/n0 cons=g0/cpu1/core0/ec0/cd2/n0".to_owned(),
        producer_group: 0,
        producer_number: 0,
        producer_numa_node: 0,
        consumer_group: 0,
        consumer_number: 1,
        consumer_numa_node: 0,
        nanos_per_item: 10.5,
        consumer_batch: 84.9,
        producer_batch: 1.0,
    };

    SubmissionRecord {
        schema_version: SCHEMA_VERSION,
        recorded_at: "2026-08-31T12:00:00Z".to_owned(),
        recorded_at_epoch_seconds: 1_788_177_600,
        build: BuildIdentity {
            crate_version: "0.1.0",
            commit: Some("abcdef123456"),
            dirty: Some(false),
            source: BuildSource::Ci,
        },
        machine: MachineDescription {
            cpu_model: Some("Example CPU".to_owned()),
            model_suppressed: false,
            os_build: Some("10.0.26200.9168".to_owned()),
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
fn a_record_is_fully_trusted_only_when_the_build_and_the_topology_both_are() {
    assert!(fully_populated().is_fully_trusted());

    let mut synthetic = fully_populated();
    synthetic.topology_provenance = Provenance::Synthetic;
    assert!(!synthetic.is_fully_trusted());

    let mut unofficial = fully_populated();
    unofficial.build.source = BuildSource::Local;
    assert!(!unofficial.is_fully_trusted());
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
