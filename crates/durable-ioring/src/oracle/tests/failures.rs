// Copyright (c) 2026 Mike Grier
//! The oracle's failure rules (DI-3.2.4), each refused when broken and accepted when kept.

use super::{Oracle, TestEntry, V, done, epoch, op, write_kind};
use crate::FileKey;
use crate::ids::{FailureId, FailureToken, InstanceId, OpId};
use crate::oracle::Violation;
use crate::types::{Cause, Entry, Epoch, Failed, ImportScope, SuspectSet, SuspectWrite};

fn failure(instance: InstanceId, seq: u64) -> FailureId {
    FailureId { instance, seq }
}

fn suspect(writes: &[(OpId, Epoch<V>)]) -> SuspectSet<V> {
    SuspectSet::new(
        writes
            .iter()
            .map(|&(op, epoch)| SuspectWrite {
                op,
                file: FileKey(1),
                epoch,
            })
            .collect(),
    )
}

fn failed(id: FailureId, writes: &[(OpId, Epoch<V>)]) -> TestEntry {
    Entry::Failed(Failed {
        id,
        token: FailureToken::mint(id).0,
        cause: Cause::Imported {
            scope: ImportScope::All,
        },
        suspect: suspect(writes),
    })
}

fn abandoned(id: FailureId, writes: &[(OpId, Epoch<V>)]) -> TestEntry {
    Entry::Abandoned {
        failure: id,
        suspect: suspect(writes),
    }
}

/// An oracle that has seen write 0 tagged 1 and write 1 tagged 3 accepted, the first completed.
fn two_writes(instance: InstanceId) -> Oracle {
    let mut oracle = Oracle::new();
    oracle
        .pushed(op(instance, 0), write_kind(instance, 1), 0)
        .expect("a write");
    oracle
        .pushed(op(instance, 1), write_kind(instance, 3), 1)
        .expect("a write");
    oracle
        .observe(&done(op(instance, 0), write_kind(instance, 1), 0))
        .expect("its completion");
    oracle
}

#[test]
fn a_failure_suspecting_accepted_writes_in_push_order_is_accepted_and_others_are_not() {
    let instance = InstanceId::next();
    let w0 = (op(instance, 0), epoch(instance, 1));
    let w1 = (op(instance, 1), epoch(instance, 3));

    let mut oracle = two_writes(instance);
    oracle
        .observe(&failed(failure(instance, 0), &[w0, w1]))
        .expect("a completed write and one in flight");
    oracle
        .observe(&failed(failure(instance, 1), &[]))
        .expect("an empty suspect set");
    assert!(matches!(
        oracle.observe(&failed(failure(instance, 0), &[])),
        Err(Violation::DuplicateFailure { .. })
    ));

    let unknown = (op(instance, 9), epoch(instance, 1));
    let retagged = (op(instance, 0), epoch(instance, 2));
    for (seq, writes) in [(2, vec![unknown]), (3, vec![retagged])] {
        assert!(
            matches!(
                oracle.observe(&failed(failure(instance, seq), &writes)),
                Err(Violation::SuspectNotPushed { .. })
            ),
            "{writes:?}"
        );
    }
    assert!(matches!(
        oracle.observe(&failed(failure(instance, 4), &[w1, w0])),
        Err(Violation::SuspectsOutOfOrder { .. })
    ));
}

#[test]
fn a_failure_reaching_back_past_a_reported_durable_is_a_violation() {
    let instance = InstanceId::next();
    let mut oracle = two_writes(instance);
    oracle.sealed(epoch(instance, 2));
    let durable: TestEntry = Entry::Durable {
        through: epoch(instance, 2),
    };
    oracle.observe(&durable).expect("durable through 2");
    let w0 = (op(instance, 0), epoch(instance, 1));
    let w1 = (op(instance, 1), epoch(instance, 3));
    assert!(matches!(
        oracle.observe(&failed(failure(instance, 0), &[w0])),
        Err(Violation::SuspectAlreadyDurable { .. })
    ));
    oracle
        .observe(&failed(failure(instance, 1), &[w1]))
        .expect("3 is above the mark");
}

