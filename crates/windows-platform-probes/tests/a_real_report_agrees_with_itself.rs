// Copyright (c) Mike Grier.

//! The report this crate renders from a *real* measurement, checked against the
//! oracle.
//!
//! # Why this is not a unit test
//!
//! The oracle's own tests pin it against fixtures. A fixture is a report
//! somebody wrote down, so a fixture-bound oracle checks correspondences over
//! states its author already imagined -- and the defect the oracle exists for
//! was a state nobody had imagined: `topology_report` printing `BUG IN THIS
//! PROBE ... Nothing below about cache partitioning can be trusted` while the
//! verdict two paragraphs below printed `=> agree`.
//!
//! More narrowly, a fixture cannot notice the **renderer** drifting away from
//! the prose labels the oracle looks for. Both sides would still agree with
//! each other; only the real artifact disagrees.
//!
//! Some unit tests in this crate do call `measure()` and so do read this host.
//! What none of them does is run the ORACLE over a report rendered from that
//! reading, which is the gap this file closes. On CI it runs across the hosted
//! runner fleet -- a slow survey of shapes no fixture anticipates.
//!
//! # It asserts nothing about this machine
//!
//! Deliberately. A test that expected a processor count, a cache level or a
//! verdict would fail on the next runner shape rather than on a defect, and
//! would have to be loosened until it asserted nothing. What it checks is that
//! whatever this host produced, the report's parts agree **with each other** --
//! a property every host must satisfy, including one whose topology cannot be
//! read at all.

use windows_placement_probe::fingerprint::Fingerprint;
use windows_platform_probes::report_oracle;
use windows_platform_probes::topology::measure;
use windows_platform_probes::topology_report::{attribution, report, report_unmeasured};

/// The report exactly as `probe-topology` composes it.
///
/// Composed here rather than by running the binary and reading its stdout,
/// because the oracle needs the report as a value; the binary is covered
/// separately by the stdout test. The sequence mirrors `src/bin/topology.rs`
/// deliberately -- a report assembled some other way would be checking an
/// artifact no one ships.
fn real_report() -> (String, bool) {
    let before = Fingerprint::discover();
    let measured = measure();
    let after = Fingerprint::discover();
    let banner = attribution(&before, &after);

    match measured {
        Ok(observation) => (report(&banner, &observation), true),
        Err(error) => (report_unmeasured(&banner, &error), false),
    }
}

#[test]
fn a_report_rendered_from_this_host_agrees_with_itself() {
    let (text, _) = real_report();

    // The whole assertion. Not "the report says X" -- "the report does not
    // contradict itself", which is checkable without knowing anything about
    // the machine.
    report_oracle::assert_corresponds(&text);
}

#[test]
fn the_oracle_is_actually_reading_this_host_s_report() {
    // Without this, the test above is worth nothing on a host whose report the
    // oracle cannot parse: every lookup returns `None`, every comparison is
    // skipped, and it passes having checked exactly zero correspondences.
    //
    // The unit tests pin the oracle against a fixture, which cannot notice the
    // renderer drifting away from it. Only the real artifact can, and only if
    // something requires a violation to appear.
    //
    // So a fact this host really rendered is corrupted, and the oracle must
    // report it. `"processors":0` is the corruption because no host has zero
    // online processors, so it disagrees with the prose on every machine
    // without needing to know what the prose says.
    let (text, measured) = real_report();

    if !measured {
        // `report_unmeasured` carries no counts to corrupt. A host whose
        // topology cannot be read is a legitimate outcome -- "cannot measure"
        // is a third answer in this crate -- and skipping is honest here in a
        // way it would not be for the assertion above, which still ran.
        eprintln!("topology could not be read on this host; corruption check skipped");
        return;
    }

    // Every double-rendered fact, not just one. Corrupting a single field would
    // leave the other pairs unguarded: the renderer could drift away from the
    // oracle's other prose labels and this would still pass on the strength of
    // the one.
    //
    // Zero is the corruption for all of them because no host has zero online
    // processors, zero processor groups, zero packages or zero physical cores.
    // So each disagrees with the prose on every machine, without this test
    // needing to know what the prose says.
    for key in [
        "\"processors\":",
        "\"groups\":",
        "\"packages\":",
        "\"cores\":",
        "\"numa_domains\":",
        "\"outermost_partitioning_cache_level\":",
        // Nested, and reached by their inner keys: `policies` is an object and
        // `caches` an array of objects, so these corrupt the first entry of
        // each rather than the container.
        "\"single\":",
        "\"domains\":",
    ] {
        let corrupted = corrupt_count(&text, key);
        assert_ne!(
            corrupted, text,
            "corrupting {key} changed nothing, so this proves nothing -- a \
             sabotage that fails to apply is indistinguishable from an \
             instrument that fails to fire.\n\n--- the report ---\n{text}"
        );

        let violations = report_oracle::check(&corrupted);
        assert!(
            !violations.is_empty(),
            "the oracle read no correspondence for {key} in a report this host \
             actually produced, so \
             `a_report_rendered_from_this_host_agrees_with_itself` is passing \
             without checking that fact. The renderer has probably drifted from \
             the prose label the oracle looks for.\n\n--- the report ---\n{text}"
        );
    }
}

/// Rewrite one NDJSON count to a value no host can have.
fn corrupt_count(report: &str, key: &str) -> String {
    report
        .lines()
        .map(|line| {
            if line.starts_with('{') {
                replace_json_number(line, key, "0")
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Replace the numeric value following `key` in a flat JSON line.
fn replace_json_number(line: &str, key: &str, value: &str) -> String {
    let Some(start) = line.find(key) else {
        return line.to_owned();
    };
    let after = start + key.len();
    let end = line[after..]
        .find([',', '}'])
        .map_or(line.len(), |offset| after + offset);

    format!("{}{key}{value}{}", &line[..start], &line[end..])
}
