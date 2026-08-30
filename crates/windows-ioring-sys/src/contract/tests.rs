// Copyright (c) 2026 Mike Grier
//! Tests for [`RingContract`].
//!
//! Two halves, and the second matters as much as the first. The oracle must
//! catch each conservation failure it claims to catch -- and it must **not**
//! report a violation for a legal sequence, because an oracle that cries wolf
//! is one its reader learns to skip. The "does not fire" cases are therefore
//! written as deliberately as the "does fire" ones.

use super::{RingContract, Violation};

/// A complete, legal lifecycle: pushed, completed, claimed.
fn full_cycle(contract: &mut RingContract, user_data: usize) {
    contract.observe_push(user_data);
    contract.observe_completion(user_data);
    contract.observe_claim(user_data);
}

#[test]
fn a_complete_lifecycle_is_quiescent() {
    let mut contract = RingContract::new();
    full_cycle(&mut contract, 1);
    full_cycle(&mut contract, 2);
    full_cycle(&mut contract, 3);
    assert_eq!(contract.check_quiescent(), Vec::new());
}

#[test]
fn nothing_at_all_is_quiescent() {
    // The degenerate case. A contract that reported a violation for a ring
    // nobody used would fail every test that does not touch the ring.
    let contract = RingContract::new();
    assert_eq!(contract.check_quiescent(), Vec::new());
}

#[test]
fn a_push_that_never_completes_is_reported() {
    // The rule `IoRing::run_down`'s termination depends on: every queued SQE
    // completes, so one that has not is either still in flight or lost.
    let mut contract = RingContract::new();
    full_cycle(&mut contract, 1);
    contract.observe_push(2);

    assert_eq!(
        contract.check_quiescent(),
        vec![Violation::Outstanding { user_data: 2 }]
    );
}

#[test]
fn a_completion_claimed_by_nothing_is_reported_as_a_leak() {
    // This is `Appender::claim`'s real defect, in miniature: a completion was
    // observed, the token was never claimed, and the arena slot it held is
    // gone for the life of the process. Nothing else in the crate notices.
    let mut contract = RingContract::new();
    contract.observe_push(7);
    contract.observe_completion(7);
    // No claim.

    assert_eq!(
        contract.check_quiescent(),
        vec![Violation::LeakedToken { user_data: 7 }]
    );
}

#[test]
fn a_deliberate_leak_is_not_reported() {
    // Leaking is legitimate when it is *stated* -- it is what keeps a buffer
    // alive when a caller cannot prove the kernel is done. The difference
    // between a stated and an unstated leak is the whole point.
    let mut contract = RingContract::new();
    contract.observe_push(9);
    contract.observe_completion(9);
    contract.observe_deliberate_leak(9);

    assert_eq!(contract.check_quiescent(), Vec::new());
}

#[test]
fn a_deliberate_leak_before_any_completion_is_also_accepted() {
    // A token abandoned while its operation is still in flight is the
    // canonical reason leaking exists: the kernel may still be reading the
    // buffer, so the memory must outlive the caller's knowledge of it.
    let mut contract = RingContract::new();
    contract.observe_push(11);
    contract.observe_deliberate_leak(11);

    assert_eq!(contract.check_quiescent(), Vec::new());
}

#[test]
fn a_second_completion_for_the_same_operation_is_reported() {
    // The "exactly" in "exactly one completion".
    let mut contract = RingContract::new();
    contract.observe_push(4);
    contract.observe_completion(4);
    contract.observe_completion(4);

    assert!(
        contract
            .violations()
            .contains(&Violation::DuplicateCompletion { user_data: 4 }),
        "a duplicate completion must be reported: {:?}",
        contract.violations()
    );
}

#[test]
fn a_completion_for_an_operation_never_pushed_is_reported() {
    let mut contract = RingContract::new();
    contract.observe_completion(0xDEAD);

    assert!(
        contract
            .violations()
            .contains(&Violation::UnexpectedCompletion { user_data: 0xDEAD }),
        "an unrecognised completion must be reported: {:?}",
        contract.violations()
    );
}

#[test]
fn a_completion_after_a_claim_is_a_duplicate_not_a_fresh_operation() {
    // Reusing a `user_data` that has already run its course must not look
    // like a new operation, or the ring could complete something twice and
    // the second one would pass as legitimate.
    let mut contract = RingContract::new();
    full_cycle(&mut contract, 5);
    contract.observe_completion(5);

    assert!(
        contract
            .violations()
            .contains(&Violation::DuplicateCompletion { user_data: 5 }),
        "a completion after a full cycle is a duplicate: {:?}",
        contract.violations()
    );
}

#[test]
fn a_busy_registered_buffer_is_reported_at_quiescence() {
    let mut contract = RingContract::new();
    contract.observe_buffer(0, 0);
    contract.observe_buffer(1, 2);

    assert_eq!(
        contract.check_quiescent(),
        vec![Violation::BufferStillInUse {
            index: 1,
            outstanding: 2
        }]
    );
}

#[test]
fn a_quiet_registered_buffer_is_not_reported() {
    let mut contract = RingContract::new();
    for index in 0..8 {
        contract.observe_buffer(index, 0);
    }
    assert_eq!(contract.check_quiescent(), Vec::new());
}

#[test]
fn a_buffer_reported_busy_then_quiet_is_not_reported() {
    // Counts are re-reported as they change; the latest observation wins.
    // Treating them as append-only would accuse every buffer that was ever
    // used.
    let mut contract = RingContract::new();
    contract.observe_buffer(3, 1);
    contract.observe_buffer(3, 0);
    assert_eq!(contract.check_quiescent(), Vec::new());
}

