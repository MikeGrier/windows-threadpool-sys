// Copyright (c) 2026 Mike Grier
//! The diagnostic sinks (DI-D-51): what each wants, and where a fan-out sends an event.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::{
    Diagnostic, DiagnosticKind, DiagnosticSink, FanOutDiagnostics, FlushPart, NoDiagnostics,
    StderrDiagnostics,
};
use crate::types::{FlushDomain, IoTargetKey};

/// A sink that keeps each event it is given, as text, and wants only what it was told to.
#[derive(Default)]
pub(crate) struct Recording {
    pub(crate) wanted: bool,
    pub(crate) seen: Mutex<Vec<String>>,
}

impl Recording {
    pub(crate) fn wanting() -> Arc<Self> {
        Arc::new(Self {
            wanted: true,
            seen: Mutex::new(Vec::new()),
        })
    }

    pub(crate) fn seen(&self) -> Vec<String> {
        self.seen.lock().expect("the record").clone()
    }
}

impl DiagnosticSink for Recording {
    fn wants(&self, _: DiagnosticKind) -> bool {
        self.wanted
    }

    fn report(&self, event: &Diagnostic<'_>) {
        self.seen
            .lock()
            .expect("the record")
            .push(event.to_string());
    }
}

fn report(sink: &dyn DiagnosticSink, what: &'static str, n: u32) {
    let event = Diagnostic::Inconsistency {
        instance: 7,
        what,
        detail: format_args!("lineage {n}"),
    };
    if sink.wants(event.kind()) {
        sink.report(&event);
    }
}

#[test]
fn no_diagnostics_wants_nothing() {
    assert!(!NoDiagnostics.wants(DiagnosticKind::Inconsistency));
}

#[test]
fn stderr_diagnostics_wants_everything() {
    assert!(StderrDiagnostics.wants(DiagnosticKind::Inconsistency));
}

#[test]
fn an_inconsistency_names_its_instance_what_and_detail() {
    let sink = Recording::wanting();
    report(sink.as_ref(), "a write completed twice", 3);
    assert_eq!(
        sink.seen(),
        ["instance 7 found its own state inconsistent: a write completed twice (lineage 3)"]
    );
}

#[test]
fn an_event_is_of_its_kind() {
    let event = Diagnostic::Inconsistency {
        instance: 1,
        what: "x",
        detail: format_args!(""),
    };
    assert_eq!(event.kind(), DiagnosticKind::Inconsistency);
}

#[test]
fn each_delay_event_is_of_its_kind() {
    let issued = Diagnostic::FlushIssued {
        instance: 1,
        lineage: 0,
        through: format_args!("3"),
        files: 1,
        domains: 0,
    };
    let answered = Diagnostic::FlushAnswered {
        instance: 1,
        lineage: 0,
        through: format_args!("3"),
        part: FlushPart::File(IoTargetKey(2)),
        elapsed: Duration::from_nanos(4),
        succeeded: true,
    };
    let waited = Diagnostic::SealWaited {
        instance: 1,
        lineage: 0,
        through: format_args!("3"),
        waited: Duration::from_nanos(5),
    };
    let lock = Diagnostic::LockWait {
        instance: 1,
        lock: "engine",
        waited: Duration::from_nanos(6),
    };
    assert_eq!(issued.kind(), DiagnosticKind::FlushIssued);
    assert_eq!(answered.kind(), DiagnosticKind::FlushAnswered);
    assert_eq!(waited.kind(), DiagnosticKind::SealWaited);
    assert_eq!(lock.kind(), DiagnosticKind::LockWait);
}

#[test]
fn a_flush_answer_names_its_part_its_outcome_and_its_time() {
    let file = Diagnostic::FlushAnswered {
        instance: 2,
        lineage: 1,
        through: format_args!("9"),
        part: FlushPart::File(IoTargetKey(7)),
        elapsed: Duration::from_nanos(15),
        succeeded: false,
    };
    assert_eq!(
        file.to_string(),
        "instance 2 lineage 1 seal through 9: flush of file 7 failed after 15ns"
    );
    let domain = FlushDomain::new(b"ssd".to_vec());
    let part = Diagnostic::FlushAnswered {
        instance: 2,
        lineage: 1,
        through: format_args!("9"),
        part: FlushPart::Domain(&domain),
        elapsed: Duration::from_nanos(1),
        succeeded: true,
    };
    assert_eq!(
        part.to_string(),
        "instance 2 lineage 1 seal through 9: flush of domain \"ssd\" succeeded after 1ns"
    );
}

#[test]
fn a_domain_whose_name_is_not_text_is_shown_with_the_replacement_character() {
    let domain = FlushDomain::new(vec![0xff, b'a']);
    assert_eq!(
        FlushPart::Domain(&domain).to_string(),
        "domain \"\u{fffd}a\""
    );
}

#[test]
fn stderr_diagnostics_wants_every_delay_kind() {
    for kind in [
        DiagnosticKind::LockWait,
        DiagnosticKind::FlushIssued,
        DiagnosticKind::FlushAnswered,
        DiagnosticKind::SealWaited,
    ] {
        assert!(StderrDiagnostics.wants(kind), "{kind:?}");
    }
}

#[test]
fn a_fan_out_reports_to_every_sink_that_wants_the_kind_in_order() {
    let first = Recording::wanting();
    let deaf = Arc::new(Recording::default());
    let second = Recording::wanting();
    let fan = FanOutDiagnostics::new(vec![first.clone(), deaf.clone(), second.clone()]);
    report(&fan, "one", 1);
    report(&fan, "two", 2);
    assert_eq!(first.seen().len(), 2);
    assert_eq!(
        second.seen(),
        first.seen(),
        "each sink sees the same events, in order"
    );
    assert!(
        deaf.seen().is_empty(),
        "a sink that wants nothing is never given anything"
    );
}

#[test]
fn a_fan_out_wants_what_any_of_its_sinks_wants() {
    let deaf: Arc<dyn DiagnosticSink> = Arc::new(Recording::default());
    let hearing: Arc<dyn DiagnosticSink> = Recording::wanting();
    assert!(!FanOutDiagnostics::new(vec![deaf.clone()]).wants(DiagnosticKind::Inconsistency));
    assert!(FanOutDiagnostics::new(vec![deaf, hearing]).wants(DiagnosticKind::Inconsistency));
    assert!(!FanOutDiagnostics::default().wants(DiagnosticKind::Inconsistency));
}

#[test]
fn a_fan_out_debugs_by_its_count() {
    let fan = FanOutDiagnostics::new(vec![Arc::new(NoDiagnostics), Arc::new(StderrDiagnostics)]);
    assert_eq!(format!("{fan:?}"), "FanOutDiagnostics { sinks: 2 }");
}
