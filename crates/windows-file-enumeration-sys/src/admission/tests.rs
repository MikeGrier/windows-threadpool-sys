// Copyright (c) 2026 Mike Grier
//! Tests for admission and the affine enumeration handle.

use super::*;
use crate::completion::{Completion, TerminalOutcome};
use crate::error::BeginFailure;
use crate::session::{
    MINIMUM_COMPLETION_RING_CAPACITY, MINIMUM_SUBMISSION_CAPACITY, Receiver, Session,
};
use std::time::Duration;
use wtf_string::Wtf16String;

fn request() -> EnumerationRequest {
    EnumerationRequest::new(&Wtf16String::from(r"C:\Windows")).expect("a resolvable path")
}

/// A session whose pool objects never fire, so a test that drives servicing
/// explicitly is not racing the thread pool for the same work.
fn session() -> (Session, Receiver) {
    session_with(8, 8)
}

/// A suppressed session with explicit bounds, for the tests whose subject is
/// how the rings account for reservations.
fn session_with(submission: usize, completion: usize) -> (Session, Receiver) {
    let (session, receiver) = Session::new(submission, completion).expect("valid bounds");
    session.suppress_pool();
    (session, receiver)
}

/// A session that really does use the thread pool, for the tests whose subject
/// is that the pool eventually runs.
fn live_session() -> (Session, Receiver) {
    Session::new(8, 8).expect("a session with room")
}

/// Drain the submission ring on this thread, so a test observes the servicer's
/// effects without racing the thread pool.
fn service(session: &Session) {
    session.shared.drain_submissions();
}

#[test]
fn an_admitted_enumeration_gets_a_handle_naming_it() {
    let (session, _receiver) = session();
    let handle = session.try_begin(request()).expect("room");
    let id = handle.id();
    service(&session);
    assert!(session.shared.contains(id));
    handle.detach();
}

#[test]
fn each_admission_gets_its_own_identifier() {
    let (session, _receiver) = session();
    let first = session.try_begin(request()).expect("room");
    let second = session.try_begin(request()).expect("room");
    assert_ne!(first.id(), second.id());
    first.detach();
    second.detach();
}

#[test]
fn admission_registers_the_request_it_was_given() {
    let (session, _receiver) = session();
    let handle = session.try_begin(request()).expect("room");
    service(&session);
    assert_eq!(session.enumerations(), 1);
    handle.detach();
}

#[test]
fn an_explicit_token_is_used_instead_of_capturing_one() {
    // The traversal shape: capture once, reuse for every directory.
    let (session, _receiver) = session();
    let token = ImpersonationToken::capture().expect("a context");
    let first = session
        .try_begin_with_token(request(), token.clone())
        .expect("room");
    let second = session
        .try_begin_with_token(request(), token)
        .expect("room");
    service(&session);
    assert_eq!(session.enumerations(), 2);
    first.detach();
    second.detach();
}

#[test]
fn a_full_submission_ring_refuses_a_begin_and_returns_the_request() {
    // Three slots: abandon, this enumeration's cancel reservation, and its
    // begin message. A second begin has nowhere to go until the first is
    // serviced.
    let (session, _receiver) = session_with(MINIMUM_SUBMISSION_CAPACITY, 8);
    let first = session.try_begin(request()).expect("the only room");

    let error = session.try_begin(request()).expect_err("no room");
    assert_eq!(error.failure(), BeginFailure::SubmissionRingFull);
    let (returned, token) = error.into_parts();
    assert_eq!(returned.path().to_string_lossy(), r"C:\Windows");
    assert!(token.is_some(), "the captured context comes back too");

    first.detach();
}

