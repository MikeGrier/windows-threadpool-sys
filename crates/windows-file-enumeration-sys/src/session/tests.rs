// Copyright (c) 2026 Mike Grier
//! Tests for the session shell: construction, servicing, and the receiver.

use super::*;
use crate::completion::TerminalOutcome;
use crate::error::SessionFailure;
use crate::testing::named_file;
use wtf_string::Wtf16String;

/// A session whose pool objects never fire, so a test that drives servicing
/// explicitly is not racing the thread pool for the same work.
fn session() -> (Session, Receiver) {
    let (session, receiver) = Session::new(8, 8).expect("a session with room");
    session.suppress_pool();
    (session, receiver)
}

/// A session that really does use the thread pool, for the tests whose subject
/// is that the pool eventually runs.
fn live_session() -> (Session, Receiver) {
    Session::new(8, 8).expect("a session with room")
}

/// A suppressed session with explicit bounds.
fn session_with(submission: usize, completion: usize) -> (Session, Receiver) {
    let (session, receiver) = Session::new(submission, completion).expect("valid bounds");
    session.suppress_pool();
    (session, receiver)
}

fn request() -> EnumerationRequest {
    EnumerationRequest::new(&Wtf16String::from(r"C:\Windows")).expect("a resolvable path")
}

/// Admit one enumeration and let it run: the tests here are about the shell,
/// not about cancellation.
fn admit(session: &Session) -> EnumerationId {
    let handle = session.try_begin(request()).expect("room");
    let enumeration = handle.id();
    handle.detach();
    enumeration
}

/// Drain the submission ring on this thread, so a test observes the servicer's
/// effects without racing the thread pool.
fn service(session: &Session) {
    session.shared.drain_submissions();
}

#[test]
fn a_session_reports_the_bounds_it_was_built_with() {
    let (session, receiver) = Session::new(5, 6).expect("valid bounds");
    assert_eq!(session.submission_capacity(), 5);
    assert_eq!(session.completion_capacity(), 6);
    assert_eq!(receiver.capacity(), 6);
    assert_eq!(session.enumerations(), 0);
}

#[test]
fn a_submission_ring_too_small_for_one_enumeration_is_rejected() {
    // Three slots are load-bearing: abandon, cancel, and one begin.
    for capacity in 0..MINIMUM_SUBMISSION_CAPACITY {
        let error = Session::new(capacity, 8).expect_err("too small");
        assert_eq!(error.failure(), SessionFailure::SubmissionCapacityTooSmall);
    }
    Session::new(MINIMUM_SUBMISSION_CAPACITY, 8).expect("the smallest usable ring");
}

#[test]
fn a_completion_ring_too_small_to_keep_one_data_slot_is_rejected() {
    // A ring of one could hold a reserved terminal or an entry, never both.
    let error = Session::new(8, 1).expect_err("too small");
    assert_eq!(error.failure(), SessionFailure::CompletionCapacityTooSmall);
    Session::new(8, MINIMUM_COMPLETION_RING_CAPACITY).expect("the smallest usable ring");
}

#[test]
fn identifiers_are_unique_and_monotonic_within_a_session() {
    let (session, _receiver) = session();
    let first = session.shared.next_enumeration_id();
    let second = session.shared.next_enumeration_id();
    assert!(first < second);
    assert_ne!(first, second);
}

#[test]
fn a_fresh_session_is_not_abandoned() {
    let (session, _receiver) = session();
    assert!(!session.is_abandoned());
}

#[test]
fn an_empty_receiver_has_nothing_to_take() {
    let (_session, receiver) = session();
    assert!(receiver.is_empty());
    assert_eq!(receiver.len(), 0);
    assert!(receiver.try_recv().is_none());
    assert!(!receiver.is_disconnected());
}

#[test]
fn the_receiver_observes_what_the_ring_was_given() {
    let (session, receiver) = session();
    session
        .shared
        .completions
        .try_send_entry(Completion::Entry {
            enumeration: EnumerationId::from_raw(1),
            entry: named_file("a.txt"),
        })
        .expect("room");

    assert_eq!(receiver.len(), 1);
    let record = receiver.try_recv().expect("a record");
    assert_eq!(record.enumeration(), EnumerationId::from_raw(1));
    assert!(receiver.try_recv().is_none());
}

