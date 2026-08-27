// Copyright (c) 2026 Mike Grier
//! Scripted interleavings over the session model.
//!
//! Each test is one ordering of events that the shell must survive. The model
//! re-checks every invariant after every step, so what a test asserts at the end
//! is only the *outcome*; the safety properties were already proven along the
//! way.

use super::*;
use crate::session::{MINIMUM_COMPLETION_RING_CAPACITY, MINIMUM_SUBMISSION_CAPACITY};

/// The ordinary life of one enumeration: start, deliver, finish, drain.
#[test]
fn one_enumeration_runs_to_completion() {
    let mut model = Model::new(8, 8);
    model.run(&[
        Op::Begin,
        Op::Service,
        Op::EnterQuantum(0),
        Op::OfferEntry(0, "a"),
        Op::OfferEntry(0, "b"),
        Op::LeaveQuantum(0, false),
        Op::Complete(0),
        Op::Detach(0),
        Op::DrainReceiver,
    ]);
    assert_eq!(model.entries(0), ["a", "b"]);
    assert_eq!(model.terminal(0), Some("completed"));
    assert_eq!(model.registered(), 0);
}

/// Two enumerations share one session; each keeps its own order and terminal.
#[test]
fn two_enumerations_interleave_without_losing_their_own_order() {
    let mut model = Model::new(16, 16);
    model.run(&[
        Op::Begin,
        Op::Begin,
        Op::Service,
        Op::OfferEntry(0, "a0"),
        Op::OfferEntry(1, "b0"),
        Op::OfferEntry(0, "a1"),
        Op::OfferEntry(1, "b1"),
        Op::OfferEntry(0, "a2"),
        Op::Complete(0),
        Op::Complete(1),
        Op::Detach(0),
        Op::Detach(1),
        Op::DrainReceiver,
    ]);
    assert_eq!(model.entries(0), ["a0", "a1", "a2"]);
    assert_eq!(model.entries(1), ["b0", "b1"]);
    assert_eq!(model.terminal(0), Some("completed"));
    assert_eq!(model.terminal(1), Some("completed"));
}

/// Cancellation while nothing is running finishes the enumeration at once.
#[test]
fn cancelling_a_quiescent_enumeration_terminates_it() {
    let mut model = Model::new(8, 8);
    model.run(&[
        Op::Begin,
        Op::Service,
        Op::Cancel(0),
        Op::Service,
        Op::DrainReceiver,
    ]);
    assert_eq!(model.terminal(0), Some("cancelled"));
    assert_eq!(model.registered(), 0);
}

/// Dropping the affine handle is cancellation, by a different name.
#[test]
fn dropping_the_handle_terminates_the_enumeration() {
    let mut model = Model::new(8, 8);
    model.run(&[
        Op::Begin,
        Op::Service,
        Op::DropHandle(0),
        Op::Service,
        Op::DrainReceiver,
    ]);
    assert_eq!(model.terminal(0), Some("cancelled"));
}

/// Cancellation cannot preempt a running quantum, and must not deliver its
/// terminal before the entries that quantum had already produced.
#[test]
fn cancelling_during_a_quantum_defers_the_terminal_behind_its_entries() {
    let mut model = Model::new(8, 8);
    model.run(&[
        Op::Begin,
        Op::Service,
        Op::EnterQuantum(0),
        Op::OfferEntry(0, "a"),
        // The cancel is serviced while the quantum is still running, so it can
        // only record the intent.
        Op::Cancel(0),
        Op::Service,
    ]);
    assert_eq!(model.registered(), 1, "the quantum still owns it");
    assert_eq!(model.terminal(0), None);

    model.run(&[
        // The quantum produces one more entry before it notices, which must
        // still be delivered.
        Op::OfferEntry(0, "b"),
        Op::LeaveQuantum(0, false),
        Op::DrainReceiver,
    ]);
    assert_eq!(model.entries(0), ["a", "b"]);
    assert_eq!(model.terminal(0), Some("cancelled"));
    assert_eq!(model.registered(), 0);
}

/// A quantum that finds its enumeration already cancelled does no work.
#[test]
fn a_quantum_scheduled_after_cancellation_does_nothing() {
    let mut model = Model::new(8, 8);
    model.run(&[
        Op::Begin,
        Op::Service,
        Op::Cancel(0),
        Op::Service,
        // The enumeration is gone, so entering finds nothing and leaving is a
        // no-op rather than a second terminal.
        Op::EnterQuantum(0),
        Op::LeaveQuantum(0, false),
        Op::DrainReceiver,
    ]);
    assert_eq!(model.terminal(0), Some("cancelled"));
}

/// A cancel serviced before its begin simply finds nothing; the begin then
/// registers normally rather than being retroactively cancelled.
#[test]
fn a_cancel_for_an_unknown_enumeration_is_ignored() {
    let mut model = Model::new(8, 8);
    model.run(&[
        Op::Begin,
        Op::Service,
        Op::Cancel(0),
        Op::Service,
        // A second enumeration reusing the freed capacity is unaffected.
        Op::Begin,
        Op::Service,
        Op::Detach(1),
    ]);
    assert_eq!(model.registered(), 1);
    assert_eq!(model.terminal(1), None);
}

