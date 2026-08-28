// Copyright (c) 2026 Mike Grier
//! Tests for one enumeration's quantum, against real directories.

use super::*;
use crate::buffer::NativeBuffer;
use crate::entry::FileIdentityMode;
use crate::request::MINIMUM_BUFFER_CAPACITY;
use crate::scratch::Scratch;

fn engine_for(path: &std::path::Path, mode: FileIdentityMode) -> EngineState {
    let request = EnumerationRequest::for_path(path)
        .expect("a resolvable path")
        .with_file_identity(mode)
        .with_buffer_capacity(MINIMUM_BUFFER_CAPACITY)
        .expect("representable");
    let token = ImpersonationToken::capture().expect("the calling thread has a context");
    let buffer = NativeBuffer::try_new(request.buffer_capacity()).expect("allocation");
    EngineState::new(request, token, buffer)
}

fn terminal(outcome: QuantumOutcome) -> TerminalOutcome {
    match outcome {
        QuantumOutcome::Finished(outcome) => outcome,
        other => panic!("expected the enumeration to finish, got {other:?}"),
    }
}

/// Run quanta until the enumeration finishes, or give up.
///
/// An empty directory takes two: one for its . and .. batch, and one to
/// reach exhaustion.
fn run_to_completion(engine: &mut EngineState) -> TerminalOutcome {
    for _ in 0..64 {
        if let QuantumOutcome::Finished(outcome) = advance(engine) {
            return outcome;
        }
    }
    panic!("the enumeration never finished");
}

#[test]
fn an_empty_directory_completes() {
    let scratch = Scratch::empty();
    let mut engine = engine_for(scratch.path(), FileIdentityMode::Omit);
    let outcome = run_to_completion(&mut engine);
    assert!(outcome.is_completed(), "{outcome:?}");
}

#[test]
fn a_missing_directory_fails_to_open() {
    let scratch = Scratch::empty();
    let mut engine = engine_for(&scratch.child("absent"), FileIdentityMode::Omit);
    let outcome = terminal(advance(&mut engine));
    let failure = outcome.failure().expect("a failure");
    assert!(
        matches!(failure, EnumerationError::DirectoryOpen(_)),
        "{failure:?}"
    );
}

#[test]
fn a_file_is_rejected_rather_than_enumerated() {
    let scratch = Scratch::with_files(&["plain.txt"]);
    let mut engine = engine_for(&scratch.child("plain.txt"), FileIdentityMode::Omit);
    let outcome = terminal(advance(&mut engine));
    let failure = outcome.failure().expect("a failure");
    assert!(
        matches!(failure, EnumerationError::DirectoryOpen(_)),
        "a file is an open failure, not a capability failure: {failure:?}"
    );
}

#[test]
fn a_batch_yields_rather_than_refilling_twice() {
    // One refill per quantum, then a scheduling point. FE-9 is what turns the
    // batch into entries; until then it is read and passed over, so the
    // enumeration still reaches its true end.
    let scratch = Scratch::with_files(&["a.txt"]);
    let mut engine = engine_for(scratch.path(), FileIdentityMode::Omit);
    let outcome = advance(&mut engine);
    assert!(matches!(outcome, QuantumOutcome::Yielded), "{outcome:?}");
}

#[test]
fn a_directory_with_entries_still_reaches_exhaustion() {
    let scratch = Scratch::with_files(&["a.txt", "b.txt", "c.txt"]);
    let mut engine = engine_for(scratch.path(), FileIdentityMode::Omit);
    let outcome = run_to_completion(&mut engine);
    assert!(outcome.is_completed(), "{outcome:?}");
}

#[test]
fn omitting_identity_asks_the_volume_nothing() {
    let scratch = Scratch::empty();
    let mut engine = engine_for(scratch.path(), FileIdentityMode::Omit);
    let _ = run_to_completion(&mut engine);
    assert_eq!(engine.volume_serial(), None);
}

#[test]
fn a_best_effort_identity_obtains_the_volume_serial_on_a_local_disk() {
    let scratch = Scratch::empty();
    let mut engine = engine_for(scratch.path(), FileIdentityMode::BestEffort);
    let outcome = run_to_completion(&mut engine);
    assert!(outcome.is_completed(), "{outcome:?}");
    assert!(
        engine.volume_serial().is_some(),
        "a local volume reports its serial"
    );
}

#[test]
fn a_required_identity_also_completes_when_the_volume_answers() {
    let scratch = Scratch::empty();
    let mut engine = engine_for(scratch.path(), FileIdentityMode::Required);
    let outcome = run_to_completion(&mut engine);
    assert!(outcome.is_completed(), "{outcome:?}");
    assert!(engine.volume_serial().is_some());
}

#[test]
fn the_open_happens_once_across_quanta() {
    // The second quantum continues the same handle rather than reopening, which
    // is what makes the record cursor meaningful.
    let scratch = Scratch::with_files(&["a.txt"]);
    let mut engine = engine_for(scratch.path(), FileIdentityMode::Omit);
    assert!(matches!(advance(&mut engine), QuantumOutcome::Yielded));

    // Now past the first query, so the next refill uses the continuation class
    // and reaches exhaustion.
    let outcome = terminal(advance(&mut engine));
    assert!(outcome.is_completed(), "{outcome:?}");
}

#[test]
fn a_failed_open_is_not_retried_as_an_empty_directory() {
    // A missing directory must never look like exhaustion, however many quanta
    // it is given.
    let scratch = Scratch::empty();
    let mut engine = engine_for(&scratch.child("absent"), FileIdentityMode::Omit);
    for _ in 0..3 {
        let outcome = terminal(advance(&mut engine));
        assert!(outcome.failure().is_some(), "{outcome:?}");
    }
}

#[test]
fn debug_output_names_the_phase() {
    let scratch = Scratch::empty();
    let engine = engine_for(scratch.path(), FileIdentityMode::Omit);
    let rendered = format!("{engine:?}");
    assert!(rendered.contains("Unopened"), "{rendered}");
}
