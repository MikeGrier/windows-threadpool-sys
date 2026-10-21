// Copyright (c) 2026 Mike Grier
//! The debugger sink's formatting, apart from its emission (DI-12.1.3): the line each event becomes, what it wants,
//! and the one change made to a line. That a line reaches a debugger is `tests/debug_output.rs`.

use std::time::Duration;

use durable_ioring::{
    Diagnostic, DiagnosticKind, DiagnosticSink, FlushDomain, FlushPart, IoTargetKey,
};

use super::{DebugOutputDiagnostics, line};

const KINDS: [DiagnosticKind; 5] = [
    DiagnosticKind::Inconsistency,
    DiagnosticKind::LockWait,
    DiagnosticKind::FlushIssued,
    DiagnosticKind::FlushAnswered,
    DiagnosticKind::SealWaited,
];

#[test]
fn it_wants_every_kind() {
    for kind in KINDS {
        assert!(DebugOutputDiagnostics.wants(kind), "{kind:?}");
    }
}

#[test]
fn an_inconsistency_is_a_dioring_line() {
    let event = Diagnostic::Inconsistency {
        instance: 7,
        what: "a write completed twice",
        detail: format_args!("lineage 3"),
    };
    assert_eq!(
        line(&event),
        "dioring: instance 7 found its own state inconsistent: a write completed twice (lineage 3)"
    );
}

#[test]
fn a_lock_wait_is_a_dioring_line() {
    let event = Diagnostic::LockWait {
        instance: 4,
        lock: "engine",
        waited: Duration::from_nanos(12),
    };
    assert_eq!(
        line(&event),
        "dioring: instance 4 waited 12ns for its engine lock"
    );
}

#[test]
fn a_flush_issued_is_a_dioring_line() {
    let event = Diagnostic::FlushIssued {
        instance: 2,
        lineage: 0,
        through: format_args!("5"),
        files: 3,
        domains: 1,
    };
    assert_eq!(
        line(&event),
        "dioring: instance 2 lineage 0 seal through 5: issued flushes of 3 files and 1 domains"
    );
}

#[test]
fn a_file_flush_answered_is_a_dioring_line() {
    let event = Diagnostic::FlushAnswered {
        instance: 2,
        lineage: 1,
        through: format_args!("9"),
        part: FlushPart::File(IoTargetKey(7)),
        elapsed: Duration::from_nanos(15),
        succeeded: false,
    };
    assert_eq!(
        line(&event),
        "dioring: instance 2 lineage 1 seal through 9: flush of file 7 failed after 15ns"
    );
}

#[test]
fn a_domain_flush_answered_is_a_dioring_line() {
    let domain = FlushDomain::new(b"ssd-4".to_vec());
    let event = Diagnostic::FlushAnswered {
        instance: 2,
        lineage: 1,
        through: format_args!("9"),
        part: FlushPart::Domain(&domain),
        elapsed: Duration::from_nanos(40),
        succeeded: true,
    };
    assert_eq!(
        line(&event),
        "dioring: instance 2 lineage 1 seal through 9: flush of domain \"ssd-4\" succeeded after 40ns"
    );
}

#[test]
fn a_seal_waited_is_a_dioring_line() {
    let event = Diagnostic::SealWaited {
        instance: 2,
        lineage: 0,
        through: format_args!("1"),
        waited: Duration::from_nanos(30),
    };
    assert_eq!(
        line(&event),
        "dioring: instance 2 lineage 0 seal through 1: waited 30ns for its writes"
    );
}

#[test]
fn a_nul_in_a_detail_is_written_as_backslash_zero_and_the_rest_is_left() {
    let event = Diagnostic::Inconsistency {
        instance: 1,
        what: "x",
        detail: format_args!("a\0b\0\0c"),
    };
    let text = line(&event);
    assert_eq!(
        text,
        "dioring: instance 1 found its own state inconsistent: x (a\\0b\\0\\0c)"
    );
    assert!(!text.contains('\0'));
}

#[test]
fn a_nul_in_a_domains_name_is_written_as_backslash_zero() {
    let domain = FlushDomain::new(vec![b'a', 0, b'b']);
    let event = Diagnostic::FlushAnswered {
        instance: 1,
        lineage: 0,
        through: format_args!("1"),
        part: FlushPart::Domain(&domain),
        elapsed: Duration::ZERO,
        succeeded: true,
    };
    let text = line(&event);
    assert!(text.contains("domain \"a\\0b\""), "{text}");
    assert!(!text.contains('\0'));
}

#[test]
fn a_backslash_and_a_zero_already_in_the_text_are_left_as_they_are() {
    let event = Diagnostic::Inconsistency {
        instance: 1,
        what: "x",
        detail: format_args!("a\\0b"),
    };
    assert_eq!(
        line(&event),
        "dioring: instance 1 found its own state inconsistent: x (a\\0b)"
    );
}

#[test]
fn a_line_with_text_outside_ascii_keeps_it() {
    let event = Diagnostic::Inconsistency {
        instance: 1,
        what: "\u{3042}",
        detail: format_args!("\u{1f600}"),
    };
    assert_eq!(
        line(&event),
        "dioring: instance 1 found its own state inconsistent: \u{3042} (\u{1f600})"
    );
}

#[test]
fn reporting_makes_the_call_and_does_not_fail_for_any_event() {
    // With no debugger the call does nothing; this shows each report is made, with a NUL in the text too.
    let sink = DebugOutputDiagnostics;
    sink.report(&Diagnostic::Inconsistency {
        instance: 1,
        what: "x",
        detail: format_args!("a\0b"),
    });
    sink.report(&Diagnostic::LockWait {
        instance: 1,
        lock: "engine",
        waited: Duration::ZERO,
    });
}

#[test]
fn the_sink_is_copy_default_and_debug() {
    let a = DebugOutputDiagnostics;
    let b = a;
    let c: DebugOutputDiagnostics = Default::default();
    let _ = (a, b, c);
    assert_eq!(format!("{a:?}"), "DebugOutputDiagnostics");
}
