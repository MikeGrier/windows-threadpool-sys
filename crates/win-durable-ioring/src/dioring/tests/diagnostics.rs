// Copyright (c) 2026 Mike Grier
//! The driver's diagnostic sink (DI-12.1.6, DI-D-51): a sink given at construction replaces the default, and an instance given
//! none writes its events to ETW. The registration the default makes is `tests/default_diagnostics.rs`, which needs a process of
//! its own.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use durable_ioring::oracle::PairingChecker;
use durable_ioring::{Diagnostic, DiagnosticKind, DiagnosticSink};
use durable_ioring_events::{EtwDiagnostics, PROVIDER_ID, id};
use win_etw_sys::{Session, level};

use super::pushes::{GIVEN, Harness, Ring, given, instance, temp};
use super::setup;
use crate::dioring::Setup;

/// A sink that keeps each event as text, wants every kind, and holds the stream to the pairing rule
/// (DI-D-87), which it takes from the core's checker and does not restate.
#[derive(Default)]
struct Recording(Mutex<Vec<String>>, Mutex<PairingChecker>);

impl Recording {
    /// Every pair the instance began has ended.
    fn finished(&self) {
        let checker = std::mem::take(&mut *self.1.lock().expect("the checker"));
        checker.finish(&[]).expect("every pair ended");
    }
}

impl DiagnosticSink for Recording {
    fn wants(&self, _: DiagnosticKind) -> bool {
        true
    }

    fn report(&self, event: &Diagnostic<'_>) {
        self.1
            .lock()
            .expect("the checker")
            .observe(event)
            .expect("the pairing rule");
        self.0.lock().expect("the record").push(event.to_string());
    }
}

/// Writes to the file and seals through epoch 1, and waits for it to be durable.
fn write_and_seal(harness: &mut Harness) {
    harness.write(GIVEN, 0, b"first", 1, 1);
    harness.next();
    harness.seal(1);
    match harness.next_entry() {
        durable_ioring::Entry::Durable { .. } => {}
        other => panic!("expected Durable, got {other:?}"),
    }
}

#[test]
fn a_chosen_sink_receives_the_instances_events_in_place_of_the_default() {
    let temp = temp(&[0; 64]);
    let sink = Arc::new(Recording::default());
    let ring = Ring::new(Setup {
        diagnostics: Some(sink.clone()),
        ..setup(vec![given(GIVEN, &temp)], Vec::new(), None)
    })
    .expect("build an instance")
    .0;
    let mut harness = Harness::new(ring);
    write_and_seal(&mut harness);
    harness.finish();
    sink.finished();

    let lines = sink.0.lock().expect("the record").clone();
    // Pair ids are minted per process, so each event is found by its shape around one.
    let found = |before: &str, after: &str| {
        lines
            .iter()
            .any(|line| line.contains(before) && line.ends_with(after))
    };
    assert!(found("seal through 1 (pair ", "): began"), "{lines:?}");
    assert!(found("flush of file 2 (pair ", ") began"), "{lines:?}");
    assert!(found("flush of file 2 (pair ", ") succeeded"), "{lines:?}");
    assert!(found("seal through 1 (pair ", "): durable"), "{lines:?}");
    let me = instance_named_by(&lines);
    assert!(
        lines
            .iter()
            .all(|line| line.starts_with(&format!("instance {me} "))),
        "{lines:?}"
    );
}

/// The instance number every line of an instance's events names.
fn instance_named_by(lines: &[String]) -> u64 {
    lines[0]
        .split_whitespace()
        .nth(1)
        .expect("a number after `instance`")
        .parse()
        .expect("a number")
}

#[test]
fn a_chosen_sink_that_wants_nothing_is_not_asked_to_report() {
    struct Nothing(Mutex<u32>);
    impl DiagnosticSink for Nothing {
        fn wants(&self, _: DiagnosticKind) -> bool {
            false
        }
        fn report(&self, _: &Diagnostic<'_>) {
            *self.0.lock().expect("the count") += 1;
        }
    }
    let temp = temp(&[0; 64]);
    let sink = Arc::new(Nothing(Mutex::new(0)));
    let ring = Ring::new(Setup {
        diagnostics: Some(sink.clone()),
        ..setup(vec![given(GIVEN, &temp)], Vec::new(), None)
    })
    .expect("build an instance")
    .0;
    let mut harness = Harness::new(ring);
    write_and_seal(&mut harness);
    harness.finish();
    assert_eq!(*sink.0.lock().expect("the count"), 0);
}

/// Whether `condition` becomes true within a bound: a provider registered before a session enabled it is told so afterwards.
fn becomes(condition: impl Fn() -> bool) -> bool {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(10) {
        if condition() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    false
}

#[test]
#[ignore = "starts a trace session, which needs an elevated process or the Performance Log Users group: run with --include-ignored, as CI does"]
fn an_instance_with_no_chosen_sink_writes_its_events_to_etw() {
    let mut session = Session::start().expect("a trace session (this test needs privilege)");
    session
        .enable(PROVIDER_ID, level::VERBOSE, u64::MAX)
        .expect("enabled");
    // Other tests of this process share the provider's registration, which may have been made before the session was
    // enabled, so the session's arrival is waited for.
    let probe = EtwDiagnostics::new();
    assert!(becomes(|| probe.wants(DiagnosticKind::Flush)));

    let temp = temp(&[0; 64]);
    let mut harness = Harness::new(instance(vec![given(GIVEN, &temp)], Vec::new()));
    let me = harness.lineage().instance.number();
    write_and_seal(&mut harness);
    harness.finish();
    let recorded = session.stop().expect("stopped and read");

    // A session records every instance of the process, so this one's events are those that name it first.
    let ours: Vec<u16> = recorded
        .iter()
        .filter(|event| u64::from_ne_bytes(event.data[..8].try_into().expect("an instance")) == me)
        .map(|event| event.descriptor.id)
        .collect();
    for event in [
        id::SEAL_BEGAN,
        id::FLUSH_BEGAN_FILE,
        id::FLUSH_ENDED_FILE,
        id::SEAL_ENDED,
    ] {
        assert!(ours.contains(&event), "{event} in {ours:?}");
    }
}
