// Copyright (c) 2026 Mike Grier
//! Tests for the ring's bookkeeping (M24.2).
//!
//! **These open no ring.** That is the point of the extraction: every rule
//! below is this crate's own specification, so it can be checked exhaustively
//! in memory rather than by driving a kernel object and hoping the interesting
//! state is reachable. They are the first tests in `src/` for which
//! [D-49](../../DESIGN-NOTES.md#d-49)'s complaint -- that `cargo test --lib`
//! does not mean what its name implies -- does not apply.
//!
//! They assert nothing about Windows, and must not start to. The moment a test
//! here encodes a belief about kernel behaviour it becomes the frozen
//! observation [D-52](../../DESIGN-NOTES.md#d-52) warns about.

use super::{Accounting, RingId};

#[test]
fn a_fresh_ledger_is_empty() {
    let a = Accounting::new();
    assert_eq!(a.outstanding(), 0);
    assert_eq!(a.registered_file_count(), 0);
    assert_eq!(a.registered_buffer_count(), 0);
}

#[test]
fn identities_start_at_zero_and_increase_by_one() {
    let mut a = Accounting::new();
    for expected in 0..64usize {
        assert_eq!(a.reserve_user_data().expect("space remains"), expected);
    }
}

#[test]
fn an_identity_is_never_handed_out_twice() {
    // The property the ring's inventory depends on to match a completion to
    // its operation (D-4): if two live operations shared a `UserData`, either
    // could be handed the other's payload.
    let mut a = Accounting::new();
    let mut seen = std::collections::HashSet::new();
    for _ in 0..4096 {
        let id = a.reserve_user_data().expect("space remains");
        assert!(seen.insert(id), "identity {id} was handed out twice");
    }
}

#[test]
fn completing_an_operation_does_not_recycle_its_identity() {
    // Reserve, complete, reserve again: the count returns to zero but the
    // identity must still move on. Recycling would be the same defect as
    // duplication, arriving later.
    let mut a = Accounting::new();
    let first = a.reserve_user_data().expect("space remains");
    assert!(a.record_completion(first));
    assert_eq!(a.outstanding(), 0);
    let second = a.reserve_user_data().expect("space remains");
    assert_ne!(
        first, second,
        "an identity must not be reused after completion"
    );
}

#[test]
fn reserving_raises_the_outstanding_count() {
    let mut a = Accounting::new();
    for expected in 1..=32usize {
        a.reserve_user_data().expect("space remains");
        assert_eq!(a.outstanding(), expected);
    }
}

#[test]
fn a_recorded_completion_lowers_it() {
    let mut a = Accounting::new();
    let ids: Vec<usize> = (0..32)
        .map(|_| a.reserve_user_data().expect("space remains"))
        .collect();
    for (done, id) in ids.iter().enumerate() {
        assert!(a.record_completion(*id), "{id} was minted and in flight");
        assert_eq!(a.outstanding(), ids.len() - done - 1);
    }
    assert!(a.is_quiescent());
}

#[test]
fn completions_retire_their_own_identity_in_any_order() {
    // The kernel completes out of submission order, so retirement must be by
    // identity: retiring B first leaves exactly A outstanding.
    let mut a = Accounting::new();
    let first = a.reserve_user_data().expect("space remains");
    let second = a.reserve_user_data().expect("space remains");
    assert!(a.record_completion(second));
    assert!(!a.is_quiescent(), "the first operation is still owed");
    assert!(
        !a.record_completion(second),
        "the second is already retired"
    );
    assert!(
        !a.is_quiescent(),
        "a repeat of the second retired the first"
    );
    assert!(a.record_completion(first));
    assert!(a.is_quiescent());
}

#[test]
fn a_cancelled_reservation_lowers_it_too() {
    // The `Build*`-failed path. It must not count against run-down, because
    // the operation never entered the queue and no completion will arrive.
    let mut a = Accounting::new();
    let kept = a.reserve_user_data().expect("space remains");
    let failed = a.reserve_user_data().expect("space remains");
    a.cancel_reservation(failed);
    assert_eq!(a.outstanding(), 1);
    assert!(
        !a.record_completion(failed),
        "a cancelled reservation can never be retired again"
    );
    assert!(a.record_completion(kept), "the surviving one is still owed");
    assert!(a.is_quiescent());
}

