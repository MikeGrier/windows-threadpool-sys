// Copyright (c) 2026 Mike Grier
//! The sink with no session listening (DE-D-4): it wants nothing, builds nothing, and shares one registration. What it does for
//! a session that is listening is `tests/session.rs`.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use durable_ioring::{
    Diagnostic, DiagnosticKind, DiagnosticSink, FlushDomain, FlushPart, IoTargetKey,
};

use super::EtwDiagnostics;

const KINDS: [DiagnosticKind; 5] = [
    DiagnosticKind::Inconsistency,
    DiagnosticKind::LockWait,
    DiagnosticKind::FlushIssued,
    DiagnosticKind::FlushAnswered,
    DiagnosticKind::SealWaited,
];

/// Counts how many times it is formatted: text that is built for an event that no one wants would show here.
struct Counted(Arc<AtomicU32>);

impl fmt::Display for Counted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fetch_add(1, Ordering::Relaxed);
        f.write_str("counted")
    }
}

#[test]
fn a_sink_registers_the_provider() {
    let sink = EtwDiagnostics::new();
    assert!(sink.is_registered());
    assert!(EtwDiagnostics::try_new().is_ok());
}

#[test]
fn with_no_session_a_sink_wants_no_kind() {
    let sink = EtwDiagnostics::new();
    for kind in KINDS {
        assert!(!sink.wants(kind), "{kind:?}");
    }
}

#[test]
fn with_no_session_reporting_builds_no_text_and_writes_nothing() {
    let sink = EtwDiagnostics::new();
    let formatted = Arc::new(AtomicU32::new(0));
    let detail = Counted(formatted.clone());
    let through = Counted(formatted.clone());
    sink.report(&Diagnostic::Inconsistency {
        instance: 1,
        what: "x",
        detail: format_args!("{detail}"),
    });
    sink.report(&Diagnostic::FlushIssued {
        instance: 1,
        lineage: 0,
        through: format_args!("{through}"),
        files: 1,
        domains: 0,
    });
    sink.report(&Diagnostic::SealWaited {
        instance: 1,
        lineage: 0,
        through: format_args!("{through}"),
        waited: Duration::ZERO,
    });
    assert_eq!(
        formatted.load(Ordering::Relaxed),
        0,
        "text was built for events no session was listening to"
    );
}

#[test]
fn with_no_session_every_kind_of_event_is_reported_with_no_failure() {
    let sink = EtwDiagnostics::new();
    let domain = FlushDomain::new(b"d".to_vec());
    sink.report(&Diagnostic::LockWait {
        instance: 1,
        lock: "engine",
        waited: Duration::ZERO,
    });
    sink.report(&Diagnostic::FlushAnswered {
        instance: 1,
        lineage: 0,
        through: format_args!("1"),
        part: FlushPart::File(IoTargetKey(1)),
        elapsed: Duration::ZERO,
        succeeded: true,
    });
    sink.report(&Diagnostic::FlushAnswered {
        instance: 1,
        lineage: 0,
        through: format_args!("1"),
        part: FlushPart::Domain(&domain),
        elapsed: Duration::ZERO,
        succeeded: false,
    });
}

#[test]
fn sinks_in_one_process_share_one_registration() {
    let a = EtwDiagnostics::new();
    let b = EtwDiagnostics::new();
    let c = a.clone();
    let (ra, rb, rc) = (
        a.registration.as_ref().expect("registered"),
        b.registration.as_ref().expect("registered"),
        c.registration.as_ref().expect("registered"),
    );
    assert!(Arc::ptr_eq(ra, rb));
    assert!(Arc::ptr_eq(ra, rc));
}

#[test]
fn a_sink_that_could_not_register_wants_nothing_and_writes_nothing() {
    let sink = EtwDiagnostics { registration: None };
    assert!(!sink.is_registered());
    for kind in KINDS {
        assert!(!sink.wants(kind));
    }
    sink.report(&Diagnostic::LockWait {
        instance: 1,
        lock: "engine",
        waited: Duration::ZERO,
    });
}

#[test]
fn a_sink_is_default_constructed_and_debuggable() {
    let sink = EtwDiagnostics::default();
    assert!(sink.is_registered());
    assert!(format!("{sink:?}").starts_with("EtwDiagnostics"));
}

#[test]
fn sinks_are_made_and_dropped_on_many_threads_at_once() {
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                for _ in 0..50 {
                    let sink = EtwDiagnostics::new();
                    assert!(sink.is_registered());
                    let _ = sink.wants(DiagnosticKind::LockWait);
                }
            });
        }
    });
}
