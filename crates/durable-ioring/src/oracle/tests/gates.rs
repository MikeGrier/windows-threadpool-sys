// Copyright (c) 2026 Mike Grier
//! The oracle's rules for gated operations (DI-3.2.5.2), each refused when broken and accepted when
//! kept.

use super::{LEN, Oracle, TestEntry, V, completion, done, op};
use crate::IoTargetKey;
use crate::ids::{FailureId, FailureToken, InstanceId, Lineage, OpId};
use crate::oracle::Violation;
use crate::types::{
    Entry, Epoch, Failed, FailureCause, ImportScope, OpKind, Outcome, SuspectSet, SuspectWrite,
};

fn at(instance: InstanceId, lineage: u64, id: u64) -> Epoch<V> {
    Epoch::new(
        Lineage {
            instance,
            seq: lineage,
        },
        id,
    )
}

fn write(epoch: Epoch<V>) -> OpKind<V> {
    OpKind::Write { epoch }
}

fn never_issued(id: OpId, kind: OpKind<V>, abandoned: Epoch<V>, context: u32) -> TestEntry {
    completion(id, kind, Outcome::NeverIssued { abandoned }, context)
}

fn durable(through: Epoch<V>) -> TestEntry {
    Entry::Durable { through }
}

fn failed(id: FailureId, writes: &[(OpId, Epoch<V>)]) -> TestEntry {
    Entry::Failed(Failed {
        id,
        token: FailureToken::mint(id).0,
        cause: FailureCause::Imported {
            scope: ImportScope::All,
        },
        suspect: SuspectSet::new(
            writes
                .iter()
                .map(|&(op, epoch)| SuspectWrite {
                    op,
                    target: IoTargetKey(1),
                    epoch,
                })
                .collect(),
        ),
        observed: 0,
    })
}

fn abandoned(id: FailureId, writes: &[(OpId, Epoch<V>)]) -> TestEntry {
    let Entry::Failed(failed) = failed(id, writes) else {
        unreachable!()
    };
    Entry::Abandoned {
        failure: id,
        suspect: failed.suspect,
        markings: Vec::new(),
    }
}

#[test]
fn a_gated_operation_completes_only_once_its_gate_is_reported_durable() {
    let instance = InstanceId::next();
    let gate = at(instance, 1, 4);
    let kind = write(at(instance, 0, 2));
    let mut oracle = Oracle::new();
    oracle
        .pushed_gated(op(instance, 0), kind, LEN, 0, gate)
        .expect("a gated write");
    assert!(matches!(
        oracle.observe(&done(op(instance, 0), kind, 0)),
        Err(Violation::ReleasedEarly { gate: g, .. }) if g == gate
    ));

    let mut oracle = Oracle::new();
    oracle
        .pushed_gated(op(instance, 0), OpKind::Read, LEN, 0, gate)
        .expect("a gated read");
    oracle.sealed(at(instance, 1, 3));
    oracle.observe(&durable(at(instance, 1, 3))).expect("3");
    assert!(
        matches!(
            oracle.observe(&done(op(instance, 0), OpKind::Read, 0)),
            Err(Violation::ReleasedEarly { .. })
        ),
        "3 is below the gate"
    );

    let mut oracle = Oracle::new();
    oracle
        .pushed_gated(op(instance, 0), kind, LEN, 0, gate)
        .expect("a gated write");
    oracle.sealed(at(instance, 1, 5));
    oracle.observe(&durable(at(instance, 1, 5))).expect("5");
    oracle
        .observe(&done(op(instance, 0), kind, 0))
        .expect("released once its gate's lineage passed it");
    oracle.finish().expect("nothing outstanding");
}

#[test]
fn never_issued_comes_only_for_a_gated_operation() {
    let instance = InstanceId::next();
    let mut oracle = Oracle::new();
    oracle
        .pushed(op(instance, 0), OpKind::Read, LEN, 0)
        .expect("an ungated read");
    assert!(matches!(
        oracle.observe(&never_issued(
            op(instance, 0),
            OpKind::Read,
            at(instance, 1, 1),
            0
        )),
        Err(Violation::NeverIssuedUngated { .. })
    ));
}

