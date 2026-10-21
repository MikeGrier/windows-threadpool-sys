// Copyright (c) 2026 Mike Grier
//! The diagnostic sinks (DI-D-51): what each wants, and where a fan-out sends an event.

use std::sync::{Arc, Mutex};

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
fn each_pair_event_is_of_its_kind() {
    use crate::correlation::CorrelationId;
    let id = CorrelationId::chosen(1);
    let seal = Diagnostic::SealBegan {
        instance: 1,
        lineage: 0,
        through: format_args!("3"),
        id,
    };
    let flush = Diagnostic::FlushBegan {
        instance: 1,
        lineage: 0,
        through: format_args!("3"),
        part: FlushPart::File(IoTargetKey(2)),
        id,
    };
    let lock = Diagnostic::LockWaitEnded {
        instance: 1,
        lock: "engine",
        id,
    };
    assert_eq!(seal.kind(), DiagnosticKind::Seal);
    assert_eq!(flush.kind(), DiagnosticKind::Flush);
    assert_eq!(lock.kind(), DiagnosticKind::LockWait);
}

#[test]
fn a_flush_end_names_its_part_its_pair_and_how_it_ended() {
    use super::FlushOutcome;
    use crate::correlation::CorrelationId;
    let id = CorrelationId::chosen(4);
    let file = Diagnostic::FlushEnded {
        instance: 2,
        lineage: 1,
        through: format_args!("9"),
        part: FlushPart::File(IoTargetKey(7)),
        id,
        outcome: FlushOutcome::Failed,
    };
    assert_eq!(
        file.to_string(),
        "instance 2 lineage 1 seal through 9: flush of file 7 (pair 4) failed"
    );
    let domain = FlushDomain::new(b"ssd".to_vec());
    let part = Diagnostic::FlushEnded {
        instance: 2,
        lineage: 1,
        through: format_args!("9"),
        part: FlushPart::Domain(&domain),
        id,
        outcome: FlushOutcome::Succeeded,
    };
    assert_eq!(
        part.to_string(),
        "instance 2 lineage 1 seal through 9: flush of domain \"ssd\" (pair 4) succeeded"
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
fn a_line_is_prefixed_dioring_colon_and_a_space() {
    assert_eq!(super::LINE_PREFIX, "dioring: ");
}

#[test]
fn stderr_diagnostics_wants_every_pair_kind() {
    for kind in [
        DiagnosticKind::LockWait,
        DiagnosticKind::Seal,
        DiagnosticKind::Flush,
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

#[test]
fn each_end_of_a_pair_says_its_kind_id_and_edge_and_nothing_else_is_a_pair() {
    use super::{Edge, FlushOutcome, Pairing, SealOutcome};
    use crate::correlation::CorrelationId;
    let id = CorrelationId::chosen(42);
    let domain = FlushDomain::new(b"log".as_slice());
    let pairing = |event: &Diagnostic<'_>| event.pairing();
    let expect = |kind, edge| Some(Pairing { kind, id, edge });
    assert_eq!(
        pairing(&Diagnostic::SealBegan {
            instance: 1,
            lineage: 0,
            through: format_args!("1"),
            id
        }),
        expect(DiagnosticKind::Seal, Edge::Begin)
    );
    assert_eq!(
        pairing(&Diagnostic::SealEnded {
            instance: 1,
            lineage: 0,
            through: format_args!("1"),
            id,
            outcome: SealOutcome::Durable
        }),
        expect(DiagnosticKind::Seal, Edge::End)
    );
    assert_eq!(
        pairing(&Diagnostic::FlushBegan {
            instance: 1,
            lineage: 0,
            through: format_args!("1"),
            part: FlushPart::Domain(&domain),
            id
        }),
        expect(DiagnosticKind::Flush, Edge::Begin)
    );
    assert_eq!(
        pairing(&Diagnostic::FlushEnded {
            instance: 1,
            lineage: 0,
            through: format_args!("1"),
            part: FlushPart::File(IoTargetKey(3)),
            id,
            outcome: FlushOutcome::Abandoned
        }),
        expect(DiagnosticKind::Flush, Edge::End)
    );
    assert_eq!(
        pairing(&Diagnostic::LockWaitBegan {
            instance: 1,
            lock: "engine",
            id
        }),
        expect(DiagnosticKind::LockWait, Edge::Begin)
    );
    assert_eq!(
        pairing(&Diagnostic::LockWaitEnded {
            instance: 1,
            lock: "engine",
            id
        }),
        expect(DiagnosticKind::LockWait, Edge::End)
    );
    assert_eq!(
        pairing(&Diagnostic::Inconsistency {
            instance: 1,
            what: "x",
            detail: format_args!("y")
        }),
        None
    );
}

#[test]
fn every_end_of_a_pair_shows_its_id_and_how_it_ended() {
    use super::{FlushOutcome, SealOutcome};
    use crate::correlation::CorrelationId;
    let id = CorrelationId::chosen(17);
    let lines = [
        Diagnostic::SealBegan {
            instance: 2,
            lineage: 1,
            through: format_args!("9"),
            id,
        }
        .to_string(),
        Diagnostic::SealEnded {
            instance: 2,
            lineage: 1,
            through: format_args!("9"),
            id,
            outcome: SealOutcome::Blocked,
        }
        .to_string(),
        Diagnostic::SealEnded {
            instance: 2,
            lineage: 1,
            through: format_args!("9"),
            id,
            outcome: SealOutcome::Ended,
        }
        .to_string(),
        Diagnostic::FlushBegan {
            instance: 2,
            lineage: 1,
            through: format_args!("9"),
            part: FlushPart::File(IoTargetKey(4)),
            id,
        }
        .to_string(),
        Diagnostic::FlushEnded {
            instance: 2,
            lineage: 1,
            through: format_args!("9"),
            part: FlushPart::File(IoTargetKey(4)),
            id,
            outcome: FlushOutcome::Failed,
        }
        .to_string(),
        Diagnostic::LockWaitBegan {
            instance: 2,
            lock: "engine",
            id,
        }
        .to_string(),
        Diagnostic::LockWaitEnded {
            instance: 2,
            lock: "engine",
            id,
        }
        .to_string(),
    ];
    for line in &lines {
        assert!(line.contains("(pair 17)"), "{line}");
        assert!(line.starts_with("instance 2"), "{line}");
    }
    assert!(lines[0].ends_with("began"));
    assert!(lines[1].ends_with("blocked"));
    assert!(lines[2].ends_with("ended unanswered"));
    assert!(lines[3].contains("flush of file 4") && lines[3].ends_with("began"));
    assert!(lines[4].ends_with("failed"));
    assert!(lines[5].contains("waits for its engine lock"));
    assert!(lines[6].contains("took its engine lock"));
}
