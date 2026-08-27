// Copyright (c) 2026 Mike Grier
//! Tests for [`ContractChecker`].
//!
//! Two halves, and the second matters as much as the first: the checker must
//! reject what the contract forbids, and must **accept** the sequences the M14
//! audit found are legal but surprising. Over-constraining is the same defect as
//! under-specifying, and this crate has shipped it once already.

use super::{ContractChecker, ContractViolation, Terminator};
use crate::directory::VolumeIdentity;
use crate::notify::{Change, ChangeKind, DesyncCause, RelativeName};
use crate::queue::{Notification, Outcome, WatchId};
use crate::retry::WatchMode;

fn watch() -> WatchId {
    WatchId::from_raw(1)
}

fn batch(watch: WatchId) -> Notification {
    Notification::Batch {
        watch,
        changes: vec![Change {
            kind: ChangeKind::Added,
            name: RelativeName::for_test("a.txt"),
        }],
    }
}

fn desync(watch: WatchId, cause: DesyncCause) -> Notification {
    Notification::Desync { watch, cause }
}

fn established(watch: WatchId, mode: WatchMode) -> Notification {
    Notification::Established { watch, mode }
}

fn volume(serial: u32) -> VolumeIdentity {
    VolumeIdentity::for_test(serial, "NTFS", "Label")
}

// --- terminality ---

#[test]
fn nothing_may_follow_a_cancelled_completion() {
    let mut checker = ContractChecker::new();
    let w = watch();
    checker
        .observe(&Notification::Completion {
            watch: w,
            outcome: Outcome::Cancelled,
        })
        .expect("cancellation is legal");
    assert_eq!(
        checker.observe(&batch(w)),
        Err(ContractViolation::AfterTerminal {
            watch: w,
            terminator: Terminator::Cancelled,
        })
    );
}

#[test]
fn nothing_may_follow_a_stopped_desync() {
    let mut checker = ContractChecker::new();
    let w = watch();
    checker
        .observe(&desync(w, DesyncCause::Stopped))
        .expect("a terminal desync is legal");
    assert_eq!(
        checker.observe(&desync(w, DesyncCause::Overflow)),
        Err(ContractViolation::AfterTerminal {
            watch: w,
            terminator: Terminator::Stopped,
        })
    );
}

#[test]
fn one_watchs_terminator_does_not_end_another() {
    // Ordering is defined *within* a subscription (D-12/D-26), so interleaving
    // across watches is never a violation.
    let mut checker = ContractChecker::new();
    let (a, b) = (WatchId::from_raw(1), WatchId::from_raw(2));
    checker
        .observe(&desync(a, DesyncCause::Stopped))
        .expect("a ends");
    checker.observe(&batch(b)).expect("b is untouched");
}

// --- tier-conditioned emission (D-17) ---

#[test]
fn a_coarse_watch_may_not_emit_a_batch_or_a_kernel_overflow() {
    let mut checker = ContractChecker::new();
    let w = watch();
    checker
        .observe(&established(w, WatchMode::Coarse))
        .expect("establishing coarse is legal");

    assert_eq!(
        checker.observe(&batch(w)),
        Err(ContractViolation::BatchInCoarseTier { watch: w })
    );
    assert_eq!(
        checker.observe(&desync(w, DesyncCause::Overflow)),
        Err(ContractViolation::CauseUnreachableInTier {
            watch: w,
            cause: DesyncCause::Overflow,
            tier: WatchMode::Coarse,
        })
    );
}

#[test]
fn a_coarse_watch_may_emit_queue_full() {
    // The over-correction that shipped once: QueueFull is a delivery-layer loss
    // and is tier-independent, so excluding it here would encode a restriction
    // the watcher does not keep.
    let mut checker = ContractChecker::new();
    let w = watch();
    checker
        .observe(&established(w, WatchMode::Coarse))
        .expect("coarse");
    checker
        .observe(&desync(w, DesyncCause::QueueFull))
        .expect("QueueFull is legal under either tier");
}

#[test]
fn a_detailed_watch_may_not_emit_the_coarse_cause() {
    let mut checker = ContractChecker::new();
    let w = watch();
    checker
        .observe(&established(w, WatchMode::Detailed))
        .expect("detailed");
    assert_eq!(
        checker.observe(&desync(w, DesyncCause::Coarse)),
        Err(ContractViolation::CauseUnreachableInTier {
            watch: w,
            cause: DesyncCause::Coarse,
            tier: WatchMode::Detailed,
        })
    );
}