#[test]
fn never_issued_names_its_gate_or_own_epoch_once_reported_abandoned() {
    let instance = InstanceId::next();
    let gate = at(instance, 1, 4);
    let own = at(instance, 0, 2);
    let suspect = op(instance, 9);
    let failure = FailureId { instance, seq: 0 };
    // The oracle as it stands once write 9, of `epoch`, has been suspected and abandoned.
    let setup = |epoch: Epoch<V>| {
        let mut oracle = Oracle::new();
        oracle
            .pushed_gated(op(instance, 0), write(own), LEN, 0, gate)
            .expect("a gated write");
        oracle
            .pushed(suspect, write(epoch), LEN, 9)
            .expect("a write");
        oracle
            .observe(&failed(failure, &[(suspect, epoch)]))
            .expect("observed");
        oracle
            .observe(&abandoned(failure, &[(suspect, epoch)]))
            .expect("abandoned");
        oracle
    };

    let mut oracle = setup(gate);
    oracle
        .observe(&never_issued(op(instance, 0), write(own), gate, 0))
        .expect("its gate, abandoned");

    let mut oracle = setup(own);
    oracle
        .observe(&never_issued(op(instance, 0), write(own), own, 0))
        .expect("its own epoch, abandoned");

    let mut oracle = setup(own);
    assert!(
        matches!(
            oracle.observe(&never_issued(op(instance, 0), write(own), gate, 0)),
            Err(Violation::NeverIssuedNotAbandoned { abandoned: a, .. }) if a == gate
        ),
        "its gate, which was not abandoned"
    );

    let unrelated = at(instance, 1, 7);
    let mut oracle = setup(unrelated);
    assert!(
        matches!(
            oracle.observe(&never_issued(op(instance, 0), write(own), unrelated, 0)),
            Err(Violation::NeverIssuedNotAbandoned { .. })
        ),
        "abandoned, but neither its gate nor its own"
    );
}

#[test]
fn never_issued_may_name_a_gate_its_lineage_ended_before_reaching() {
    let instance = InstanceId::next();
    let ended = Lineage { instance, seq: 1 };
    let mut oracle = Oracle::new();
    oracle
        .pushed_gated(op(instance, 0), OpKind::Read, LEN, 0, at(instance, 1, 4))
        .expect("a gated read");
    oracle
        .pushed_gated(op(instance, 1), OpKind::Read, LEN, 1, at(instance, 1, 2))
        .expect("another");
    oracle.sealed(at(instance, 1, 2));
    oracle.observe(&durable(at(instance, 1, 2))).expect("2");
    let end: TestEntry = Entry::LineageEnded {
        lineage: ended,
        abandoned_through: Some(4),
    };
    oracle.observe(&end).expect("ends");
    oracle
        .observe(&never_issued(
            op(instance, 0),
            OpKind::Read,
            at(instance, 1, 4),
            0,
        ))
        .expect("4 was above the mark when the lineage ended");
    assert!(
        matches!(
            oracle.observe(&never_issued(
                op(instance, 1),
                OpKind::Read,
                at(instance, 1, 2),
                1
            )),
            Err(Violation::NeverIssuedNotAbandoned { .. })
        ),
        "2 was durable before the end"
    );
}

#[test]
fn no_failure_suspects_a_gated_write_before_its_gate_is_reported_durable() {
    let instance = InstanceId::next();
    let gate = at(instance, 1, 4);
    let own = at(instance, 0, 2);
    let held = op(instance, 0);
    let mut oracle = Oracle::new();
    oracle
        .pushed_gated(held, write(own), LEN, 0, gate)
        .expect("a gated write");
    assert!(matches!(
        oracle.observe(&failed(FailureId { instance, seq: 0 }, &[(held, own)])),
        Err(Violation::SuspectStillHeld { op, .. }) if op == held
    ));
    oracle.sealed(gate);
    oracle.observe(&durable(gate)).expect("the gate");
    oracle
        .observe(&failed(FailureId { instance, seq: 1 }, &[(held, own)]))
        .expect("released, so it may be in flight");
}