/// A full ring refuses entries instead of dropping them, and accepts again as
/// soon as the receiver makes room.
#[test]
fn backpressure_refuses_entries_and_resumes_after_a_take() {
    // Capacity 4 with one reserved terminal leaves room for three entries.
    let mut model = Model::new(8, 4);
    model.run(&[
        Op::Begin,
        Op::Service,
        Op::OfferEntry(0, "a"),
        Op::OfferEntry(0, "b"),
        Op::OfferEntry(0, "c"),
        Op::OfferEntry(0, "d"),
    ]);
    assert_eq!(model.refused(), 1, "the fourth entry had nowhere to go");

    model.run(&[Op::Recv, Op::OfferEntry(0, "d")]);
    assert_eq!(model.refused(), 1, "room appeared, so the retry succeeded");

    model.run(&[Op::Complete(0), Op::Detach(0), Op::DrainReceiver]);
    assert_eq!(model.entries(0), ["a", "b", "c", "d"]);
    assert_eq!(model.terminal(0), Some("completed"));
}

/// Sharing a session shares backpressure: one enumeration filling the ring
/// stalls the other, and neither loses an entry.
#[test]
fn backpressure_is_shared_between_enumerations_in_one_session() {
    // Capacity 5, two reserved terminals, so three entry slots between them.
    let mut model = Model::new(16, 5);
    model.run(&[
        Op::Begin,
        Op::Begin,
        Op::Service,
        Op::OfferEntry(0, "a0"),
        Op::OfferEntry(0, "a1"),
        Op::OfferEntry(0, "a2"),
        Op::OfferEntry(1, "b0"),
    ]);
    assert_eq!(model.refused(), 1, "the second enumeration is stalled too");

    model.run(&[Op::Recv, Op::OfferEntry(1, "b0"), Op::DrainReceiver]);
    assert_eq!(model.entries(0), ["a0", "a1", "a2"]);
    assert_eq!(model.entries(1), ["b0"]);

    model.run(&[Op::Detach(0), Op::Detach(1)]);
}

/// A parked enumeration is the one waiting for room; taking a record is what
/// un-parks it.
#[test]
fn a_parked_enumeration_is_resumed_by_a_take() {
    // Capacity 3 with one reserved terminal leaves exactly two entry slots.
    let mut model = Model::new(8, 3);
    model.run(&[
        Op::Begin,
        Op::Service,
        Op::EnterQuantum(0),
        Op::OfferEntry(0, "a"),
        Op::OfferEntry(0, "b"),
        // No room for the third, so the quantum yields parked rather than
        // blocking a worker or dropping the record it had not yet parsed.
        Op::OfferEntry(0, "c"),
        Op::LeaveQuantum(0, true),
    ]);
    assert_eq!(model.refused(), 1);

    model.run(&[
        Op::Recv,
        Op::OfferEntry(0, "c"),
        Op::Complete(0),
        Op::Detach(0),
        Op::DrainReceiver,
    ]);
    assert_eq!(model.entries(0), ["a", "b", "c"]);
    assert_eq!(model.terminal(0), Some("completed"));
}

/// The terminal is deliverable exactly when there is no ordinary room left,
/// which is the reason its slot is reserved at admission.
#[test]
fn a_terminal_lands_in_a_ring_with_no_ordinary_room() {
    let mut model = Model::new(8, 3);
    model.run(&[
        Op::Begin,
        Op::Service,
        Op::OfferEntry(0, "a"),
        Op::OfferEntry(0, "b"),
        Op::OfferEntry(0, "c"),
    ]);
    assert_eq!(model.refused(), 1);
    model.run(&[Op::Complete(0), Op::Detach(0), Op::DrainReceiver]);
    assert_eq!(model.entries(0), ["a", "b"]);
    assert_eq!(model.terminal(0), Some("completed"));
}

/// Abandonment stops everything and owes nothing, because nobody is left to
/// observe an outcome.
#[test]
fn abandonment_releases_everything_without_a_terminal() {
    let mut model = Model::new(16, 16);
    model.run(&[
        Op::Begin,
        Op::Begin,
        Op::Service,
        Op::OfferEntry(0, "a"),
        Op::DropReceiver,
        Op::Service,
        Op::BeginRefused(BeginFailure::Abandoned),
    ]);
    assert_eq!(model.registered(), 0);
    assert_eq!(model.terminal(0), None);
    assert_eq!(model.terminal(1), None);
    model.run(&[Op::Detach(0), Op::Detach(1)]);
}

/// Abandonment during a running quantum still tears down, and the quantum's
/// later transitions are harmless no-ops.
#[test]
fn abandonment_during_a_quantum_is_safe() {
    let mut model = Model::new(8, 8);
    model.run(&[
        Op::Begin,
        Op::Service,
        Op::EnterQuantum(0),
        Op::DropReceiver,
        Op::Service,
        Op::OfferEntry(0, "a"),
        Op::LeaveQuantum(0, false),
        Op::Complete(0),
        Op::Detach(0),
    ]);
    assert_eq!(model.registered(), 0);
}