#[test]
fn tier_rules_are_unchecked_until_a_tier_is_actually_reported() {
    // `Established` is opt-in (D-13), so a subscription without
    // `report_liveness` never reveals its tier. Assuming Detailed would invent
    // a rule; the checker leaves those rules unchecked instead.
    let mut checker = ContractChecker::new();
    let w = watch();
    checker
        .observe(&desync(w, DesyncCause::Coarse))
        .expect("no tier known, so nothing to contradict");
    checker
        .observe(&desync(w, DesyncCause::Overflow))
        .expect("likewise");
}

// --- what the audit says is legal, and must therefore be accepted ---

#[test]
fn a_resumed_without_a_preceding_suspended_is_legal() {
    // M14.2: a route coalescing onto an already-faulted watcher joins after
    // `enter_fault` sent its Suspendeds, so it observes a bracket it never saw
    // open. A checker that balanced brackets would reject a legal stream.
    let mut checker = ContractChecker::new();
    let w = watch();
    checker
        .observe(&Notification::Resumed { watch: w })
        .expect("Resumed may open a bracket the watch never saw start");
}

#[test]
fn a_suspended_closed_by_stopped_rather_than_resumed_is_legal() {
    // M14.2: a permanent failure to reopen closes the bracket with a terminator
    // instead of Resumed.
    let mut checker = ContractChecker::new();
    let w = watch();
    checker
        .observe(&Notification::Suspended { watch: w })
        .expect("suspend");
    checker
        .observe(&desync(w, DesyncCause::Stopped))
        .expect("a terminator may close an open bracket");
}

#[test]
fn established_need_not_be_a_watchs_first_notification() {
    // M14.2: for that same mid-fault join the order is Completion first, then
    // Desync { Reestablished }, Resumed, and only then the first Established.
    let mut checker = ContractChecker::new();
    let w = watch();
    checker
        .observe(&Notification::Completion {
            watch: w,
            outcome: Outcome::Subscribed,
        })
        .expect("completion first");
    checker
        .observe(&desync(w, DesyncCause::Reestablished))
        .expect("recovery");
    checker
        .observe(&Notification::Resumed { watch: w })
        .expect("resumed");
    checker
        .observe(&established(w, WatchMode::Detailed))
        .expect("and only now a tier");
}

#[test]
fn a_watchs_tier_may_change_between_establishments() {
    // D-61 re-resolves the tier on every reopen, so this is not stickiness to
    // enforce. Detailed is retried first every time, so a downgrade is not
    // permanent either -- both directions are legal.
    let mut checker = ContractChecker::new();
    let w = watch();
    checker
        .observe(&established(w, WatchMode::Detailed))
        .expect("detailed");
    checker
        .observe(&established(w, WatchMode::Coarse))
        .expect("downgraded");
    checker
        .observe(&established(w, WatchMode::Detailed))
        .expect("and back again");
}

#[test]
fn establishing_then_subscribed_is_legal() {
    // A retryable first open reports Establishing and is not terminal (D-46);
    // the watch may later establish for real.
    let mut checker = ContractChecker::new();
    let w = watch();
    checker
        .observe(&Notification::Completion {
            watch: w,
            outcome: Outcome::Establishing,
        })
        .expect("not a failure");
    checker
        .observe(&Notification::Completion {
            watch: w,
            outcome: Outcome::Subscribed,
        })
        .expect("and later establishes");
}

// --- volume-change continuity and distinctness (D-50/D-78) ---

#[test]
fn a_volume_change_to_the_same_serial_is_rejected() {
    let mut checker = ContractChecker::new();
    let w = watch();
    assert_eq!(
        checker.observe(&Notification::VolumeChanged {
            watch: w,
            previous: volume(0x1111),
            current: volume(0x1111),
        }),
        Err(ContractViolation::VolumeUnchanged { watch: w })
    );
}

