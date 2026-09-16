// Copyright (c) Mike Grier.

//! The report renderer, driven by a corpus rather than by hand-written cases.
//!
//! Adding a case is adding data to `corpus.json` -- an observation, and what the
//! rendered report must be true of. Nothing here needs to change, which is the
//! point: the report's tables were reachable by nothing in the suite until
//! `M4.7`, because rendering measured inside itself and the only way to run it
//! was a ~65-second host-dependent pass.
//!
//! **The central check is derived, not restated.** `aligned_tables` asserts that
//! every line of a named table is the same length. That is exactly the property
//! a cell wider than its column breaks, and it holds for inputs the corpus never
//! anticipated -- where a golden would only catch what somebody thought to
//! record, and would have to be regenerated every time the prose moved.

use super::render_observation;
use serde_json::Value;
use windows_platform_probes::queue_contention::{Observation, Run};

/// Compiled in, so a missing or malformed corpus is a build failure rather than
/// a test that silently runs nothing.
const CORPUS: &str = include_str!("corpus.json");

/// A row is `[shape, producers, nanos_per_op, ops_per_second, refusals,
/// fastest, slowest]`, positionally.
///
/// The struct literal is exhaustive, so adding a field to [`Run`] stops this
/// file compiling -- which is the reminder to decide what the corpus should say
/// about it, rather than letting a new field go unexercised.
fn run_from(value: &Value) -> Run {
    let row = value.as_array().expect("a run is an array");
    let number = |index: usize| -> f64 {
        row[index]
            .as_f64()
            .unwrap_or_else(|| panic!("field {index} of a run is a number"))
    };
    Run {
        // Leaked so the fixture can hand back the `&'static str` the field
        // wants. A test process is the one place that is the cheap answer, and
        // the corpus is a fixed compiled-in set, so this cannot grow.
        shape: Box::leak(
            row[0]
                .as_str()
                .expect("a shape is a string")
                .to_owned()
                .into_boxed_str(),
        ),
        producers: number(1) as usize,
        nanos_per_op: number(2),
        ops_per_second: number(3),
        refusals: number(4) as u64,
        fastest_nanos_per_op: number(5),
        slowest_nanos_per_op: number(6),
    }
}

fn observation_from(value: &Value) -> Observation {
    let rows = |key: &str| -> Vec<Run> {
        value[key]
            .as_array()
            .unwrap_or_else(|| panic!("`{key}` is an array"))
            .iter()
            .map(run_from)
            .collect()
    };
    Observation {
        isolated: rows("isolated"),
        drained: rows("drained"),
        available_parallelism: value["available_parallelism"]
            .as_u64()
            .map(|count| count as usize),
    }
}

/// The lines of the table whose header contains `header`, header included.
///
/// A table runs from its header to the first blank line. The drained tables
/// carry a second header row, which is part of the table and has to line up
/// with the rest of it, so it is not skipped.
fn table_lines<'a>(report: &'a str, header: &str) -> Vec<&'a str> {
    let all: Vec<&str> = report.lines().collect();
    let start = all
        .iter()
        .position(|line| line.contains(header))
        .unwrap_or_else(|| panic!("no table header containing {header:?} in:\n{report}"));
    all[start..]
        .iter()
        .take_while(|line| !line.trim().is_empty())
        .copied()
        .collect()
}

#[test]
fn every_corpus_case_renders_a_report_whose_tables_line_up() {
    let corpus: Value = serde_json::from_str(CORPUS).expect("the corpus parses");
    let cases = corpus["cases"].as_array().expect("`cases` is an array");
    assert!(
        !cases.is_empty(),
        "an empty corpus would pass every assertion below without testing anything"
    );

    for case in cases {
        let name = case["name"].as_str().expect("a case is named");
        let why = case["why"].as_str().expect("a case says why it exists");
        let observation = observation_from(&case["observation"]);

        let mut report = String::new();
        render_observation(&mut report, &observation);

        let expect = &case["expect"];
        for header in expect["aligned_tables"]
            .as_array()
            .expect("`aligned_tables` is an array")
        {
            let header = header.as_str().expect("a header is a string");
            let lines = table_lines(&report, header);
            assert!(
                lines.len() > 1,
                "[{name}] the table at {header:?} has no rows, so its alignment \
                 is not being checked\n{why}"
            );
            let width = lines[0].len();
            for line in &lines {
                assert_eq!(
                    line.len(),
                    width,
                    "[{name}] a cell overran its column, so every column after it \
                     no longer lines up with its header.\n{why}\n\
                     header ({width}): {:?}\n  line ({}): {line:?}",
                    lines[0],
                    line.len()
                );
            }
        }

        for needle in expect["contains"]
            .as_array()
            .expect("`contains` is an array")
        {
            let needle = needle.as_str().expect("a needle is a string");
            assert!(
                report.contains(needle),
                "[{name}] expected {needle:?} in the report.\n{why}\n{report}"
            );
        }

        for needle in expect["absent"].as_array().expect("`absent` is an array") {
            let needle = needle.as_str().expect("a needle is a string");
            assert!(
                !report.contains(needle),
                "[{name}] {needle:?} must not reach the report.\n{why}\n{report}"
            );
        }
    }
}

/// The alignment check must be able to fail, or the corpus proves nothing.
///
/// Every case above passes, which is indistinguishable from a check that cannot
/// fail. This hands the detector a deliberately overrun table and asserts it
/// notices -- the anti-vacuity half of a sabotage run, made in-suite because the
/// detector is the instrument here.
#[test]
fn the_alignment_check_can_tell_a_misaligned_table_from_an_aligned_one() {
    let misaligned = "producers      ratio\n1          1.00x [1.00-1.00]\n";
    let lines = table_lines(misaligned, "ratio");
    assert_eq!(lines.len(), 2, "the fixture has a header and one row");
    assert_ne!(
        lines[0].len(),
        lines[1].len(),
        "this fixture exists to be misaligned; if it is not, the corpus check is \
         being asked to detect something that is not there"
    );

    let aligned = "producers      ratio\n        1      1.00x\n";
    let lines = table_lines(aligned, "ratio");
    assert_eq!(
        lines[0].len(),
        lines[1].len(),
        "and it must not call an aligned table misaligned"
    );
}