#[test]
fn a_refused_begin_leaves_no_reservation_behind() {
    // The smallest ring accounts for exactly one live enumeration: the standing
    // abandon slot, that enumeration's cancellation and retirement
    // reservations, and one transient begin message.
    let (session, _receiver) = session_with(MINIMUM_SUBMISSION_CAPACITY, 8);
    let first = session.try_begin(request()).expect("room");
    session.try_begin(request()).expect_err("no room");
    service(&session);

    // Cancelling returns everything the first enumeration held. If the refused
    // attempt had kept anything, this second begin would not fit.
    first.cancel();
    service(&session);
    let second = session.try_begin(request()).expect("room again");
    second.detach();
}

#[test]
fn the_smallest_submission_ring_carries_one_live_enumeration_at_a_time() {
    // Each live enumeration holds a cancellation reservation for as long as its
    // handle can cancel it, so the bound is on *outstanding* work, not on the
    // total a session may ever start.
    let (session, _receiver) = session_with(MINIMUM_SUBMISSION_CAPACITY, 8);
    for _ in 0..3 {
        let handle = session.try_begin(request()).expect("room for one");
        session
            .try_begin(request())
            .expect_err("only one at a time");
        service(&session);
        handle.cancel();
        service(&session);
    }
}

#[test]
fn a_completion_ring_that_cannot_reserve_a_terminal_refuses_the_begin() {
    // The smallest completion ring accounts for exactly one enumeration.
    let (session, _receiver) = session_with(8, MINIMUM_COMPLETION_RING_CAPACITY);
    let first = session
        .try_begin(request())
        .expect("the only terminal slot");
    let error = session.try_begin(request()).expect_err("no terminal slot");
    assert_eq!(error.failure(), BeginFailure::CompletionRingFull);
    first.detach();
}

#[test]
fn a_begin_refused_by_the_completion_ring_returns_its_submission_reservation() {
    let (session, _receiver) = session_with(8, MINIMUM_COMPLETION_RING_CAPACITY);
    let first = session.try_begin(request()).expect("room");
    session.try_begin(request()).expect_err("no terminal slot");

    // Nothing was accepted, so the submission ring is back to one begin plus
    // one cancellation reservation plus the standing abandon slot.
    assert_eq!(session.shared.submissions.len(), 1);
    first.cancel();
    service(&session);
}

#[test]
fn an_abandoned_session_refuses_further_begins() {
    let (session, receiver) = session();
    drop(receiver);
    let error = session.try_begin(request()).expect_err("abandoned");
    assert_eq!(error.failure(), BeginFailure::Abandoned);
}

#[test]
fn dropping_the_handle_cancels_its_enumeration() {
    let (session, receiver) = session();
    let handle = session.try_begin(request()).expect("room");
    let id = handle.id();
    service(&session);

    drop(handle);
    service(&session);

    let record = receiver.try_recv().expect("a terminal");
    match record {
        Completion::Terminal {
            enumeration,
            outcome,
        } => {
            assert_eq!(enumeration, id);
            assert!(matches!(outcome, TerminalOutcome::Cancelled));
        }
        Completion::Entry { .. } => panic!("expected a terminal"),
    }
}

#[test]
fn cancelling_explicitly_is_the_same_as_dropping() {
    let (session, receiver) = session();
    let handle = session.try_begin(request()).expect("room");
    let id = handle.id();
    service(&session);

    handle.cancel();
    service(&session);

    let record = receiver.try_recv().expect("a terminal");
    assert_eq!(record.enumeration(), id);
    assert!(record.is_terminal());
    assert!(receiver.try_recv().is_none(), "exactly one terminal");
}

#[test]
fn a_detached_enumeration_is_not_cancelled() {
    let (session, receiver) = session();
    let handle = session.try_begin(request()).expect("room");
    let id = handle.id();
    service(&session);

    handle.detach();
    service(&session);

    assert!(session.shared.contains(id), "still registered");
    assert!(receiver.try_recv().is_none(), "no terminal was produced");
}

