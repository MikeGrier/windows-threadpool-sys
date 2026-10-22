// Copyright (c) 2026 Mike Grier
//! The debugger sink's formatting, apart from its emission (DI-12.1.3): the line each event becomes, what it wants,
//! and the one change made to a line. That a line reaches a debugger is `tests/debug_output.rs`.

use durable_ioring::{
    CorrelationId, Diagnostic, DiagnosticKind, DiagnosticSink, FlushDomain, FlushOutcome,
    FlushPart, IoTargetKey, SealOutcome,
};

use super::{DebugOutputDiagnostics, line};

const KINDS: [DiagnosticKind; 4] = [
    DiagnosticKind::Inconsistency,
    DiagnosticKind::LockWait,
    DiagnosticKind::Seal,
    DiagnosticKind::Flush,
];

/// A pair id, minted as the core mints one: the debugger sink wants every kind.
fn minted() -> CorrelationId {
    CorrelationId::for_pair(&DebugOutputDiagnostics, DiagnosticKind::Seal).expect("wanted")
}

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
fn a_lock_waits_pair_is_two_dioring_lines() {
    let id = minted();
    assert_eq!(
        line(&Diagnostic::LockWaitBegan {
            instance: 4,
            lock: "engine",
            id
        }),
        format!("dioring: instance 4 (pair {id}): waits for its engine lock")
    );
    assert_eq!(
        line(&Diagnostic::LockWaitEnded {
            instance: 4,
            lock: "engine",
            id
        }),
        format!("dioring: instance 4 (pair {id}): took its engine lock")
    );
}

#[test]
fn a_seals_pair_is_two_dioring_lines() {
    let id = minted();
    assert_eq!(
        line(&Diagnostic::SealBegan {
            instance: 2,
            lineage: 0,
            through: format_args!("5"),
            id
        }),
        format!("dioring: instance 2 lineage 0 seal through 5 (pair {id}): began")
    );
    assert_eq!(
        line(&Diagnostic::SealEnded {
            instance: 2,
            lineage: 0,
            through: format_args!("5"),
            id,
            outcome: SealOutcome::Blocked
        }),
        format!("dioring: instance 2 lineage 0 seal through 5 (pair {id}): blocked")
    );
}

#[test]
fn a_file_flush_is_two_dioring_lines() {
    let id = minted();
    let part = FlushPart::File(IoTargetKey(7));
    assert_eq!(
        line(&Diagnostic::FlushBegan {
            instance: 2,
            lineage: 1,
            through: format_args!("9"),
            part,
            id
        }),
        format!("dioring: instance 2 lineage 1 seal through 9: flush of file 7 (pair {id}) began")
    );
    assert_eq!(
        line(&Diagnostic::FlushEnded {
            instance: 2,
            lineage: 1,
            through: format_args!("9"),
            part,
            id,
            outcome: FlushOutcome::Failed
        }),
        format!("dioring: instance 2 lineage 1 seal through 9: flush of file 7 (pair {id}) failed")
    );
}

#[test]
fn a_domain_flush_end_is_a_dioring_line() {
    let id = minted();
    let domain = FlushDomain::new(b"ssd-4".to_vec());
    assert_eq!(
        line(&Diagnostic::FlushEnded {
            instance: 2,
            lineage: 1,
            through: format_args!("9"),
            part: FlushPart::Domain(&domain),
            id,
            outcome: FlushOutcome::Succeeded
        }),
        format!(
            "dioring: instance 2 lineage 1 seal through 9: flush of domain \"ssd-4\" (pair {id}) succeeded"
        )
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
    let event = Diagnostic::FlushBegan {
        instance: 1,
        lineage: 0,
        through: format_args!("1"),
        part: FlushPart::Domain(&domain),
        id: minted(),
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
    let id = minted();
    sink.report(&Diagnostic::LockWaitBegan {
        instance: 1,
        lock: "engine",
        id,
    });
    sink.report(&Diagnostic::LockWaitEnded {
        instance: 1,
        lock: "engine",
        id,
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
