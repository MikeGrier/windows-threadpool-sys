// Copyright (c) 2026 Mike Grier
//! The oracle's marking rules (DI-3.2.4.2, DI-D-113), each refused when broken and accepted when
//! kept: the markings its observer was told of for a failure are checked when the failure is
//! resolved, and at `finish` for those still unresolved.

use std::io;
use std::sync::LazyLock;

use super::{FailedEntry, LEN, Oracle, Report, TestEntry, V, completion, op, write_kind};
use crate::IoTargetKey;
use crate::error_code::ErrorCode;
use crate::ids::{FailureId, FailureToken, InstanceId, Lineage, OpId};
use crate::observer::FailureObserver;
use crate::oracle::Violation;
use crate::types::{
    Entry, Epoch, Failed, FailureCause, ImportScope, IoFault, Marking, MarkingKind, Outcome,
    SuspectWrite,
};

/// The fault the failed write here reports, `ERROR_IO_DEVICE`: one report, shared by its
/// completion and the nullifier that agrees with it.
static IO_DEVICE: LazyLock<IoFault> = LazyLock::new(|| fault(1117));

/// A fault carrying `code`, with a report of its own.
fn fault(code: u32) -> IoFault {
    IoFault {
        source: crate::test_support::SOURCE,
        code: Some(ErrorCode::from_hresult(code.cast_signed())),
        refusal: None,
        error: crate::report_io_error(io::Error::from_raw_os_error(code.cast_signed())),
    }
}

fn failure(instance: InstanceId, seq: u64) -> FailureId {
    FailureId { instance, seq }
}

fn suspect(instance: InstanceId, writes: &[u64]) -> Vec<SuspectWrite<V>> {
    writes
        .iter()
        .map(|&seq| SuspectWrite {
            op: op(instance, seq),
            target: IoTargetKey(1),
            epoch: Epoch::new(Lineage { instance, seq: 0 }, 1),
        })
        .collect()
}

fn failed_at(instance: InstanceId, id: u64, writes: &[u64], ticks: u64) -> FailedEntry {
    let id = failure(instance, id);
    let entry = Entry::Failed(Failed {
        id,
        token: FailureToken::detached(id),
        cause: FailureCause::Imported {
            scope: ImportScope::All,
        },
        observed: ticks,
    });
    FailedEntry(entry, suspect(instance, writes))
}

fn marking(write: OpId, kind: MarkingKind, ticks: u64) -> Marking<V> {
    Marking {
        write,
        observed: ticks,
        kind,
    }
}

/// Tell the oracle's observer of a marking, as the instance does as the write stops being
/// uncovered.
fn tell(oracle: &Oracle, failure: FailureId, write: OpId, kind: MarkingKind, ticks: u64) {
    oracle
        .observer()
        .marked(failure, &marking(write, kind, ticks));
}

/// The entry reporting `failure` abandoned, which is when the oracle checks its markings.
fn abandoned(failure: FailureId) -> TestEntry {
    Entry::Abandoned { failure }
}

fn nullified() -> MarkingKind {
    MarkingKind::Nullified {
        fault: IO_DEVICE.clone(),
    }
}

fn covered() -> MarkingKind {
    MarkingKind::Covered {
        flushed: true,
        provider: false,
    }
}

/// Writes 0, 1 and 2, tagged 1, pushed; failure 0, stamped at zero, suspecting all three; then
/// write 0 completed with the fault, and writes 1 and 2 in full.
fn three_writes_one_failure(instance: InstanceId) -> Oracle {
    let mut oracle = Oracle::new();
    for seq in 0..3 {
        oracle
            .pushed(op(instance, seq), write_kind(instance, 1), LEN, 0)
            .expect("a write");
    }
    oracle
        .report(failed_at(instance, 0, &[0, 1, 2], 0))
        .expect("a failure suspecting all three");
    let outcomes = [
        Outcome::Failed(IO_DEVICE.clone()),
        Outcome::Transferred(LEN),
        Outcome::Transferred(LEN),
    ];
    for (seq, outcome) in (0..).zip(outcomes) {
        oracle
            .observe(&completion(
                op(instance, seq),
                write_kind(instance, 1),
                outcome,
                0,
            ))
            .expect("its completion");
    }
    oracle
}

#[test]
fn markings_that_agree_with_their_writes_completions_are_accepted() {
    let instance = InstanceId::next();
    let mut oracle = three_writes_one_failure(instance);
    let f = failure(instance, 0);
    for (seq, kind) in [(0, nullified()), (1, covered()), (2, covered())] {
        tell(&oracle, f, op(instance, seq), kind, 0);
    }
    oracle
        .observe(&abandoned(f))
        .expect("every marking agrees with its write's completion");
}

#[test]
fn a_marking_contradicting_its_writes_completion_is_refused() {
    let instance = InstanceId::next();
    let f = failure(instance, 0);
    let wrong_code = MarkingKind::Nullified { fault: fault(5) };
    // The same code, but another report than the completion's: a nullifier relays the one report.
    let another_report = MarkingKind::Nullified { fault: fault(1117) };
    for (seq, kind) in [
        (1, nullified()),
        (0, wrong_code),
        (0, another_report),
        (0, covered()),
    ] {
        let mut oracle = three_writes_one_failure(instance);
        tell(&oracle, f, op(instance, seq), kind.clone(), 0);
        assert!(
            matches!(
                oracle.observe(&abandoned(f)),
                Err(Violation::MarkingContradicted { .. })
            ),
            "write {seq} {kind:?}"
        );
    }
}