#[test]
fn violations_are_reported_in_a_stable_order() {
    // `HashMap` iteration order is unspecified, so without sorting the same
    // failure would print differently between runs and could not be diffed.
    let mut contract = RingContract::new();
    for user_data in [50_usize, 10, 30, 20, 40] {
        contract.observe_push(user_data);
    }

    let first = contract.check_quiescent();
    let second = contract.check_quiescent();
    assert_eq!(first, second, "repeated checks must agree");
    assert_eq!(
        first,
        vec![
            Violation::Outstanding { user_data: 10 },
            Violation::Outstanding { user_data: 20 },
            Violation::Outstanding { user_data: 30 },
            Violation::Outstanding { user_data: 40 },
            Violation::Outstanding { user_data: 50 },
        ]
    );
}

#[test]
fn every_violation_is_reported_rather_than_only_the_first() {
    // Several violations usually share one cause, and seeing only one invites
    // fixing a symptom.
    let mut contract = RingContract::new();
    contract.observe_push(1); // never completes
    contract.observe_push(2);
    contract.observe_completion(2); // never claimed
    contract.observe_buffer(0, 3); // still in use

    let violations = contract.check_quiescent();
    assert_eq!(violations.len(), 3, "got {violations:?}");
}

#[test]
fn assert_quiescent_names_every_violation() {
    let mut contract = RingContract::new();
    contract.observe_push(1);
    contract.observe_push(2);

    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        contract.assert_quiescent();
    }));
    let payload = panicked.expect_err("assert_quiescent must panic when the contract is broken");
    let message = payload
        .downcast_ref::<String>()
        .expect("a string panic payload");
    assert!(
        message.contains("0x1"),
        "should name operation 1: {message}"
    );
    assert!(
        message.contains("0x2"),
        "should name operation 2: {message}"
    );
}

#[test]
fn assert_quiescent_is_silent_when_the_contract_holds() {
    let mut contract = RingContract::new();
    full_cycle(&mut contract, 1);
    contract.assert_quiescent();
}

#[test]
fn a_tokenless_operation_is_complete_once_it_completes() {
    // `flush_raw` and `cancel_raw` return a bare `user_data` rather than a
    // token, because they own nothing a claim could hand back. Treating them
    // like token-carrying pushes would report a `LeakedToken` the caller
    // cannot possibly satisfy -- a violation that is not merely wrong but
    // unfixable, which is the worst kind an oracle can emit.
    //
    // Found by binding the real backpressure test in `submission_lifecycle.rs`
    // to this contract (M16.2), which pushes nothing but raw flushes.
    let mut contract = RingContract::new();
    contract.observe_tokenless_push(1);
    contract.observe_completion(1);

    assert_eq!(contract.check_quiescent(), Vec::new());
}

#[test]
fn a_tokenless_operation_that_never_completes_is_still_reported() {
    // Owning no token does not excuse it from the one-SQE-one-completion rule.
    let mut contract = RingContract::new();
    contract.observe_tokenless_push(2);

    assert_eq!(
        contract.check_quiescent(),
        vec![Violation::Outstanding { user_data: 2 }]
    );
}

#[test]
fn a_tokenless_operation_completing_twice_is_still_a_duplicate() {
    let mut contract = RingContract::new();
    contract.observe_tokenless_push(3);
    contract.observe_completion(3);
    contract.observe_completion(3);

    assert!(
        contract
            .violations()
            .contains(&Violation::DuplicateCompletion { user_data: 3 }),
        "got {:?}",
        contract.violations()
    );
}

#[test]
fn a_push_that_was_rejected_is_simply_never_observed() {
    // The other half of the rule: a `Build*` that fails synchronously releases
    // its reservation and produces no completion, so it is un-counted rather
    // than uncompleted. Reporting it would manufacture an `Outstanding`
    // violation at teardown for an operation that never existed.
    //
    // Nothing in the API enforces this -- it is a rule about what a caller
    // reports -- so it is written down as a test to make the intent legible.
    let mut contract = RingContract::new();
    contract.observe_tokenless_push(1);
    contract.observe_completion(1);
    // A second push was attempted here and rejected with queue-full. It is
    // deliberately not observed.
    assert_eq!(contract.check_quiescent(), Vec::new());
}

#[test]
fn busy_registered_buffers_are_reported_in_index_order() {
    // `check_quiescent` sorts its busy buffers so a failure reads the same way
    // twice -- `HashMap` iteration order is deliberately unspecified. M18.3
    // showed the sort's match arm could be deleted without any test noticing,
    // because the only test with a busy buffer had exactly one of them, and one
    // element is sorted whatever the key says.
    // Enough of them that an unsorted report is implausible rather than merely
    // unlikely: deleting the sort key leaves every entry comparing equal, and
    // `sort_by_key` is stable, so the output becomes `HashMap` iteration order.
    // With three entries that lands sorted one time in six -- which is exactly
    // what happened on the first attempt at this test, and the mutant survived.
    const BUSY: u32 = 8;

    let mut contract = RingContract::new();
    contract.observe_buffer(99, 0); // quiet, and highest -- must not appear
    for index in (0..BUSY).rev() {
        contract.observe_buffer(index, (index + 1) as usize);
    }

    let reported = contract.check_quiescent();
    let expected: Vec<_> = (0..BUSY)
        .map(|index| Violation::BufferStillInUse {
            index,
            outstanding: (index + 1) as usize,
        })
        .collect();

    assert_eq!(
        reported, expected,
        "busy buffers must be reported by ascending index, and the quiet one omitted"
    );
}
