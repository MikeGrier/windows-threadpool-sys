// Copyright (c) 2026 Mike Grier
//! Tests for one enumeration's quantum, against real directories.

use super::*;
use crate::buffer::NativeBuffer;
use crate::completion::{Completion, EnumerationId};
use crate::completion_ring::{CompletionRing, MINIMUM_COMPLETION_CAPACITY};
use crate::entry::FileIdentityMode;
use crate::pattern::{CaseSensitivity, NamePattern};
use crate::predicate::{PredicateClause, QueryByExample};
use crate::request::MINIMUM_BUFFER_CAPACITY;
use crate::scratch::Scratch;
use wtf_string::Wtf16Str;

/// The single enumeration identity these tests need: nothing here shares a
/// ring with a second enumeration, so one arbitrary value names it throughout.
const ENUMERATION: EnumerationId = EnumerationId::from_raw(0);

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

fn engine_with_predicate(path: &std::path::Path, predicate: QueryByExample) -> EngineState {
    let request = EnumerationRequest::for_path(path)
        .expect("a resolvable path")
        .with_predicate(predicate)
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
fn run_to_completion(engine: &mut EngineState, completions: &CompletionRing) -> TerminalOutcome {
    for _ in 0..64 {
        if let QuantumOutcome::Finished(outcome) = advance(engine, ENUMERATION, completions) {
            return outcome;
        }
    }
    panic!("the enumeration never finished");
}

/// Drain every entry currently queued, as plain names, for an order-insensitive
/// comparison.
fn drain_names(completions: &CompletionRing) -> Vec<String> {
    let mut names = Vec::new();
    while let Some(Completion::Entry { entry, .. }) = completions.try_take() {
        names.push(entry.name().to_string_lossy());
    }
    names
}

#[test]
fn an_empty_directory_completes() {
    let scratch = Scratch::empty();
    let mut engine = engine_for(scratch.path(), FileIdentityMode::Omit);
    let completions = CompletionRing::new(8);
    let outcome = run_to_completion(&mut engine, &completions);
    assert!(outcome.is_completed(), "{outcome:?}");
    assert!(
        completions.try_take().is_none(),
        "no entries in an empty directory"
    );
}

#[test]
fn a_missing_directory_fails_to_open() {
    let scratch = Scratch::empty();
    let mut engine = engine_for(&scratch.child("absent"), FileIdentityMode::Omit);
    let completions = CompletionRing::new(8);
    let outcome = terminal(advance(&mut engine, ENUMERATION, &completions));
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
    let completions = CompletionRing::new(8);
    let outcome = terminal(advance(&mut engine, ENUMERATION, &completions));
    let failure = outcome.failure().expect("a failure");
    assert!(
        matches!(failure, EnumerationError::DirectoryOpen(_)),
        "a file is an open failure, not a capability failure: {failure:?}"
    );
}

#[test]
fn a_batch_yields_rather_than_refilling_twice() {
    // One refill per quantum, then a scheduling point -- even though this
    // quantum also parses and delivers everything the refill just loaded.
    let scratch = Scratch::with_files(&["a.txt"]);
    let mut engine = engine_for(scratch.path(), FileIdentityMode::Omit);
    let completions = CompletionRing::new(8);
    let outcome = advance(&mut engine, ENUMERATION, &completions);
    assert!(matches!(outcome, QuantumOutcome::Yielded), "{outcome:?}");
    assert_eq!(drain_names(&completions), ["a.txt"]);
}

#[test]
fn a_directory_with_entries_still_reaches_exhaustion() {
    let scratch = Scratch::with_files(&["a.txt", "b.txt", "c.txt"]);
    let mut engine = engine_for(scratch.path(), FileIdentityMode::Omit);
    let completions = CompletionRing::new(8);
    let outcome = run_to_completion(&mut engine, &completions);
    assert!(outcome.is_completed(), "{outcome:?}");

    let mut names = drain_names(&completions);
    names.sort();
    assert_eq!(names, ["a.txt", "b.txt", "c.txt"]);
}

#[test]
fn dot_and_dotdot_are_never_delivered() {
    let scratch = Scratch::empty();
    let mut engine = engine_for(scratch.path(), FileIdentityMode::Omit);
    let completions = CompletionRing::new(8);
    let outcome = run_to_completion(&mut engine, &completions);
    assert!(outcome.is_completed(), "{outcome:?}");
    assert!(
        completions.try_take().is_none(),
        "an empty directory delivers nothing, not '.' and '..'"
    );
}

#[test]
fn a_predicate_can_reject_every_entry_without_stopping_the_enumeration() {
    let scratch = Scratch::with_files(&["a.txt", "b.txt"]);
    let name = "nothing-matches-this".encode_utf16().collect::<Vec<_>>();
    let pattern = NamePattern::literal(Wtf16Str::from_units(&name));
    let predicate = QueryByExample::new()
        .with(PredicateClause::Name {
            pattern,
            case: CaseSensitivity::Insensitive,
            negated: false,
        })
        .expect("a non-vacuous clause");
    let mut engine = engine_with_predicate(scratch.path(), predicate);
    let completions = CompletionRing::new(8);
    let outcome = run_to_completion(&mut engine, &completions);
    assert!(outcome.is_completed(), "{outcome:?}");
    assert!(
        completions.try_take().is_none(),
        "a reject-all predicate delivers nothing, but still reaches its end"
    );
}

#[test]
fn a_full_completion_ring_parks_and_retains_the_unread_record() {
    let scratch = Scratch::with_files(&["a.txt", "b.txt", "c.txt"]);
    let mut engine = engine_for(scratch.path(), FileIdentityMode::Omit);
    let completions = CompletionRing::new(MINIMUM_COMPLETION_CAPACITY);

    let outcome = advance(&mut engine, ENUMERATION, &completions);
    assert!(matches!(outcome, QuantumOutcome::Parked), "{outcome:?}");
    assert_eq!(
        completions.len(),
        MINIMUM_COMPLETION_CAPACITY,
        "the ring is full"
    );
    let mut names = drain_names(&completions);
    assert_eq!(names.len(), MINIMUM_COMPLETION_CAPACITY);

    // Draining is what makes room; the parked record was never lost, so it is
    // delivered on the very next quantum rather than a refill re-reading
    // records this one already handled.
    let outcome = advance(&mut engine, ENUMERATION, &completions);
    assert!(matches!(outcome, QuantumOutcome::Yielded), "{outcome:?}");

    names.extend(drain_names(&completions));
    names.sort();
    assert_eq!(names, ["a.txt", "b.txt", "c.txt"]);
}

#[test]
fn omitting_identity_asks_the_volume_nothing() {
    let scratch = Scratch::empty();
    let mut engine = engine_for(scratch.path(), FileIdentityMode::Omit);
    let completions = CompletionRing::new(8);
    let _ = run_to_completion(&mut engine, &completions);
    assert_eq!(engine.volume_serial(), None);
}

#[test]
fn a_best_effort_identity_obtains_the_volume_serial_on_a_local_disk() {
    let scratch = Scratch::with_files(&["a.txt"]);
    let mut engine = engine_for(scratch.path(), FileIdentityMode::BestEffort);
    let completions = CompletionRing::new(8);
    let outcome = run_to_completion(&mut engine, &completions);
    assert!(outcome.is_completed(), "{outcome:?}");
    assert!(
        engine.volume_serial().is_some(),
        "a local volume reports its serial"
    );

    let Some(Completion::Entry { entry, .. }) = completions.try_take() else {
        panic!("a.txt was never delivered");
    };
    assert_eq!(entry.identity().volume_serial(), engine.volume_serial());
    assert!(entry.identity().is_volume_qualified());
}

#[test]
fn a_required_identity_also_completes_when_the_volume_answers() {
    let scratch = Scratch::empty();
    let mut engine = engine_for(scratch.path(), FileIdentityMode::Required);
    let completions = CompletionRing::new(8);
    let outcome = run_to_completion(&mut engine, &completions);
    assert!(outcome.is_completed(), "{outcome:?}");
    assert!(engine.volume_serial().is_some());
}

#[test]
fn the_open_happens_once_across_quanta() {
    // The second quantum continues the same handle rather than reopening, which
    // is what makes the record cursor meaningful.
    let scratch = Scratch::with_files(&["a.txt"]);
    let mut engine = engine_for(scratch.path(), FileIdentityMode::Omit);
    let completions = CompletionRing::new(8);
    assert!(matches!(
        advance(&mut engine, ENUMERATION, &completions),
        QuantumOutcome::Yielded
    ));

    // Now past the first query, so the next refill uses the continuation class
    // and reaches exhaustion.
    let outcome = terminal(advance(&mut engine, ENUMERATION, &completions));
    assert!(outcome.is_completed(), "{outcome:?}");
}

#[test]
fn a_failed_open_is_not_retried_as_an_empty_directory() {
    // A missing directory must never look like exhaustion, however many quanta
    // it is given.
    let scratch = Scratch::empty();
    let mut engine = engine_for(&scratch.child("absent"), FileIdentityMode::Omit);
    let completions = CompletionRing::new(8);
    for _ in 0..3 {
        let outcome = terminal(advance(&mut engine, ENUMERATION, &completions));
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
