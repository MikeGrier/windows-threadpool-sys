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
    // The property a `Token` depends on to validate a completion (D-4): if two
    // live operations shared a `UserData`, either could claim the other's
    // completion.
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
    a.record_completion();
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
    for _ in 0..32 {
        a.reserve_user_data().expect("space remains");
    }
    for expected in (0..32usize).rev() {
        a.record_completion();
        assert_eq!(a.outstanding(), expected);
    }
}

#[test]
fn a_cancelled_reservation_lowers_it_too() {
    // The `Build*`-failed path. It must not count against run-down, because
    // the operation never entered the queue and no completion will arrive.
    let mut a = Accounting::new();
    a.reserve_user_data().expect("space remains");
    a.reserve_user_data().expect("space remains");
    a.cancel_reservation();
    assert_eq!(a.outstanding(), 1);
}

#[test]
fn the_outstanding_count_saturates_rather_than_wrapping() {
    // Both release paths, in both directions of the same rule. A wrap here
    // would make `run_down` wait on `usize::MAX` operations that do not exist
    // -- a hang, not a miscount.
    let mut a = Accounting::new();
    a.record_completion();
    assert_eq!(a.outstanding(), 0, "recording against an empty ledger");
    a.cancel_reservation();
    assert_eq!(a.outstanding(), 0, "cancelling against an empty ledger");

    let mut b = Accounting::new();
    b.reserve_user_data().expect("space remains");
    b.record_completion();
    b.record_completion();
    b.cancel_reservation();
    assert_eq!(b.outstanding(), 0, "over-releasing a single reservation");
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
    a.reserve_user_data().expect("space remains");
    a.reserve_registered_files(2);
    a.reserve_registered_buffers(2);
    assert_eq!(a.outstanding(), 1, "registering does not mint an operation");

    a.record_completion();
    assert_eq!(
        a.registered_file_count(),
        2,
        "completing does not unregister"
    );
    assert_eq!(a.registered_buffer_count(), 2);
}

#[test]
fn every_ledger_gets_its_own_identity() {
    // What stops a token minted by one ring from claiming another's
    // completion. Built without any ring at all, which is the extraction
    // paying for itself: this used to need two live kernel objects.
    let ids: Vec<RingId> = (0..256).map(|_| Accounting::new().ring_id()).collect();
    let unique: std::collections::HashSet<RingId> = ids.iter().copied().collect();
    assert_eq!(unique.len(), ids.len(), "two ledgers shared an identity");
}

#[test]
fn a_ledgers_identity_does_not_change_over_its_life() {
    let mut a = Accounting::new();
    let id = a.ring_id();
    a.reserve_user_data().expect("space remains");
    a.record_completion();
    a.reserve_registered_files(4);
    assert_eq!(a.ring_id(), id, "identity is fixed for the ledger's life");
}

#[test]
fn identities_from_separate_ledgers_do_not_collide_even_though_both_start_at_zero() {
    // `UserData` restarts at 0 per ring, so the *pair* is what identifies an
    // operation -- which is the reason `RingId` exists and why `Token`
    // compares both.
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