#[test]
fn a_second_volume_change_must_continue_from_the_first() {
    let mut checker = ContractChecker::new();
    let w = watch();
    checker
        .observe(&Notification::VolumeChanged {
            watch: w,
            previous: volume(0x1111),
            current: volume(0x2222),
        })
        .expect("first change");

    // Continuing from 0x2222 is legal.
    let mut ok = ContractChecker::new();
    ok.observe(&Notification::VolumeChanged {
        watch: w,
        previous: volume(0x1111),
        current: volume(0x2222),
    })
    .expect("first");
    ok.observe(&Notification::VolumeChanged {
        watch: w,
        previous: volume(0x2222),
        current: volume(0x3333),
    })
    .expect("continues from the prior current");

    // Drawing `previous` independently is not.
    assert_eq!(
        checker.observe(&Notification::VolumeChanged {
            watch: w,
            previous: volume(0x9999),
            current: volume(0x3333),
        }),
        Err(ContractViolation::VolumeDiscontinuity { watch: w })
    );
}

// --- the batch API ---

#[test]
fn observe_all_reports_the_first_violation() {
    let mut checker = ContractChecker::new();
    let w = watch();
    let stream = vec![
        Notification::Completion {
            watch: w,
            outcome: Outcome::Subscribed,
        },
        desync(w, DesyncCause::Stopped),
        batch(w),
    ];
    assert_eq!(
        checker.observe_all(&stream),
        Err(ContractViolation::AfterTerminal {
            watch: w,
            terminator: Terminator::Stopped,
        })
    );
}

#[test]
fn a_batch_inside_a_fault_bracket_is_legal() {
    // `on_completion` re-arms before it decodes, so a read that completed and
    // then failed to re-arm enters the fault first and publishes the batch it
    // already had in hand afterwards. Dropping those changes to keep the
    // bracket tidy would be the silent loss the design forbids, so this
    // ordering is real. A checker that rejected data inside a bracket would
    // reject production output (PR #42 review).
    let mut checker = ContractChecker::new();
    let w = watch();
    checker
        .observe(&Notification::Suspended { watch: w })
        .expect("the fault opens");
    checker
        .observe(&batch(w))
        .expect("the completion that triggered it still delivers");
    checker
        .observe(&desync(w, DesyncCause::Reestablished))
        .expect("and the bracket resolves");
}

#[test]
fn a_volume_violation_does_not_cascade_into_the_next_change() {
    // `observe` documents that state advances even when it reports a violation,
    // so a caller that logs and continues sees genuinely new violations rather
    // than echoes of the first. Both volume branches used to return before
    // recording `current`, so the next legitimately-continuous change was
    // falsely reported as discontinuous too (PR #42 review).
    let mut checker = ContractChecker::new();
    let w = watch();

    checker
        .observe(&Notification::VolumeChanged {
            watch: w,
            previous: volume(0x1111),
            current: volume(0x2222),
        })
        .expect("first change");

    // A discontinuity: `previous` does not continue from the prior `current`.
    assert_eq!(
        checker.observe(&Notification::VolumeChanged {
            watch: w,
            previous: volume(0x9999),
            current: volume(0x3333),
        }),
        Err(ContractViolation::VolumeDiscontinuity { watch: w })
    );

    // Continuing from 0x3333 is legal and must be accepted: the violation above
    // advanced the state to its `current` rather than leaving 0x2222 behind.
    checker
        .observe(&Notification::VolumeChanged {
            watch: w,
            previous: volume(0x3333),
            current: volume(0x4444),
        })
        .expect("the violation must not echo into the next change");
}

#[test]
fn an_unchanged_volume_violation_also_advances_state() {
    // Checked by what happens *next*: after the violation the state must hold
    // 0x1111, so a follow-up that does not continue from it is caught. A test
    // that merely accepted a continuous follow-up would pass without the fix
    // too, since an unrecorded `None` state skips the continuity check
    // entirely.
    let mut checker = ContractChecker::new();
    let w = watch();
    assert_eq!(
        checker.observe(&Notification::VolumeChanged {
            watch: w,
            previous: volume(0x1111),
            current: volume(0x1111),
        }),
        Err(ContractViolation::VolumeUnchanged { watch: w })
    );
    assert_eq!(
        checker.observe(&Notification::VolumeChanged {
            watch: w,
            previous: volume(0x9999),
            current: volume(0x2222),
        }),
        Err(ContractViolation::VolumeDiscontinuity { watch: w }),
        "the violated change still recorded its current, so this discontinuity is visible"
    );
}
