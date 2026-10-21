// Copyright (c) 2026 Mike Grier
//! The debugger sink's emission (DI-12.1.3): this test binary starts itself as a process being debugged, runs
//! `a_process_that_reports_events` in it, and keeps each line the sink writes, through `win-debugging-sys`'s
//! `test-util` debugger, which uses only documented debugger functions (its WD-D-2 and WD-D-3).
//!
//! # Why this is an integration test
//!
//! It crosses a process boundary: a debugger is a second process, and what it receives is the operating system's.
//!
//! The expected lines are written out here and not built by the function under test, so the test says what a
//! reader of the debugger's output sees.

#![cfg(windows)]

use std::time::Duration;

use win_debugging_sys::run_debugged;
use win_durable_ioring::{
    DebugOutputDiagnostics, Diagnostic, DiagnosticSink, FlushDomain, FlushPart, IoTargetKey,
};

/// The name of the test the process being debugged runs.
const REPORTER: &str = "a_process_that_reports_events";

const BOUND: Duration = Duration::from_secs(30);

/// Run in the process being debugged: reports one event of each kind, and two whose text has a NUL. Run on its own,
/// with no debugger, the call does nothing, so this is harmless in an ordinary run of the tests.
#[test]
fn a_process_that_reports_events() {
    let sink = DebugOutputDiagnostics;
    let domain = FlushDomain::new(b"ssd-4".to_vec());
    let nul_domain = FlushDomain::new(vec![b'a', 0, b'b']);
    sink.report(&Diagnostic::Inconsistency {
        instance: 7,
        what: "a write completed twice",
        detail: format_args!("lineage 3"),
    });
    sink.report(&Diagnostic::LockWait {
        instance: 4,
        lock: "engine",
        waited: Duration::from_nanos(12),
    });
    sink.report(&Diagnostic::FlushIssued {
        instance: 2,
        lineage: 0,
        through: format_args!("5"),
        files: 3,
        domains: 1,
    });
    sink.report(&Diagnostic::FlushAnswered {
        instance: 2,
        lineage: 1,
        through: format_args!("9"),
        part: FlushPart::File(IoTargetKey(7)),
        elapsed: Duration::from_nanos(15),
        succeeded: false,
    });
    sink.report(&Diagnostic::FlushAnswered {
        instance: 2,
        lineage: 1,
        through: format_args!("9"),
        part: FlushPart::Domain(&domain),
        elapsed: Duration::from_nanos(40),
        succeeded: true,
    });
    sink.report(&Diagnostic::SealWaited {
        instance: 2,
        lineage: 0,
        through: format_args!("1"),
        waited: Duration::from_nanos(30),
    });
    sink.report(&Diagnostic::Inconsistency {
        instance: 1,
        what: "x",
        detail: format_args!("a\0b"),
    });
    sink.report(&Diagnostic::FlushAnswered {
        instance: 1,
        lineage: 0,
        through: format_args!("1"),
        part: FlushPart::Domain(&nul_domain),
        elapsed: Duration::from_nanos(1),
        succeeded: true,
    });
}

#[test]
fn a_debugger_receives_each_event_as_a_dioring_line_in_order() {
    let exe = std::env::current_exe().expect("the test binary's path");
    let observed = run_debugged(&exe, &["--exact", REPORTER, "--test-threads=1"], BOUND)
        .expect("the process was debugged");
    assert_eq!(observed.exit_code, Some(0), "the reporter did not pass");

    let received: Vec<String> = observed.strings.iter().map(|line| line.text()).collect();
    assert_eq!(
        received,
        [
            "dioring: instance 7 found its own state inconsistent: a write completed twice (lineage 3)",
            "dioring: instance 4 waited 12ns for its engine lock",
            "dioring: instance 2 lineage 0 seal through 5: issued flushes of 3 files and 1 domains",
            "dioring: instance 2 lineage 1 seal through 9: flush of file 7 failed after 15ns",
            "dioring: instance 2 lineage 1 seal through 9: flush of domain \"ssd-4\" succeeded after 40ns",
            "dioring: instance 2 lineage 0 seal through 1: waited 30ns for its writes",
            "dioring: instance 1 found its own state inconsistent: x (a\\0b)",
            "dioring: instance 1 lineage 0 seal through 1: flush of domain \"a\\0b\" succeeded after 1ns",
        ]
    );
    assert!(observed.strings.iter().all(|line| line.unicode));
}
