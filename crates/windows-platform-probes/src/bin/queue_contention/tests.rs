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
use std::panic::AssertUnwindSafe;
use windows_platform_probes::queue_contention::{Observation, Run};

/// Asserts every line of a table shares its header's width.
///
/// **The one statement of the alignment rule.** The corpus check calls it, and
/// so does the anti-vacuity test at the bottom of this file, so that test
/// exercises the assertion the corpus actually relies on. A test that compared
/// the line lengths itself would be a second copy of the rule, and would keep
/// passing after this body was deleted.
#[track_caller]
fn assert_aligned(name: &str, lines: &[&str], why: &str) {
    let width = lines[0].len();
    for line in lines {
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

/// Every table whose header contains `header`, each with its header included.
///
/// A table runs from its header to the first blank line. The drained tables
/// carry a second header row, which is part of the table and has to line up
/// with the rest of it, so it is not skipped.
///
/// **All occurrences, not the first.** The report emits the claim-word layout
/// table twice under identical headers -- once isolated, once drained -- and
/// `ns/op range` heads both raw tables. Returning only the first meant the
/// corpus checked the isolated table and a width regression in the drained one
/// passed unseen, which is half the report unguarded.
fn tables<'a>(report: &'a str, header: &str) -> Vec<Vec<&'a str>> {
    let all: Vec<&str> = report.lines().collect();
    let mut found = Vec::new();
    let mut index = 0;
    while index < all.len() {
        if all[index].contains(header) && is_columnar(all[index]) {
            let table: Vec<&str> = all[index..]
                .iter()
                .take_while(|line| !line.trim().is_empty())
                .copied()
                .collect();
            // Step past this table so its own rows cannot match again.
            index += table.len().max(1);
            found.push(table);
        } else {
            index += 1;
        }
    }
    found
}

/// Whether a line is a table header rather than prose that mentions one.
///
/// Header names are matched as substrings, and the report's prose discusses the
/// columns it prints -- "The atomic floor is the cheapest possible contended
/// operation" contains `atomic floor` and is a sentence. Slicing from there
/// gathers a paragraph and compares the lengths of its lines, which fails for
/// the ordinary reason that prose is ragged.
///
/// A header is columnar: its fields are separated by gaps of multiple spaces, so
/// it splits into two or more parts. Prose is single-spaced and splits into one.
/// That one test tells them apart without the fixture having to enumerate
/// either.
fn is_columnar(line: &str) -> bool {
    line.trim()
        .split("  ")
        .filter(|part| !part.is_empty())
        .count()
        >= 2
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
            let found = tables(&report, header);
            assert!(
                !found.is_empty(),
                "[{name}] no table header containing {header:?} in:\n{why}\n{report}"
            );
            for (occurrence, lines) in found.iter().enumerate() {
                assert!(
                    lines.len() > 1,
                    "[{name}] the table at {header:?} (occurrence {}) has no rows, \
                     so its alignment is not being checked\n{why}",
                    occurrence + 1
                );
                assert_aligned(name, lines, why);
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
/// fail. This hands [`assert_aligned`] a deliberately overrun table and asserts
/// it panics -- the anti-vacuity half of a sabotage run, made in-suite because
/// the detector is the instrument here.
///
/// It calls the same function the corpus calls rather than re-deriving the rule
/// from the fixture's line lengths. A test that compared the lengths itself
/// would keep passing after [`assert_aligned`]'s body was deleted, which is
/// exactly the failure it exists to rule out.
///
/// The caught panic prints its message through the default hook, so a backtrace
/// line appears in this test's output on success. That is left alone: silencing
/// it means installing a process-global no-op panic hook, and this suite runs
/// its tests as threads in one process, so the window would swallow a concurrent
/// test's failure message.
#[test]
fn the_alignment_check_can_tell_a_misaligned_table_from_an_aligned_one() {
    let misaligned = tables(
        "producers      ratio\n1          1.00x [1.00-1.00]\n",
        "ratio",
    )
    .remove(0);
    assert_eq!(misaligned.len(), 2, "the fixture has a header and one row");
    let caught = std::panic::catch_unwind(AssertUnwindSafe(|| {
        assert_aligned("fixture", &misaligned, "a deliberately overrun table");
    }));
    assert!(
        caught.is_err(),
        "the detector passed a table whose row is {} wide against a {} header; \
         it cannot report a real overrun either",
        misaligned[1].len(),
        misaligned[0].len()
    );

    let aligned = tables("producers      ratio\n        1      1.00x\n", "ratio").remove(0);
    assert_aligned("fixture", &aligned, "an aligned table must not be reported");
}

/// The layout table is rendered for both regimes, and both are checked.
///
/// Returning every occurrence only helps if there are two to find. Were the
/// drained layout table to stop being rendered, every alignment assertion above
/// would still pass -- there would simply be one fewer table to check, which is
/// silence rather than failure. This pins the count so the disappearance is a
/// test failure instead of a quietly smaller suite.
#[test]
fn an_ordinary_observation_renders_the_layout_table_for_both_regimes() {
    let corpus: Value = serde_json::from_str(CORPUS).expect("the corpus parses");
    let case = corpus["cases"]
        .as_array()
        .expect("`cases` is an array")
        .iter()
        .find(|case| case["name"] == "ordinary")
        .expect("the corpus has an `ordinary` case");

    let mut report = String::new();
    render_observation(&mut report, &observation_from(&case["observation"]));

    assert_eq!(
        tables(&report, "16/48 vs").len(),
        2,
        "the claim-word layout table is rendered once isolated and once drained, \
         so a count other than two means a regime stopped being reported\n{report}"
    );
}
