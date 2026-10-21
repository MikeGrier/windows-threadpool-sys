// Copyright (c) 2026 Mike Grier
//! The driver's default diagnostic sink registers the ETW provider (DI-12.1.6): an instance built with no sink of its own does,
//! and one built with a sink of its own does not. What the default's events are, once a session listens, is
//! `src/dioring/tests/diagnostics.rs`.
//!
//! # Why this is an integration test
//!
//! It asks whether the provider is registered in the process, which any other instance alive in the same process would make
//! true, so it needs a process of its own, which an integration test file is.

#![cfg(windows)]

use std::sync::Arc;

use win_durable_ioring::{
    DeliveryMode, Dioring, EtwDiagnostics, NoDiagnostics, Setup, StderrDiagnostics,
};

type Ring = Dioring<Vec<u8>, u64, (), Vec<u8>>;

fn setup(
    diagnostics: Option<Arc<dyn win_durable_ioring::DiagnosticSink>>,
) -> Setup<'static, 'static, u64, Vec<u8>> {
    Setup {
        submission_queue_size: 16,
        completion_queue_size: 16,
        files: Vec::new(),
        buffers: Vec::new(),
        provider: None,
        diagnostics,
        delivery: DeliveryMode::Pool(None),
    }
}

#[test]
fn the_default_registers_the_provider_and_a_chosen_sink_does_not() {
    assert!(
        !EtwDiagnostics::provider_is_registered_in_process(),
        "before any instance"
    );

    // A sink of the consumer's own: nothing registers the provider.
    for chosen in [
        Arc::new(NoDiagnostics) as Arc<dyn win_durable_ioring::DiagnosticSink>,
        Arc::new(StderrDiagnostics),
    ] {
        let ring = Ring::new(setup(Some(chosen))).expect("an instance").0;
        assert!(
            !EtwDiagnostics::provider_is_registered_in_process(),
            "registered by an instance with a sink of its own"
        );
        drop(ring);
    }

    // None given: the default, which is the ETW sink, registers it, and gives it back with the instance.
    let ring = Ring::new(setup(None)).expect("an instance").0;
    assert!(
        EtwDiagnostics::provider_is_registered_in_process(),
        "the default did not register the provider"
    );
    drop(ring);
    assert!(
        !EtwDiagnostics::provider_is_registered_in_process(),
        "the registration outlived the instance"
    );

    // The sink itself, chosen: the same provider, and the same registration.
    let ring = Ring::new(setup(Some(Arc::new(EtwDiagnostics::new()))))
        .expect("an instance")
        .0;
    assert!(EtwDiagnostics::provider_is_registered_in_process());
    drop(ring);
    assert!(!EtwDiagnostics::provider_is_registered_in_process());
}
