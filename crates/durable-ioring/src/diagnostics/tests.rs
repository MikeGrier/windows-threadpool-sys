// Copyright (c) 2026 Mike Grier
//! The diagnostic sinks (DI-D-51): what each wants, and where a fan-out sends an event.

use std::sync::{Arc, Mutex};

use super::{
    Diagnostic, DiagnosticKind, DiagnosticSink, FanOutDiagnostics, NoDiagnostics, StderrDiagnostics,
};

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
