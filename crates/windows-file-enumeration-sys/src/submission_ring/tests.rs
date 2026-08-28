// Copyright (c) 2026 Mike Grier
//! Tests for the bounded submission ring.

use super::*;

fn ring(capacity: usize) -> SubmissionRing {
    SubmissionRing::new(capacity)
}

fn cancel(id: u64) -> ControlMessage {
    ControlMessage::Cancel(EnumerationId::from_raw(id))
}

fn cancelled_id(message: &ControlMessage) -> u64 {
    match message {
        ControlMessage::Cancel(id) => id.get(),
        _ => panic!("expected a cancellation"),
    }
}

#[test]
fn ordinary_messages_fill_the_ring_and_then_are_refused() {
    let ring = ring(3);
    for id in 0..3 {
        let _ = ring.try_push(cancel(id)).expect("within the bound");
    }
    let (returned, reason) = ring.try_push(cancel(9)).expect_err("full");
    assert_eq!(reason, SubmitRejection::Full);
    // The message comes back so a caller keeps its request and captured token.
    assert_eq!(cancelled_id(&returned), 9);
    assert_eq!(ring.len(), 3);
}

#[test]
fn messages_are_serviced_in_the_order_they_were_queued() {
    let ring = ring(4);
    for id in 0..3 {
        let _ = ring.try_push(cancel(id)).expect("room");
    }
    for id in 0..3 {
        let message = ring.take_for_service().expect("a queued message");
        assert_eq!(cancelled_id(&message), id);
    }
    assert!(ring.take_for_service().is_none());
}

#[test]
fn the_first_submission_rings_the_doorbell_and_the_rest_coalesce() {
    // A burst must not queue a burst of drains behind it.
    let ring = ring(4);
    assert_eq!(
        ring.try_push(cancel(1)).expect("room"),
        PushOutcome::RingDoorbell
    );
    assert_eq!(
        ring.try_push(cancel(2)).expect("room"),
        PushOutcome::DrainAlreadyScheduled
    );
    assert_eq!(
        ring.try_push(cancel(3)).expect("room"),
        PushOutcome::DrainAlreadyScheduled
    );
}

#[test]
fn a_drain_that_runs_dry_lets_the_next_submission_schedule_again() {
    let ring = ring(4);
    let _ = ring.try_push(cancel(1)).expect("room");
    ring.take_for_service().expect("the queued message");
    // Still draining until the servicer sees an empty ring.
    assert_eq!(
        ring.try_push(cancel(2)).expect("room"),
        PushOutcome::DrainAlreadyScheduled
    );
    ring.take_for_service().expect("the second message");
    assert!(ring.take_for_service().is_none(), "the ring runs dry");
    assert_eq!(
        ring.try_push(cancel(3)).expect("room"),
        PushOutcome::RingDoorbell
    );
}

#[test]
fn a_reservation_is_held_back_from_ordinary_traffic() {
    let ring = ring(3);
    let slot = ring.reserve_cancel().expect("room");
    let _ = ring.try_push(cancel(1)).expect("room");
    let _ = ring.try_push(cancel(2)).expect("room");
    let (_, reason) = ring.try_push(cancel(3)).expect_err("the slot is reserved");
    assert_eq!(reason, SubmitRejection::Full);

    // The reserved send always succeeds, even against a ring full of ordinary
    // traffic.
    let _ = ring.push_cancel(slot, EnumerationId::from_raw(42));
    assert_eq!(ring.len(), 3);
}

#[test]
fn a_cancellation_can_be_sent_when_ordinary_traffic_has_filled_the_ring() {
    let ring = ring(3);
    let slot = ring.reserve_cancel().expect("room");
    let _ = ring.try_push(cancel(1)).expect("room");
    let _ = ring.try_push(cancel(2)).expect("room");
    let _ = ring.push_cancel(slot, EnumerationId::from_raw(7));

    // FIFO: the two ordinary messages precede the reserved one.
    assert_eq!(cancelled_id(&ring.take_for_service().expect("first")), 1);
    assert_eq!(cancelled_id(&ring.take_for_service().expect("second")), 2);
    assert_eq!(cancelled_id(&ring.take_for_service().expect("third")), 7);
}

