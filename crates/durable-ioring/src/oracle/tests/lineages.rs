// Copyright (c) 2026 Mike Grier
//! The oracle's rules for an ended lineage (DI-3.2.5.1), each refused when broken and accepted when
//! kept, and the lineage's neighbours left alone.

use win_time_sys::TimePoint;

use super::{Oracle, TestEntry, V, done, op};
use crate::FileKey;
use crate::ids::{FailureId, FailureToken, InstanceId, Lineage};
use crate::oracle::Violation;
use crate::types::{
    Cause, Entry, Epoch, Failed, ImportScope, Marking, MarkingKind, OpKind, SuspectSet,
    SuspectWrite,
};

/// Lineage `seq` of `instance`.
fn lineage(instance: InstanceId, seq: u64) -> Lineage {
    Lineage { instance, seq }
}

fn ended(lineage: Lineage) -> TestEntry {
    Entry::LineageEnded {
        lineage,
        abandoned_through: Some(4),
    }
}

fn write(lineage: Lineage, id: u64) -> OpKind<V> {
    OpKind::Write {
        epoch: Epoch::new(lineage, id),
    }
}

fn failed(id: FailureId, writes: &[(crate::ids::OpId, Epoch<V>)]) -> TestEntry {
    Entry::Failed(Failed {
        id,
        token: FailureToken::mint(id).0,
        cause: Cause::Imported {
            scope: ImportScope::All,
        },
        suspect: SuspectSet::new(
            writes
                .iter()
                .map(|&(op, epoch)| SuspectWrite {
                    op,
                    file: FileKey(1),
                    epoch,
                })
                .collect(),
        ),
        observed: TimePoint::from_ticks(0),
    })
}

#[test]
fn an_ended_lineage_is_reported_ended_once() {
    let instance = InstanceId::next();
    let mut oracle = Oracle::new();
    oracle
        .observe(&ended(lineage(instance, 1)))
        .expect("the first end");
    oracle
        .observe(&ended(lineage(instance, 2)))
        .expect("another lineage's end");
    assert!(matches!(
        oracle.observe(&ended(lineage(instance, 1))),
        Err(Violation::LineageEndedTwice { lineage: l }) if l == lineage(instance, 1)
    ));
}

#[test]
fn no_seal_of_an_ended_lineage_is_answered_and_its_neighbours_still_are() {
    let instance = InstanceId::next();
    let (gone, kept) = (lineage(instance, 1), lineage(instance, 2));
    let mut oracle = Oracle::new();
    oracle.sealed(Epoch::new(gone, 3));
    oracle.sealed(Epoch::new(kept, 3));
    oracle.observe(&ended(gone)).expect("ends");

    let durable = |lineage| -> TestEntry {
        Entry::Durable {
            through: Epoch::new(lineage, 3),
        }
    };
    assert!(matches!(
        oracle.observe(&durable(gone)),
        Err(Violation::AnsweredAfterEnd { .. })
    ));
    let blocked: TestEntry = Entry::Blocked {
        through: Epoch::new(gone, 3),
        by: FailureId { instance, seq: 0 },
    };
    assert!(matches!(
        oracle.observe(&blocked),
        Err(Violation::AnsweredAfterEnd { .. })
    ));
    oracle
        .observe(&durable(kept))
        .expect("a lineage that did not end");
}

#[test]
fn no_write_into_an_ended_lineage_is_accepted_and_one_in_flight_still_completes() {
    let instance = InstanceId::next();
    let (gone, kept) = (lineage(instance, 1), lineage(instance, 2));
    let mut oracle = Oracle::new();
    oracle
        .pushed(op(instance, 0), write(gone, 5), 0)
        .expect("before the end");
    oracle.observe(&ended(gone)).expect("ends");
    assert!(matches!(
        oracle.pushed(op(instance, 1), write(gone, 9), 1),
        Err(Violation::WriteToEndedLineage { .. })
    ));
    oracle
        .pushed(op(instance, 2), write(kept, 9), 2)
        .expect("a lineage that did not end");
    oracle
        .observe(&done(op(instance, 0), write(gone, 5), 0))
        .expect("a write in flight at the end completes");
    oracle
        .observe(&done(op(instance, 2), write(kept, 9), 2))
        .expect("its neighbour's");
    oracle.finish().expect("nothing outstanding");
}

#[test]
fn no_failure_after_the_end_suspects_the_lineage_but_one_before_is_still_marked() {
    let instance = InstanceId::next();
    let (gone, kept) = (lineage(instance, 1), lineage(instance, 2));
    let w_gone = (op(instance, 0), Epoch::new(gone, 5));
    let w_kept = (op(instance, 1), Epoch::new(kept, 5));
    let mut oracle = Oracle::new();
    oracle
        .pushed(w_gone.0, write(gone, 5), 0)
        .expect("a write of the lineage");
    oracle
        .pushed(w_kept.0, write(kept, 5), 1)
        .expect("a write of its neighbour");
    let before = FailureId { instance, seq: 0 };
    oracle
        .observe(&failed(before, &[w_gone, w_kept]))
        .expect("a failure before the end");
    oracle.observe(&ended(gone)).expect("ends");

    assert!(matches!(
        oracle.observe(&failed(FailureId { instance, seq: 1 }, &[w_gone])),
        Err(Violation::SuspectOfEndedLineage { op, .. }) if op == w_gone.0
    ));
    oracle
        .observe(&failed(FailureId { instance, seq: 2 }, &[w_kept]))
        .expect("a failure of its neighbour");

    oracle
        .observe(&done(w_gone.0, write(gone, 5), 0))
        .expect("the write in flight completes");
    let marked: TestEntry = Entry::Marked {
        failure: before,
        marking: Marking {
            write: w_gone.0,
            observed: TimePoint::from_ticks(0),
            kind: MarkingKind::Covered,
        },
    };
    oracle
        .observe(&marked)
        .expect("the failure suspecting it before the end is marked");
}