#[test]
fn durable_past_a_failure_is_a_violation_until_it_is_healed_or_abandoned() {
    let instance = InstanceId::next();
    let w0 = (op(instance, 0), epoch(instance, 1));
    let durable: TestEntry = Entry::Durable {
        through: epoch(instance, 2),
    };
    let setup = || {
        let mut oracle = two_writes(instance);
        oracle.sealed(epoch(instance, 2));
        oracle
            .observe(&failed(failure(instance, 0), &[w0]))
            .expect("a failure holding 1");
        oracle
    };

    let mut oracle = setup();
    assert!(matches!(
        oracle.observe(&durable),
        Err(Violation::DurableThroughFailure { .. })
    ));

    let mut oracle = setup();
    oracle.healed(failure(instance, 0));
    oracle.observe(&durable).expect("healed");

    let mut oracle = setup();
    oracle
        .observe(&abandoned(failure(instance, 0), &[w0]))
        .expect("abandoned");
    oracle.observe(&durable).expect("abandoned");

    let mut oracle = two_writes(instance);
    oracle.sealed(epoch(instance, 2));
    let w1 = (op(instance, 1), epoch(instance, 3));
    oracle
        .observe(&failed(failure(instance, 0), &[w1]))
        .expect("a failure holding 3 alone");
    oracle.observe(&durable).expect("3 is above 2");
}

#[test]
fn abandoned_must_name_a_live_failure_with_its_reported_suspect_set() {
    let instance = InstanceId::next();
    let w0 = (op(instance, 0), epoch(instance, 1));
    let w1 = (op(instance, 1), epoch(instance, 3));
    let mut oracle = two_writes(instance);
    assert!(matches!(
        oracle.observe(&abandoned(failure(instance, 0), &[w0])),
        Err(Violation::AbandonedUnknown { .. })
    ));
    oracle
        .observe(&failed(failure(instance, 0), &[w0, w1]))
        .expect("a failure");
    oracle
        .observe(&failed(failure(instance, 1), &[w0, w1]))
        .expect("another");
    assert!(matches!(
        oracle.observe(&abandoned(failure(instance, 0), &[w0])),
        Err(Violation::AbandonedSuspectChanged { .. })
    ));
    oracle
        .observe(&abandoned(failure(instance, 1), &[w0, w1]))
        .expect("as reported");
    assert!(matches!(
        oracle.observe(&abandoned(failure(instance, 1), &[w0, w1])),
        Err(Violation::AbandonedUnknown { .. })
    ));
}

#[test]
fn a_write_into_an_abandoned_epoch_is_a_violation_and_one_into_another_epoch_is_not() {
    let instance = InstanceId::next();
    let w0 = (op(instance, 0), epoch(instance, 1));
    let w1 = (op(instance, 1), epoch(instance, 3));
    let mut oracle = two_writes(instance);
    oracle
        .observe(&failed(failure(instance, 0), &[w0, w1]))
        .expect("a failure");
    oracle
        .pushed(op(instance, 2), write_kind(instance, 3), 2)
        .expect("3 is not abandoned until the entry says so");
    oracle
        .observe(&abandoned(failure(instance, 0), &[w0, w1]))
        .expect("abandoned");
    assert!(matches!(
        oracle.pushed(op(instance, 3), write_kind(instance, 3), 3),
        Err(Violation::WriteToAbandoned { .. })
    ));
    oracle
        .pushed(op(instance, 4), write_kind(instance, 4), 4)
        .expect("4 was not abandoned");
    let other = InstanceId::next();
    oracle
        .pushed(op(other, 0), write_kind(other, 3), 0)
        .expect("another lineage's epoch 3");
}

#[test]
fn blocked_must_name_a_live_failure_at_or_below_the_request() {
    let instance = InstanceId::next();
    let w0 = (op(instance, 0), epoch(instance, 1));
    let w1 = (op(instance, 1), epoch(instance, 3));
    let blocked = |through, by| -> TestEntry {
        Entry::Blocked {
            through: epoch(instance, through),
            by: failure(instance, by),
        }
    };
    let mut oracle = two_writes(instance);
    assert!(matches!(
        oracle.observe(&blocked(2, 0)),
        Err(Violation::BlockedByUnrelated { .. })
    ));
    oracle
        .observe(&failed(failure(instance, 0), &[w0]))
        .expect("holds 1");
    oracle
        .observe(&failed(failure(instance, 1), &[w1]))
        .expect("holds 3");
    oracle.observe(&blocked(2, 0)).expect("1 is at or below 2");
    assert!(matches!(
        oracle.observe(&blocked(2, 1)),
        Err(Violation::BlockedByUnrelated { .. })
    ));
    oracle.healed(failure(instance, 0));
    oracle
        .observe(&blocked(2, 0))
        .expect("a heal not yet in effect still holds");
    oracle
        .observe(&abandoned(failure(instance, 0), &[w0]))
        .expect("abandoned");
    assert!(matches!(
        oracle.observe(&blocked(2, 0)),
        Err(Violation::BlockedByUnrelated { .. })
    ));
}