#[test]
fn detaching_returns_the_cancellation_reservation() {
    let (session, _receiver) = session_with(MINIMUM_SUBMISSION_CAPACITY, 8);
    let handle = session.try_begin(request()).expect("room");
    service(&session);
    // Abandon, cancel, and retire are outstanding once the begin is serviced.
    assert_eq!(session.shared.submissions.reserved(), 3);

    // Detaching gives back exactly the cancellation slot. The retirement slot
    // stays: the worker still has to be able to report itself finished.
    handle.detach();
    assert_eq!(session.shared.submissions.reserved(), 2);
}

#[test]
fn an_enumeration_reserves_both_of_its_control_messages() {
    let (session, _receiver) = session_with(16, 16);
    let before = session.shared.submissions.reserved();
    let handle = session.try_begin(request()).expect("room");
    assert_eq!(
        session.shared.submissions.reserved(),
        before + 2,
        "one for cancellation, one for retirement"
    );
    handle.detach();
}

#[test]
fn a_handle_cancels_at_most_once() {
    // `cancel` consumes the handle, so `Drop` cannot spend the slot again --
    // the reservation is accounted for exactly once either way.
    let (session, receiver) = session();
    let handle = session.try_begin(request()).expect("room");
    service(&session);
    handle.cancel();
    service(&session);
    assert!(receiver.try_recv().is_some());
    assert!(receiver.try_recv().is_none());
}

#[test]
fn cancelling_after_abandonment_produces_no_terminal() {
    let (session, receiver) = session();
    let handle = session.try_begin(request()).expect("room");
    service(&session);

    drop(receiver);
    drop(handle);
    service(&session);

    assert_eq!(session.enumerations(), 0);
}

#[test]
fn abandonment_releases_enumerations_without_terminals() {
    let (session, receiver) = session();
    let first = session.try_begin(request()).expect("room");
    let second = session.try_begin(request()).expect("room");
    service(&session);
    assert_eq!(session.enumerations(), 2);

    first.detach();
    second.detach();
    drop(receiver);
    service(&session);

    assert_eq!(session.enumerations(), 0);
    assert!(session.is_abandoned());
}

#[test]
fn abandonment_reaches_the_servicer_through_the_thread_pool() {
    // Receiver drop must not block on the teardown it starts, so the work is
    // done by the pool; the session observes it shortly afterwards.
    let (session, receiver) = live_session();
    let handle = session.try_begin(request()).expect("room");
    handle.detach();
    drop(receiver);

    for _ in 0..1000 {
        if session.is_abandoned() && session.enumerations() == 0 {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("the servicer never applied abandonment");
}

#[test]
fn admission_reaches_the_servicer_through_the_thread_pool() {
    // Registration is now transient -- the enumeration runs and retires on its
    // own -- so the stable observation is that its terminal arrives.
    let scratch = crate::scratch::Scratch::empty();
    let (session, receiver) = live_session();
    let request = EnumerationRequest::for_path(scratch.path()).expect("resolvable");
    let handle = session.try_begin(request).expect("room");
    let id = handle.id();
    handle.detach();

    for _ in 0..2000 {
        while let Some(record) = receiver.try_recv() {
            if record.is_terminal() {
                assert_eq!(record.enumeration(), id);
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("the enumeration never reached the servicer");
}

#[test]
fn several_session_clones_may_admit_concurrently() {
    let (session, _receiver) = session_with(64, 64);
    let mut handles = Vec::new();
    let mut threads = Vec::new();
    for _ in 0..4 {
        let producer = session.clone();
        threads.push(std::thread::spawn(move || {
            let handle = producer.try_begin(request()).expect("room");
            let id = handle.id();
            handle.detach();
            id
        }));
    }
    for thread in threads {
        handles.push(thread.join().expect("producer"));
    }
    handles.sort_unstable();
    handles.dedup();
    assert_eq!(handles.len(), 4, "identifiers are unique across threads");
}

#[test]
fn a_handle_describes_whether_it_can_still_cancel() {
    let (session, _receiver) = session();
    let handle = session.try_begin(request()).expect("room");
    let rendered = format!("{handle:?}");
    assert!(rendered.contains("cancellable: true"), "{rendered}");
    handle.detach();
}
