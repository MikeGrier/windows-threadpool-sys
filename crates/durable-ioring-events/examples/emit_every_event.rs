// Copyright (c) 2026 Mike Grier
//! Writes one of each event through `EtwDiagnostics`, for a person to see them decoded: start a session for the provider,
//! install the manifest as the README says, run this, stop the session, and read the trace with a tool that decodes by manifest.

#![cfg(windows)]

use std::time::Duration;

use durable_ioring::{Diagnostic, DiagnosticSink, FlushDomain, FlushPart, IoTargetKey};
use durable_ioring_events::EtwDiagnostics;

fn main() {
    let sink = EtwDiagnostics::new();
    let domain = FlushDomain::new(b"ssd-4".to_vec());
    sink.report(&Diagnostic::Inconsistency {
        instance: 7,
        what: "a write completed twice",
        detail: format_args!("lineage {}", 3),
    });
    sink.report(&Diagnostic::LockWait {
        instance: 4,
        lock: "engine",
        waited: Duration::from_nanos(12),
    });
    sink.report(&Diagnostic::FlushIssued {
        instance: 2,
        lineage: 1,
        through: format_args!("5"),
        files: 3,
        domains: 1,
    });
    sink.report(&Diagnostic::FlushAnswered {
        instance: 2,
        lineage: 1,
        through: format_args!("9"),
        part: FlushPart::File(IoTargetKey(77)),
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
}