#[test]
fn reservations_are_refused_when_the_ring_has_no_room() {
    let ring = ring(2);
    let _ = ring.try_push(cancel(1)).expect("room");
    let _ = ring.try_push(cancel(2)).expect("room");
    assert!(ring.reserve_cancel().is_none());
    assert!(ring.reserve_abandon().is_none());
}

#[test]
fn an_unused_reservation_returns_its_slot() {
    let ring = ring(2);
    let slot = ring.reserve_cancel().expect("room");
    let _ = ring.try_push(cancel(1)).expect("room");
    ring.try_push(cancel(2)).expect_err("the slot is reserved");

    release_cancel_slot(&ring, slot);
    let _ = ring.try_push(cancel(2)).expect("the slot came back");
}

#[test]
fn a_spent_reservation_cannot_be_returned_as_well() {
    // Spending consumes the slot, so there is no value left with which to
    // double-release the accounting.
    let ring = ring(3);
    let slot = ring.reserve_cancel().expect("room");
    let _ = ring.push_cancel(slot, EnumerationId::from_raw(1));

    let fresh = ring.reserve_cancel().expect("room");
    release_cancel_slot(&ring, fresh);
    assert_eq!(ring.len(), 1);
    // Two ordinary slots remain: the queued cancel occupies the third.
    let _ = ring.try_push(cancel(2)).expect("room");
    let _ = ring.try_push(cancel(3)).expect("room");
    ring.try_push(cancel(4)).expect_err("full");
}

#[test]
fn abandonment_latches_and_refuses_further_ordinary_traffic() {
    let ring = ring(4);
    let slot = ring.reserve_abandon().expect("room");
    assert!(!ring.is_abandoned());

    let _ = ring.push_abandon(slot);
    assert!(ring.is_abandoned());

    let (_, reason) = ring.try_push(cancel(1)).expect_err("abandoned");
    assert_eq!(reason, SubmitRejection::Abandoned);
}

#[test]
fn abandonment_is_refused_before_a_full_ring_rather_than_after() {
    // Abandonment is checked first, so a caller learns the session is gone
    // rather than being told to retry against a ring that will never accept it.
    let ring = ring(3);
    let slot = ring.reserve_abandon().expect("room");
    let _ = ring.push_abandon(slot);
    ring.try_push(cancel(1)).expect_err("abandoned");
    assert_eq!(ring.len(), 1, "only the abandon message is queued");
}

#[test]
fn an_abandon_message_can_be_sent_against_a_full_ring() {
    let ring = ring(3);
    let slot = ring.reserve_abandon().expect("room");
    let _ = ring.try_push(cancel(1)).expect("room");
    let _ = ring.try_push(cancel(2)).expect("room");
    let _ = ring.push_abandon(slot);
    assert_eq!(ring.len(), 3);
    assert!(matches!(
        ring.take_for_service().expect("first"),
        ControlMessage::Cancel(_)
    ));
}

#[test]
fn the_abandon_reservation_is_held_for_the_session_s_whole_life() {
    // Unlike a cancellation slot, this one is never returned: the receiver
    // always spends it, so there is no release path to get wrong.
    let ring = ring(2);
    let slot = ring.reserve_abandon().expect("room");
    let _ = ring.try_push(cancel(1)).expect("room");
    ring.try_push(cancel(2))
        .expect_err("the abandon slot is reserved");
    let _ = ring.push_abandon(slot);
    assert_eq!(ring.len(), 2);
}

#[test]
#[should_panic(expected = "at least one message")]
fn a_ring_of_zero_is_rejected_at_construction() {
    let _ = SubmissionRing::new(0);
}

#[test]
fn the_capacity_is_reported_as_configured() {
    assert_eq!(ring(5).capacity(), 5);
}
