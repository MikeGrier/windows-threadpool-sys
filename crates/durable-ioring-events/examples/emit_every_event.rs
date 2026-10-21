// Copyright (c) 2026 Mike Grier
//! Writes one of each event through `EtwDiagnostics`, for a person to see them decoded: start a session for the provider,
//! install the manifest as the README says, run this, stop the session, and read the trace with a tool that decodes by manifest.
//! Each pair's begin and end are written, the begin as `win:Start` and the end as `win:Stop`.

#![cfg(windows)]

use durable_ioring::{
    CorrelationId, Diagnostic, DiagnosticKind, DiagnosticSink, FlushDomain, FlushOutcome,
    FlushPart, IoTargetKey, SealOutcome,
};
use durable_ioring_events::EtwDiagnostics;

/// A sink that wants every kind, to mint the pairs' ids from, so the example writes whether or not a session listens.
struct Wanting;

impl DiagnosticSink for Wanting {
    fn wants(&self, _: DiagnosticKind) -> bool {
        true
    }

    fn report(&self, _: &Diagnostic<'_>) {}
}

fn main() {
    let sink = EtwDiagnostics::new();
    let id = |kind| CorrelationId::for_pair(&Wanting, kind).expect("wanted");
    let domain = FlushDomain::new(b"ssd-4".to_vec());
    sink.report(&Diagnostic::Inconsistency {
        instance: 7,
        what: "a write completed twice",
        detail: format_args!("lineage {}", 3),
    });
    let lock = id(DiagnosticKind::LockWait);
    sink.report(&Diagnostic::LockWaitBegan {
        instance: 4,
        lock: "engine",
        id: lock,
    });
    sink.report(&Diagnostic::LockWaitEnded {
        instance: 4,
        lock: "engine",
        id: lock,
    });
    let seal = id(DiagnosticKind::Seal);
    sink.report(&Diagnostic::SealBegan {
        instance: 2,
        lineage: 1,
        through: format_args!("9"),
        id: seal,
    });
    let file = id(DiagnosticKind::Flush);
    sink.report(&Diagnostic::FlushBegan {
        instance: 2,
        lineage: 1,
        through: format_args!("9"),
        part: FlushPart::File(IoTargetKey(77)),
        id: file,
    });
    let part = id(DiagnosticKind::Flush);
    sink.report(&Diagnostic::FlushBegan {
        instance: 2,
        lineage: 1,
        through: format_args!("9"),
        part: FlushPart::Domain(&domain),
        id: part,
    });
    sink.report(&Diagnostic::FlushEnded {
        instance: 2,
        lineage: 1,
        through: format_args!("9"),
        part: FlushPart::File(IoTargetKey(77)),
        id: file,
        outcome: FlushOutcome::Failed,
    });
    sink.report(&Diagnostic::FlushEnded {
        instance: 2,
        lineage: 1,
        through: format_args!("9"),
        part: FlushPart::Domain(&domain),
        id: part,
        outcome: FlushOutcome::Succeeded,
    });
    sink.report(&Diagnostic::SealEnded {
        instance: 2,
        lineage: 1,
        through: format_args!("9"),
        id: seal,
        outcome: SealOutcome::Failed,
    });
}