#[test]
fn entries_reach_the_receiver_in_the_order_they_were_produced() {
    let (session, receiver) = session();
    for name in ["a", "b", "c"] {
        session
            .shared
            .completions
            .try_send_entry(Completion::Entry {
                enumeration: EnumerationId::from_raw(1),
                entry: named_file(name),
            })
            .expect("room");
    }
    for name in ["a", "b", "c"] {
        match receiver.try_recv().expect("a record") {
            Completion::Entry { entry, .. } => assert_eq!(entry.name().to_string_lossy(), name),
            Completion::Terminal { .. } => panic!("expected an entry"),
        }
    }
}

#[test]
fn dropping_every_session_handle_ends_the_stream() {
    let (session, receiver) = session();
    let clone = session.clone();
    drop(session);
    assert!(!receiver.is_disconnected(), "a clone still produces");
    drop(clone);
    assert!(receiver.is_disconnected());
    assert!(receiver.recv().is_none());
}

#[test]
fn an_outstanding_enumeration_keeps_the_stream_open_past_the_last_handle() {
    let (session, receiver) = session();
    let slot = session
        .shared
        .completions
        .reserve_terminal(EnumerationId::from_raw(1))
        .expect("room");
    drop(session);
    assert!(!receiver.is_disconnected(), "an outcome is still owed");
    slot.send(TerminalOutcome::Completed);
    assert!(receiver.is_disconnected());
    // The terminal is still delivered, and only then does the stream end.
    let record = receiver.recv().expect("the terminal");
    assert!(record.is_terminal());
    assert!(receiver.recv().is_none());
}

#[test]
fn a_begin_is_registered_when_the_servicer_reaches_it() {
    // Deliberately no assertion that it is *not* yet registered: the pool is
    // live here, so the servicer may already have run. What is guaranteed is
    // that servicing registers it.
    let (session, _receiver) = session();
    let enumeration = admit(&session);

    service(&session);
    assert_eq!(session.enumerations(), 1);
    assert!(session.shared.contains(enumeration));
}

#[test]
fn several_enumerations_share_one_session() {
    let (session, _receiver) = session();
    let first = admit(&session);
    let second = admit(&session);
    service(&session);
    assert_eq!(session.enumerations(), 2);
    assert!(session.shared.contains(first));
    assert!(session.shared.contains(second));
}

#[test]
fn a_cancellation_finishes_a_quiescent_enumeration_with_one_terminal() {
    let (session, receiver) = session();
    let handle = session.try_begin(request()).expect("room");
    let enumeration = handle.id();
    service(&session);

    handle.cancel();
    service(&session);

    assert_eq!(session.enumerations(), 0);
    let record = receiver.try_recv().expect("a terminal");
    match record {
        Completion::Terminal {
            enumeration: id,
            outcome,
        } => {
            assert_eq!(id, enumeration);
            assert!(matches!(outcome, TerminalOutcome::Cancelled));
        }
        Completion::Entry { .. } => panic!("expected a terminal"),
    }
    assert!(receiver.try_recv().is_none(), "exactly one terminal");
}

#[test]
fn cancelling_an_unknown_enumeration_is_not_an_error() {
    // A cancellation that lost the race against completion has nothing to do.
    let (session, receiver) = session();
    let handle = session.try_begin(request()).expect("room");
    service(&session);
    handle.cancel();
    service(&session);
    assert!(receiver.try_recv().is_some(), "the first cancel lands");

    // Servicing a cancel for an enumeration that is already gone is a no-op.
    let slot = session
        .shared
        .submissions
        .reserve_cancel()
        .expect("submission room");
    let outcome = session
        .shared
        .submissions
        .push_cancel(slot, EnumerationId::from_raw(9999));
    assert_eq!(outcome, PushOutcome::RingDoorbell);
    service(&session);
    assert!(receiver.try_recv().is_none());
}