#[test]
fn a_completion_against_an_empty_ledger_retires_nothing() {
    // The case a counter had to SATURATE to survive: a wrap would make
    // `run_down` wait on `usize::MAX` operations that do not exist. An
    // identity set has nothing to wrap.
    let mut a = Accounting::new();
    assert!(!a.record_completion(0), "identity 0 was never minted");
    assert_eq!(a.outstanding(), 0);
    assert!(a.is_quiescent());
}

#[test]
fn a_foreign_or_duplicate_completion_cannot_fake_quiescence() {
    // PR #113 review: two operations in flight, a foreign CQE and the first
    // real one arrive. A saturating counter reached zero there while the
    // second operation was still owed, and for a raw push -- which stows
    // nothing -- no inventory entry was left to contradict it, so rundown
    // closed a ring the kernel could still write through.
    let mut a = Accounting::new();
    let first = a.reserve_user_data().expect("space remains");
    let second = a.reserve_user_data().expect("space remains");

    assert!(
        !a.record_completion(usize::MAX),
        "a foreign identity was never minted"
    );
    assert!(a.record_completion(first));
    assert!(
        !a.record_completion(first),
        "a duplicate of a retired identity"
    );
    assert_eq!(a.outstanding(), 1, "only the second is still owed");
    assert!(!a.is_quiescent(), "the second operation is still in flight");

    // And the other direction: the real completion is what quiesces it.
    assert!(a.record_completion(second));
    assert!(a.is_quiescent());
}

#[test]
fn an_identity_still_in_flight_is_refused_rather_than_aliased() {
    // Unreachable through a real ring, since the counter never repeats. The
    // test hook moves it back by hand to reach the one state the identity set
    // cannot survive: two operations under one key, where the first completion
    // would retire both and rundown would report a quiesce that has not
    // happened.
    let mut a = Accounting::new();
    let live = a.reserve_user_data().expect("space remains");
    a.set_next_user_data_for_test(live);
    let error = a
        .reserve_user_data()
        .expect_err("the identity is still in flight");
    assert!(
        error.to_string().contains("already in flight"),
        "unexpected error: {error}"
    );
    assert_eq!(a.outstanding(), 1, "the refusal minted nothing");

    // The other direction: once retired, the guard does not refuse it. The
    // counter never offers it again on a real ring, but the refusal is about
    // aliasing, not about history.
    assert!(a.record_completion(live));
    a.set_next_user_data_for_test(live);
    assert_eq!(a.reserve_user_data().expect("not in flight"), live);
}

#[test]
fn identity_exhaustion_is_an_error_rather_than_a_reuse() {
    // Unreachable in practice, and that is exactly why it is tested here: a
    // ledger reached through a real ring could never be driven to this state,
    // and the alternative to erroring is silently handing out a duplicate.
    let mut a = Accounting::new();
    a.set_next_user_data_for_test(usize::MAX - 1);

    let last = a
        .reserve_user_data()
        .expect("the final identity is available");
    assert_eq!(last, usize::MAX - 1);

    let error = a
        .reserve_user_data()
        .expect_err("there is no identity after the last one");
    assert!(
        error.to_string().contains("exhausted"),
        "the error says what happened: {error}"
    );
}

#[test]
fn the_last_identity_is_max_minus_one_not_max() {
    // Measured, not assumed -- the first draft of these tests asserted
    // `usize::MAX` and was wrong. The counter is advanced *before* the value
    // is returned, so a reservation made at `usize::MAX` fails rather than
    // handing it out: the identity space is `0..=usize::MAX - 1`.
    //
    // One value out of 2^64 is not worth reclaiming, and "fails rather than
    // wraps" is the property that matters. It is recorded because the
    // off-by-one is invisible from the source, and the next reader would
    // otherwise make the same wrong assumption this test's author did.
    let mut a = Accounting::new();
    a.set_next_user_data_for_test(usize::MAX);
    let _ = a
        .reserve_user_data()
        .expect_err("usize::MAX is never handed out");
}

