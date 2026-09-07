// Copyright (c) Mike Grier.

//! The one thing the unit tests cannot reach: that `Stdout` actually writes.
//!
//! `report::Captured` makes a probe's findings a value a test can read, and the
//! unit tests use it for everything. But that means every in-process test passes
//! whether or not `report::Stdout` -- the implementation every probe uses in
//! production -- emits anything at all.
//!
//! Found by mutation testing rather than by inspection: replacing
//! `<impl Report for Stdout>::line` with `()` survived the whole suite. A no-op
//! there means every probe in this crate runs, exits zero, and prints nothing,
//! which is the failure mode that looks most like success.
//!
//! Stable Rust cannot redirect this process's own stdout, so covering it needs a
//! real child process. That makes this an integration test by the repository's
//! own criterion -- it crosses a process boundary -- rather than by preference.

use std::process::Command;

/// A probe is exercised rather than the sink tested directly, because the sink
/// is only interesting as the thing a probe uses.
///
/// `probe-error-mode` is the one picked: it needs no privileges, no particular
/// hardware, and no device, so it behaves the same on a developer's machine and
/// on a CI runner. `CARGO_BIN_EXE_*` is set by cargo for integration tests, so
/// the path is the binary this build produced rather than whatever is on PATH.
#[test]
fn a_probe_run_as_a_process_prints_its_report() {
    let output = Command::new(env!("CARGO_BIN_EXE_probe-error-mode"))
        .output()
        .expect("run the probe");

    assert!(
        output.status.success(),
        "the probe exited with {:?}; stderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("a probe's report is UTF-8");

    // The assertion that kills the mutant. Everything below it is about the
    // report being *right*; this one is about it existing at all.
    assert!(
        !stdout.trim().is_empty(),
        "the probe produced no output, so nothing it measured reached anyone"
    );

    let lines: Vec<&str> = stdout.lines().collect();

    // The banner is rendered into the report rather than printed beside it, so
    // that a captured report carries the machine it describes. If it ever stops
    // being the first line, a pasted finding can be compared against hardware it
    // was not measured on.
    //
    // Matched on the `host:` prefix alone, which `banner_line` emits on both its
    // success and its topology-discovery-failed paths -- so this holds on a
    // machine where discovery fails, and does not encode this machine's shape.
    assert!(
        lines[0].starts_with("host:"),
        "the report's first line should be the host banner, was: {:?}",
        lines[0]
    );

    // A banner and nothing else would satisfy everything above while the
    // findings themselves went missing.
    assert!(
        lines.len() > 1,
        "the report was only its banner; the probe's own findings are missing"
    );
}
