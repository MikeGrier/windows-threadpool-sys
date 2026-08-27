// Copyright (c) 2026 Mike Grier
//! Tests for `capture`'s argument parsing.
//!
//! Table-driven over the error branches, because those are the ones a user
//! actually hits and the ones that regress silently: they run only when the
//! command line is wrong, which no other test exercises.

use super::{Args, Output};

/// An `Output` capturing both streams, so a test can assert the diagnostic was
/// produced rather than only that parsing failed.
fn sink() -> Output<Vec<u8>, Vec<u8>> {
    Output {
        stderr: Vec::new(),
        stdout: Vec::new(),
    }
}

fn args(raw: &[&str]) -> Vec<String> {
    raw.iter().map(|s| (*s).to_string()).collect()
}

#[test]
fn an_empty_command_line_uses_the_documented_defaults() {
    let mut output = sink();
    let parsed = Args::parse_from(args(&[]), &mut output).expect("defaults are valid");
    assert_eq!(parsed.seeds, 1000);
    assert_eq!(parsed.start, 0);
    assert_eq!(parsed.out, std::path::PathBuf::from("captures"));
    assert!(output.stderr.is_empty(), "a valid parse says nothing");
}

#[test]
fn every_option_is_honoured() {
    let mut output = sink();
    let parsed = Args::parse_from(
        args(&["--seeds", "7", "--start", "42", "--out", "somewhere"]),
        &mut output,
    )
    .expect("valid");
    assert_eq!(parsed.seeds, 7);
    assert_eq!(parsed.start, 42);
    assert_eq!(parsed.out, std::path::PathBuf::from("somewhere"));
}

#[test]
fn a_later_option_wins_over_an_earlier_one() {
    // Duplicates are accepted rather than rejected -- last-wins is the ordinary
    // CLI convention and is what a shell alias plus an override produces.
    let mut output = sink();
    let parsed =
        Args::parse_from(args(&["--seeds", "3", "--seeds", "9"]), &mut output).expect("valid");
    assert_eq!(parsed.seeds, 9);
}

#[test]
fn every_error_branch_reports_and_refuses() {
    // Each case is (command line, a distinctive fragment of its diagnostic).
    let cases: &[(&[&str], &str)] = &[
        (&["--seeds"], "--seeds needs a value"),
        (&["--start"], "--start needs a value"),
        (&["--out"], "--out needs a value"),
        (&["--seeds", "x"], "--seeds must be a number"),
        (&["--start", "-1"], "--start must be a number"),
        (&["--bogus"], "unrecognized argument '--bogus'"),
        (&["stray"], "unrecognized argument 'stray'"),
        (&["--seeds", "0"], "--seeds must be at least 1"),
        (
            &["--start", "18446744073709551615", "--seeds", "2"],
            "overflows a u64",
        ),
    ];

    for (line, fragment) in cases {
        let mut output = sink();
        assert!(
            Args::parse_from(args(line), &mut output).is_none(),
            "{line:?} must be refused"
        );
        let stderr = String::from_utf8(output.stderr).expect("utf-8");
        assert!(
            stderr.contains(fragment),
            "{line:?}: expected a diagnostic containing {fragment:?}, got {stderr:?}"
        );
        assert!(
            stderr.contains("usage: capture"),
            "{line:?}: every refusal should show the usage line, got {stderr:?}"
        );
    }
}

#[test]
fn the_largest_non_overflowing_range_is_accepted() {
    // The boundary the overflow check must not reject: start + seeds == u64::MAX.
    let mut output = sink();
    let parsed = Args::parse_from(
        args(&["--start", "18446744073709551614", "--seeds", "1"]),
        &mut output,
    )
    .expect("start + seeds == u64::MAX is representable");
    assert_eq!(parsed.start, u64::MAX - 1);
    assert_eq!(parsed.seeds, 1);
}