#[test]
fn a_marking_before_its_writes_completion_is_refused() {
    let instance = InstanceId::next();
    let mut oracle = Oracle::new();
    oracle
        .pushed(op(instance, 0), write_kind(instance, 1), LEN, 0)
        .expect("a write");
    oracle
        .report(failed_at(instance, 0, &[0], 0))
        .expect("a failure suspecting it");
    tell(&oracle, failure(instance, 0), op(instance, 0), covered(), 0);
    assert!(matches!(
        oracle.observe(&abandoned(failure(instance, 0))),
        Err(Violation::MarkingContradicted { .. })
    ));
}

#[test]
fn a_marking_naming_a_write_the_failure_does_not_suspect_is_refused() {
    let instance = InstanceId::next();
    let second = failure(instance, 1);
    let mut oracle = three_writes_one_failure(instance);
    oracle
        .report(failed_at(instance, 1, &[0], 0))
        .expect("a second failure, suspecting write 0 alone");
    tell(&oracle, second, op(instance, 2), covered(), 0);
    assert!(matches!(
        oracle.observe(&abandoned(second)),
        Err(Violation::MarkingNotSuspect { .. })
    ));

    let mut oracle = three_writes_one_failure(instance);
    oracle
        .report(failed_at(instance, 1, &[0], 0))
        .expect("a second failure, suspecting write 0 alone");
    tell(&oracle, second, op(instance, 0), nullified(), 0);
    oracle
        .observe(&abandoned(second))
        .expect("write 0 it does suspect");
}

#[test]
fn the_markings_of_a_failure_still_unresolved_are_checked_at_finish() {
    let instance = InstanceId::next();
    let f = failure(instance, 0);
    let oracle = three_writes_one_failure(instance);
    tell(&oracle, f, op(instance, 2), nullified(), 0);
    assert!(matches!(
        oracle.finish(),
        Err(Violation::MarkingContradicted { .. })
    ));

    let oracle = three_writes_one_failure(instance);
    tell(&oracle, f, op(instance, 2), covered(), 0);
    oracle.finish().expect("it agrees");
}

#[test]
fn a_failure_resolved_is_told_no_more() {
    let instance = InstanceId::next();
    let f = failure(instance, 0);
    let mut oracle = three_writes_one_failure(instance);
    tell(&oracle, f, op(instance, 1), covered(), 0);
    oracle.observe(&abandoned(f)).expect("abandoned");
    tell(&oracle, f, op(instance, 2), covered(), 0);
    assert!(matches!(
        oracle.finish(),
        Err(Violation::MarkedAfterResolution { failure }) if failure == f
    ));

    let mut oracle = three_writes_one_failure(instance);
    tell(&oracle, f, op(instance, 1), covered(), 0);
    oracle.observe(&abandoned(f)).expect("abandoned");
    oracle
        .finish()
        .expect("told of nothing after it was resolved");
}

#[test]
fn markings_and_failures_share_one_stamp_order() {
    let instance = InstanceId::next();
    let f = failure(instance, 0);
    let ready = || {
        let mut oracle = Oracle::new();
        oracle
            .pushed(op(instance, 0), write_kind(instance, 1), LEN, 0)
            .expect("a write");
        oracle
            .report(failed_at(instance, 0, &[0], 100))
            .expect("a failure at 100");
        oracle
            .observe(&completion(
                op(instance, 0),
                write_kind(instance, 1),
                Outcome::Transferred(LEN),
                0,
            ))
            .expect("its completion");
        oracle
    };

    let oracle = ready();
    tell(&oracle, f, op(instance, 0), covered(), 100);
    tell(&oracle, f, op(instance, 0), covered(), 150);
    oracle
        .finish()
        .expect("a marking equal to the failure's, then a later one");

    let oracle = ready();
    tell(&oracle, f, op(instance, 0), covered(), 150);
    tell(&oracle, f, op(instance, 0), covered(), 149);
    assert!(matches!(
        oracle.finish(),
        Err(Violation::StampWentBackwards { .. })
    ));

    let mut oracle = ready();
    tell(&oracle, f, op(instance, 0), covered(), 150);
    assert!(
        matches!(
            oracle.report(failed_at(instance, 1, &[], 120)),
            Err(Violation::StampWentBackwards { .. })
        ),
        "a failure stamped before the last marking"
    );
}

#[test]
fn a_failed_the_observer_was_not_told_of_is_refused() {
    let instance = InstanceId::next();
    let f = failure(instance, 0);
    let FailedEntry(entry, _) = failed_at(instance, 0, &[], 0);
    let mut oracle = Oracle::new();
    assert!(matches!(
        oracle.observe(&entry),
        Err(Violation::FailureNotTold { failure }) if failure == f
    ));
}

#[test]
fn healed_is_accepted_once_and_only_for_a_failure_the_consumer_healed() {
    let instance = InstanceId::next();
    let f = failure(instance, 0);
    let healed: TestEntry = Entry::Healed { failure: f };
    let mut oracle = three_writes_one_failure(instance);
    assert!(
        matches!(
            oracle.observe(&healed),
            Err(Violation::HealedUnknown { .. })
        ),
        "the consumer never healed it"
    );

    let mut oracle = three_writes_one_failure(instance);
    oracle.healed(f);
    oracle.observe(&healed).expect("healed");
    assert!(
        matches!(
            oracle.observe(&healed),
            Err(Violation::HealedUnknown { .. })
        ),
        "a failure ends once"
    );
}