#[test]
fn an_exhausted_ledger_does_not_charge_the_failed_reservation() {
    // The failure path must be clean: an identity that was never handed out
    // must not leave an operation outstanding, or run-down waits forever for a
    // completion that no operation will ever produce.
    let mut a = Accounting::new();
    a.set_next_user_data_for_test(usize::MAX);
    let before = a.outstanding();
    let _ = a.reserve_user_data().expect_err("exhausted");
    assert_eq!(
        a.outstanding(),
        before,
        "a refused reservation costs nothing"
    );
}

#[test]
fn registration_counts_advance_by_the_amount_reserved() {
    let mut a = Accounting::new();
    a.reserve_registered_files(4);
    assert_eq!(a.registered_file_count(), 4);
    a.reserve_registered_files(3);
    assert_eq!(a.registered_file_count(), 7);

    a.reserve_registered_buffers(8);
    assert_eq!(a.registered_buffer_count(), 8);
}

#[test]
fn the_two_registration_counts_are_independent() {
    // They index different kernel tables. Advancing one must not move the
    // other, or a registered buffer's index would collide with a file's.
    let mut a = Accounting::new();
    a.reserve_registered_files(5);
    assert_eq!(
        a.registered_buffer_count(),
        0,
        "files must not move buffers"
    );
    a.reserve_registered_buffers(9);
    assert_eq!(
        a.registered_file_count(),
        5,
        "and buffers must not move files"
    );
}

#[test]
fn registration_counts_saturate_rather_than_wrapping() {
    let mut a = Accounting::new();
    a.reserve_registered_files(u32::MAX);
    a.reserve_registered_files(1);
    assert_eq!(a.registered_file_count(), u32::MAX);

    a.reserve_registered_buffers(u32::MAX);
    a.reserve_registered_buffers(7);
    assert_eq!(a.registered_buffer_count(), u32::MAX);
}

#[test]
fn reserving_zero_registrations_changes_nothing() {
    let mut a = Accounting::new();
    a.reserve_registered_files(3);
    a.reserve_registered_files(0);
    assert_eq!(a.registered_file_count(), 3);
}

#[test]
fn registrations_and_operations_do_not_interfere() {
    let mut a = Accounting::new();
    let op = a.reserve_user_data().expect("space remains");
    a.reserve_registered_files(2);
    a.reserve_registered_buffers(2);
    assert_eq!(a.outstanding(), 1, "registering does not mint an operation");

    a.record_completion(op);
    assert_eq!(
        a.registered_file_count(),
        2,
        "completing does not unregister"
    );
    assert_eq!(a.registered_buffer_count(), 2);
}

#[test]
fn every_ledger_gets_its_own_identity() {
    // What stops an identity minted by one ring from being mistaken for
    // another's. Built without any ring at all, which is the extraction
    // paying for itself: this used to need two live kernel objects.
    let ids: Vec<RingId> = (0..256).map(|_| Accounting::new().ring_id()).collect();
    let unique: std::collections::HashSet<RingId> = ids.iter().copied().collect();
    assert_eq!(unique.len(), ids.len(), "two ledgers shared an identity");
}

#[test]
fn a_ledgers_identity_does_not_change_over_its_life() {
    let mut a = Accounting::new();
    let id = a.ring_id();
    let op = a.reserve_user_data().expect("space remains");
    a.record_completion(op);
    a.reserve_registered_files(4);
    assert_eq!(a.ring_id(), id, "identity is fixed for the ledger's life");
}

#[test]
fn identities_from_separate_ledgers_do_not_collide_even_though_both_start_at_zero() {
    // `UserData` restarts at 0 per ring, so the *pair* is what identifies an
    // operation -- which is the reason `RingId` exists and why `OperationId`
    // carries both.
    let mut a = Accounting::new();
    let mut b = Accounting::new();
    assert_eq!(a.reserve_user_data().expect("space"), 0);
    assert_eq!(b.reserve_user_data().expect("space"), 0);
    assert_ne!(
        a.ring_id(),
        b.ring_id(),
        "the ring id is what disambiguates the shared user data"
    );
}