/// Cancelling after the enumeration already completed produces no second
/// terminal.
#[test]
fn cancelling_a_completed_enumeration_adds_no_terminal() {
    let mut model = Model::new(8, 8);
    model.run(&[
        Op::Begin,
        Op::Service,
        Op::Complete(0),
        // The handle still holds its reservation, so this is the ordinary race
        // between a caller cancelling and an enumeration finishing.
        Op::Cancel(0),
        Op::Service,
        Op::DrainReceiver,
    ]);
    assert_eq!(model.terminal(0), Some("completed"));
    assert_eq!(model.registered(), 0);
}

/// Detaching leaves the enumeration running and returns its reservation.
#[test]
fn a_detached_enumeration_still_reports_its_outcome() {
    let mut model = Model::new(8, 8);
    model.run(&[
        Op::Begin,
        Op::Service,
        Op::Detach(0),
        Op::OfferEntry(0, "a"),
        Op::Complete(0),
        Op::DrainReceiver,
    ]);
    assert_eq!(model.entries(0), ["a"]);
    assert_eq!(model.terminal(0), Some("completed"));
}

/// The stream stays open while an outcome is still owed, even with no session
/// handle left.
#[test]
fn dropping_the_session_does_not_strand_an_owed_terminal() {
    let mut model = Model::new(8, 8);
    model.run(&[
        Op::Begin,
        Op::Service,
        Op::OfferEntry(0, "a"),
        Op::Complete(0),
        Op::Detach(0),
        Op::DropSession,
        Op::DrainReceiver,
    ]);
    assert_eq!(model.entries(0), ["a"]);
    assert_eq!(model.terminal(0), Some("completed"));
}

/// The smallest legal rings still carry one enumeration end to end.
#[test]
fn the_minimum_bounds_carry_one_enumeration() {
    let mut model = Model::new(
        MINIMUM_SUBMISSION_CAPACITY,
        MINIMUM_COMPLETION_RING_CAPACITY,
    );
    model.run(&[
        Op::Begin,
        // Both bounds are exactly one enumeration's worth.
        Op::BeginRefused(BeginFailure::SubmissionRingFull),
        Op::Service,
        Op::OfferEntry(0, "only"),
        Op::OfferEntry(0, "refused"),
    ]);
    assert_eq!(model.refused(), 1);

    model.run(&[Op::Complete(0), Op::Detach(0), Op::DrainReceiver]);
    assert_eq!(model.entries(0), ["only"]);
    assert_eq!(model.terminal(0), Some("completed"));
}

/// A completion ring that could not keep an unreserved data slot is rejected
/// outright rather than deadlocking every enumeration it accepts.
#[test]
fn a_completion_ring_of_one_is_rejected() {
    let error = Session::new(8, 1).expect_err("a ring of one cannot hold both");
    assert_eq!(
        error.failure(),
        crate::error::SessionFailure::CompletionCapacityTooSmall
    );
}

/// The completion ring refuses a begin once it cannot reserve another terminal,
/// which is the boundary that keeps one slot unreserved.
#[test]
fn admission_stops_at_the_completion_ring_s_reservation_boundary() {
    // Capacity 3 admits two enumerations; a third would reserve every slot.
    let mut model = Model::new(32, 3);
    model.run(&[
        Op::Begin,
        Op::Begin,
        Op::BeginRefused(BeginFailure::CompletionRingFull),
        Op::Service,
    ]);
    assert_eq!(model.registered(), 2);
    model.run(&[Op::Detach(0), Op::Detach(1)]);
}

/// Repeated start/cancel cycles neither leak reservations nor duplicate
/// terminals.
#[test]
fn repeated_cycles_leak_nothing() {
    let mut model = Model::new(MINIMUM_SUBMISSION_CAPACITY, 4);
    for _ in 0..5 {
        model.run(&[Op::Begin, Op::Service]);
        let slot = model.ids.len() - 1;
        model.run(&[
            Op::OfferEntry(slot, "x"),
            Op::Cancel(slot),
            Op::Service,
            Op::DrainReceiver,
        ]);
        assert_eq!(model.entries(slot), ["x"]);
        assert_eq!(model.terminal(slot), Some("cancelled"));
        assert_eq!(model.registered(), 0);
    }
}

/// Servicing an empty ring is a no-op, however often it happens.
#[test]
fn redundant_servicing_changes_nothing() {
    let mut model = Model::new(8, 8);
    model.run(&[
        Op::Service,
        Op::Service,
        Op::Begin,
        Op::Service,
        Op::Service,
        Op::Service,
        Op::Detach(0),
    ]);
    assert_eq!(model.registered(), 1);
}

/// Draining an empty ring is a no-op, and leaves the doorbell reset.
#[test]
fn draining_an_empty_ring_changes_nothing() {
    let mut model = Model::new(8, 8);
    model.run(&[
        Op::DrainReceiver,
        Op::Recv,
        Op::Begin,
        Op::Service,
        Op::DrainReceiver,
        Op::Detach(0),
    ]);
    assert_eq!(model.registered(), 1);
}
