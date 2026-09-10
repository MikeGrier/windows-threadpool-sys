// Copyright (c) Mike Grier.

//! Every figure a cost probe prints reaches its machine-readable line too, with
//! the same value.
//!
//! # Why this is not an oracle rule
//!
//! The M2.4 matrix found both cost probes rendering every measured figure twice
//! -- once in a prose table, once in NDJSON -- with nothing comparing the two.
//! The obvious fix was a third oracle rule set. The chosen fix was to make the
//! two renderings **derive from one source**: both walk `Observation::timings`,
//! and `json_key` decides only what the machine-readable one calls each entry.
//!
//! That makes a disagreement unrepresentable rather than detectable, which is
//! strictly stronger. A rule finds a contradiction that already exists; there is
//! now none to find.
//!
//! # So what is left to test
//!
//! That the derivation actually holds end to end, on the real binaries: a figure
//! in the table appears in the JSON, under the name `json_key` gives it, with
//! the same digits. The property is structural in the source, and this is the
//! check that the structure survives rendering, formatting and the process
//! boundary.
//!
//! It reuses the crate's own `json_key` rather than restating the mapping. A
//! test carrying its own copy of the pairing would be checking the copy, which
//! is the defect this whole milestone is about.

use std::process::Command;

/// Every `label value` row of a probe's table, and its NDJSON line.
fn run(binary: &str) -> (Vec<(String, f64)>, String) {
    let output = Command::new(binary).output().expect("run the probe");
    assert!(
        output.status.success(),
        "the probe exited with {:?}; stderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("a probe's report is UTF-8");

    let ndjson = stdout
        .lines()
        .find(|line| line.starts_with('{'))
        .expect("a cost probe emits one NDJSON line")
        .to_owned();

    // A table row starts at column 0 with a bare label and has a number next.
    // Later columns are allowed: `request_cost` prints two ratio columns after
    // the figure, and requiring exactly two tokens found zero rows there --
    // caught by the emptiness assertion below rather than by passing quietly,
    // which is the whole reason that assertion exists.
    //
    // Prose is excluded by the two conditions together: indented lines fail the
    // first, and sentences fail the second because their second word is not a
    // number.
    let rows = stdout
        .lines()
        .filter(|line| line.starts_with(|c: char| c.is_ascii_alphabetic()))
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let label = parts.next()?;
            let value: f64 = parts.next()?.parse().ok()?;
            Some((label.to_owned(), value))
        })
        .collect();

    (rows, ndjson)
}

/// The value NDJSON gave `key`, if it carries one.
fn ndjson_number(line: &str, key: &str) -> Option<f64> {
    let needle = format!("\"{key}\":");
    let start = line.find(&needle)? + needle.len();
    let rest = &line[start..];
    let end = rest.find([',', '}']).unwrap_or(rest.len());
    rest[..end].trim().parse().ok()
}

fn assert_table_matches_ndjson(binary: &str, key_for: impl Fn(&str) -> &'static str) {
    let (rows, ndjson) = run(binary);

    assert!(
        !rows.is_empty(),
        "no table rows were read from {binary}, so this test is checking \
         nothing -- the prose table's shape has probably changed"
    );

    for (label, prose) in rows {
        let key = key_for(&label);
        let json = ndjson_number(&ndjson, key).unwrap_or_else(|| {
            panic!(
                "the table printed `{label}` but the NDJSON carries no `{key}`, \
                 so a figure reached a reader and not a mining pass\n{ndjson}"
            )
        });

        // Compared as rendered, not as floats. Both renderings format to one
        // decimal place from the same `f64`, so equality here is exact; an
        // epsilon would hide precisely the formatting drift worth catching.
        assert_eq!(
            format!("{prose:.1}"),
            format!("{json:.1}"),
            "`{label}` reads {prose} in the table and {json} under `{key}`"
        );
    }
}

#[test]
fn the_doorbell_probe_prints_every_figure_to_both_readers() {
    assert_table_matches_ndjson(
        env!("CARGO_BIN_EXE_probe-doorbell-cost"),
        windows_platform_probes::doorbell_cost::json_key,
    );
}

#[test]
fn the_request_probe_prints_every_figure_to_both_readers() {
    assert_table_matches_ndjson(
        env!("CARGO_BIN_EXE_probe-request-cost"),
        windows_platform_probes::request_cost::json_key,
    );
}
