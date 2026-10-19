// Copyright (c) Mike Grier.

//! The probes that take settings (`M4.2`) take them from a command line, report
//! what they used, and refuse what they cannot use -- on the real binaries, since
//! the refusal is a process exit and the report is what a reader captures.

use std::process::{Command, Output};

const QUEUE: &str = env!("CARGO_BIN_EXE_probe-queue-contention");
const REQUEST: &str = env!("CARGO_BIN_EXE_probe-request-cost");
const DOORBELL: &str = env!("CARGO_BIN_EXE_probe-doorbell-cost");

fn run(binary: &str, args: &[&str]) -> Output {
    Command::new(binary)
        .args(args)
        .output()
        .expect("run the probe")
}

/// A refusal exits with status 2, says why and how to call the probe on stderr,
/// and puts nothing on stdout: nothing was measured, so there is no report.
fn assert_refused(binary: &str, args: &[&str], why: &str) {
    let output = run(binary, args);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(2),
        "{binary} {args:?} was not refused; stderr: {stderr}"
    );
    assert!(stderr.contains(why), "{binary} {args:?}: {stderr}");
    assert!(
        stderr.contains("usage: probe-"),
        "{binary} {args:?}: {stderr}"
    );
    assert!(
        output.stdout.is_empty(),
        "{binary} {args:?} wrote a report it did not measure: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn each_probe_refuses_a_setting_it_does_not_take() {
    assert_refused(QUEUE, &["--iteration-multiplier", "2"], "is not a setting");
    assert_refused(REQUEST, &["--repetitions", "3"], "is not a setting");
    assert_refused(DOORBELL, &["--producers", "1"], "is not a setting");
    assert_refused(REQUEST, &["--help"], "is not a setting");
}

#[test]
fn each_probe_refuses_a_value_it_cannot_use() {
    assert_refused(QUEUE, &["--repetitions", "0"], "must be at least 1");
    assert_refused(QUEUE, &["--producers", "1,1"], "listed more than once");
    assert_refused(
        REQUEST,
        &["--iteration-multiplier", "x"],
        "is not a whole number",
    );
    assert_refused(
        DOORBELL,
        &["--iteration-multiplier"],
        "needs a value after it",
    );
}

#[test]
fn a_queue_run_given_settings_reports_them_and_measures_only_the_counts_given() {
    let output = run(
        QUEUE,
        &[
            "--pushes-per-producer",
            "64",
            "--repetitions",
            "1",
            "--producers",
            "3,1",
        ],
    );
    let stdout = String::from_utf8(output.stdout).expect("a probe's report is UTF-8");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains(
            "sampling: 64 pushes per producer, median of 1 repetitions, one untimed warmup \
             pass, producer counts 3, 1"
        ),
        "{stdout}"
    );
    assert!(stdout.contains("NOT the defaults"), "{stdout}");
}

#[test]
fn a_cost_probe_given_a_multiplier_reports_it_beside_its_banner_and_in_its_row() {
    let output = run(REQUEST, &["--iteration-multiplier", "2"]);
    let stdout = String::from_utf8(output.stdout).expect("a probe's report is UTF-8");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let lines: Vec<&str> = stdout.lines().collect();
    assert!(
        lines[1].starts_with("iteration multiplier: 2 (NOT the default"),
        "the line after the banner says what the run used: {stdout}"
    );
    let row = lines
        .iter()
        .find(|line| line.starts_with('{'))
        .expect("the NDJSON row");
    assert!(row.contains(r#""iteration_multiplier":2,"#), "{row}");
}

#[test]
fn a_cost_probe_given_nothing_says_it_used_the_default() {
    let output = run(DOORBELL, &[]);
    let stdout = String::from_utf8(output.stdout).expect("a probe's report is UTF-8");
    assert!(output.status.success());
    assert!(
        stdout
            .lines()
            .nth(1)
            .is_some_and(|line| line == "iteration multiplier: 1 (the default)"),
        "{stdout}"
    );
    assert!(stdout.contains(r#""iteration_multiplier":1,"#), "{stdout}");
}