#[test]
fn a_cancel_serviced_before_its_begin_leaves_the_begin_to_register_normally() {
    // Both travel one ordered path, so a cancel queued first is applied first
    // and simply finds nothing; the later begin then registers.
    let (session, receiver) = session();
    let slot = session
        .shared
        .submissions
        .reserve_cancel()
        .expect("submission room");
    let _ = session
        .shared
        .submissions
        .push_cancel(slot, EnumerationId::from_raw(1));
    let enumeration = admit(&session);
    service(&session);

    assert_eq!(session.enumerations(), 1);
    assert!(session.shared.contains(enumeration));
    assert!(receiver.try_recv().is_none());
}

#[test]
fn abandonment_releases_every_registered_enumeration_without_a_terminal() {
    // No observer remains, so no outcome is owed.
    let (session, receiver) = session();
    admit(&session);
    admit(&session);
    service(&session);
    assert_eq!(session.enumerations(), 2);

    drop(receiver);
    service(&session);

    assert_eq!(session.enumerations(), 0);
}

#[test]
fn a_begin_is_refused_once_the_session_has_been_abandoned() {
    let (session, receiver) = session();
    drop(receiver);
    assert!(session.is_abandoned());

    session.try_begin(request()).expect_err("abandoned");
    service(&session);
    assert_eq!(session.enumerations(), 0);
}

#[test]
fn abandonment_frees_the_completion_reservations_it_released() {
    // The smallest ring makes the accounting visible: one reservation uses the
    // only reservable slot, and abandonment must give it back.
    let (session, receiver) = session_with(8, MINIMUM_COMPLETION_RING_CAPACITY);
    admit(&session);
    service(&session);
    assert!(
        session
            .shared
            .completions
            .reserve_terminal(EnumerationId::from_raw(99))
            .is_none(),
        "the only reservable slot is taken"
    );

    drop(receiver);
    service(&session);

    assert_eq!(session.shared.completions.len(), 0);
    assert!(
        session
            .shared
            .completions
            .reserve_terminal(EnumerationId::from_raw(99))
            .is_some(),
        "the slot came back"
    );
}

#[test]
fn the_doorbell_drains_the_ring_on_a_pool_thread() {
    // The end-to-end path: submit, ring, and let the thread pool service it.
    let (session, _receiver) = live_session();
    let enumeration = admit(&session);

    for _ in 0..1000 {
        if session.shared.contains(enumeration) {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("the servicer never registered the enumeration");
}

#[test]
fn a_burst_of_submissions_is_serviced_by_coalesced_drains() {
    // Room for three concurrent begins: the standing abandon slot, two reserved
    // control messages each, and three unserviced begin messages.
    let (session, receiver) = Session::new(16, 16).expect("a session with room");
    let mut handles = Vec::new();
    for _ in 0..3 {
        handles.push(session.try_begin(request()).expect("room"));
    }
    for handle in handles {
        handle.cancel();
    }

    let mut terminals = 0;
    for _ in 0..1000 {
        while receiver.try_recv().is_some() {
            terminals += 1;
        }
        if terminals == 3 {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("expected 3 terminals, saw {terminals}");
}

#[test]
fn resuming_parked_enumerations_is_safe_when_none_are_parked() {
    // The receiver calls this on every successful take, including when nothing
    // is waiting for room.
    let (session, receiver) = session();
    admit(&session);
    service(&session);
    session
        .shared
        .completions
        .try_send_entry(Completion::Entry {
            enumeration: EnumerationId::from_raw(1),
            entry: named_file("a"),
        })
        .expect("room");
    assert!(receiver.try_recv().is_some());
    assert_eq!(session.enumerations(), 1);
}

#[test]
fn debug_output_names_the_bounds() {
    let (session, receiver) = Session::new(4, 5).expect("valid");
    let rendered = format!("{session:?}");
    assert!(rendered.contains('4'), "{rendered}");
    let rendered = format!("{receiver:?}");
    assert!(rendered.contains('5'), "{rendered}");
}
