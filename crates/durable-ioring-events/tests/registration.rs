// Copyright (c) 2026 Mike Grier
//! The registration's lifetime (DE-D-4): the provider is registered while any sink is alive, and given back when the last is
//! dropped, so that a library that is unloaded does not leave a registration behind.
//!
//! # Why this is an integration test
//!
//! It asks whether the registration is gone, which any other sink alive in the same process would keep, so it needs a process
//! of its own, which an integration test file is.

#![cfg(windows)]

use durable_ioring_events::EtwDiagnostics;

#[test]
fn the_registration_is_given_back_when_the_last_sink_is_dropped_and_made_again_for_the_next() {
    assert!(
        !EtwDiagnostics::provider_is_registered_in_process(),
        "before any sink"
    );
    let first = EtwDiagnostics::new();
    let second = first.clone();
    assert!(first.is_registered());
    assert!(EtwDiagnostics::provider_is_registered_in_process());
    drop(first);
    assert!(
        EtwDiagnostics::provider_is_registered_in_process(),
        "given back while a sink was alive"
    );
    drop(second);
    assert!(
        !EtwDiagnostics::provider_is_registered_in_process(),
        "kept after every sink was dropped"
    );

    let next = EtwDiagnostics::new();
    assert!(next.is_registered());
    assert!(EtwDiagnostics::provider_is_registered_in_process());
}
