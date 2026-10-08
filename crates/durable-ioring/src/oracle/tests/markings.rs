// Copyright (c) 2026 Mike Grier
//! The oracle's marking rules (DI-3.2.4.2), each refused when broken and accepted when kept.

use std::io;

use win_time_sys::TimePoint;

use super::{Oracle, TestEntry, V, completion, op, write_kind};
use crate::FileKey;
use crate::error_code::ErrorCode;
use crate::ids::{FailureId, FailureToken, InstanceId, Lineage, OpId};
use crate::oracle::Violation;
use crate::types::{
    Cause, Entry, Epoch, Failed, ImportScope, Marking, MarkingKind, Outcome, SuspectSet,
    SuspectWrite,
};

/// `ERROR_IO_DEVICE`, the code the failed write here reports.
const IO_DEVICE: u32 = 1117;

fn failure(instance: InstanceId, seq: u64) -> FailureId {
    FailureId { instance, seq }
}

fn suspect(instance: InstanceId, writes: &[u64]) -> SuspectSet<V> {
    SuspectSet::new(
        writes
            .iter()
            .map(|&seq| SuspectWrite {
                op: op(instance, seq),
                file: FileKey(1),
                epoch: Epoch::new(Lineage { instance, seq: 0 }, 1),
            })
            .collect(),
    )
}

fn failed_at(instance: InstanceId, id: u64, writes: &[u64], ticks: u64) -> TestEntry {
    let id = failure(instance, id);
    Entry::Failed(Failed {
        id,
        token: FailureToken::mint(id).0,
        cause: Cause::Imported {
            scope: ImportScope::All,
        },
        suspect: suspect(instance, writes),
        observed: TimePoint::from_ticks(ticks),
    })
}

fn marking(write: OpId, kind: MarkingKind, ticks: u64) -> Marking<V> {
    Marking {
        write,
        observed: TimePoint::from_ticks(ticks),
        kind,
    }
}

fn marked(failure: FailureId, write: OpId, kind: MarkingKind) -> TestEntry {
    Entry::Marked {
        failure,
        marking: marking(write, kind, 0),
    }
}

fn nullified() -> MarkingKind {
    MarkingKind::Nullified {
        code: Some(ErrorCode::from_win32(IO_DEVICE)),
    }
}

/// Writes 0, 1 and 2, tagged 1, pushed; failure 0, stamped at zero, suspecting all three; then
/// write 0 completed failed, write 1 short by a byte, and write 2 in full.
fn three_writes_one_failure(instance: InstanceId) -> Oracle {
    let mut oracle = Oracle::new();
    for seq in 0..3 {
        oracle
            .pushed(op(instance, seq), write_kind(instance, 1), 0)
            .expect("a write");
    }
    oracle
        .observe(&failed_at(instance, 0, &[0, 1, 2], 0))
        .expect("a failure suspecting all three");
    let outcomes = [
        Outcome::Failed(io::Error::from_raw_os_error(IO_DEVICE.cast_signed())),
        Outcome::Transferred(3),
        Outcome::Transferred(4),
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
    for (seq, kind) in [
        (0, nullified()),
        (1, MarkingKind::Short { transferred: 3 }),
        (1, MarkingKind::Covered),
        (2, MarkingKind::Covered),
    ] {
        oracle
            .observe(&marked(f, op(instance, seq), kind))
            .unwrap_or_else(|v| panic!("write {seq} {kind:?}: {v:?}"));
    }
}

#[test]
fn a_marking_contradicting_its_writes_completion_is_refused() {
    let instance = InstanceId::next();
    let mut oracle = three_writes_one_failure(instance);
    let f = failure(instance, 0);
    let wrong_code = MarkingKind::Nullified {
        code: Some(ErrorCode::from_win32(5)),
    };
    for (seq, kind) in [
        (1, nullified()),
        (0, wrong_code),
        (0, MarkingKind::Nullified { code: None }),
        (1, MarkingKind::Short { transferred: 4 }),
        (0, MarkingKind::Short { transferred: 0 }),
        (0, MarkingKind::Covered),
    ] {
        assert!(
            matches!(
                oracle.observe(&marked(f, op(instance, seq), kind)),
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
        .pushed(op(instance, 0), write_kind(instance, 1), 0)
        .expect("a write");
    oracle
        .observe(&failed_at(instance, 0, &[0], 0))
        .expect("a failure suspecting it");
    assert!(matches!(
        oracle.observe(&marked(
            failure(instance, 0),
            op(instance, 0),
            MarkingKind::Covered
        )),
        Err(Violation::MarkingContradicted { .. })
    ));
}

#[test]
fn a_marking_naming_a_write_the_failure_does_not_suspect_is_refused() {
    let instance = InstanceId::next();
    let mut oracle = three_writes_one_failure(instance);
    oracle
        .observe(&failed_at(instance, 1, &[0], 0))
        .expect("a second failure, suspecting write 0 alone");
    assert!(matches!(
        oracle.observe(&marked(
            failure(instance, 1),
            op(instance, 2),
            MarkingKind::Covered
        )),
        Err(Violation::MarkingNotSuspect { .. })
    ));
    oracle
        .observe(&marked(failure(instance, 1), op(instance, 0), nullified()))
        .expect("write 0 it does suspect");
}

#[test]
fn a_marking_for_a_failure_not_live_is_refused() {
    let instance = InstanceId::next();
    let mut oracle = three_writes_one_failure(instance);
    assert!(matches!(
        oracle.observe(&marked(
            failure(instance, 7),
            op(instance, 2),
            MarkingKind::Covered
        )),
        Err(Violation::MarkedUnknown { .. })
    ));
    let ended: TestEntry = Entry::Abandoned {
        failure: failure(instance, 0),
        suspect: suspect(instance, &[0, 1, 2]),
        markings: Vec::new(),
    };
    oracle.observe(&ended).expect("abandoned");
    assert!(matches!(
        oracle.observe(&marked(
            failure(instance, 0),
            op(instance, 2),
            MarkingKind::Covered
        )),
        Err(Violation::MarkedUnknown { .. })
    ));
}

#[test]
fn markings_and_failures_share_one_stamp_order() {
    let instance = InstanceId::next();
    let mut oracle = Oracle::new();
    oracle
        .pushed(op(instance, 0), write_kind(instance, 1), 0)
        .expect("a write");
    oracle
        .observe(&failed_at(instance, 0, &[0], 100))
        .expect("a failure at 100");
    oracle
        .observe(&completion(
            op(instance, 0),
            write_kind(instance, 1),
            Outcome::Transferred(4),
            0,
        ))
        .expect("its completion");
    let covered = |ticks| -> TestEntry {
        Entry::Marked {
            failure: failure(instance, 0),
            marking: marking(op(instance, 0), MarkingKind::Covered, ticks),
        }
    };
    oracle
        .observe(&covered(100))
        .expect("equal to the failure's");
    oracle.observe(&covered(150)).expect("later");
    assert!(matches!(
        oracle.observe(&covered(149)),
        Err(Violation::StampWentBackwards { .. })
    ));
    assert!(
        matches!(
            oracle.observe(&failed_at(instance, 1, &[], 120)),
            Err(Violation::StampWentBackwards { .. })
        ),
        "a failure stamped before the last marking"
    );
}

#[test]
fn a_final_record_carries_exactly_the_markings_reported() {
    let instance = InstanceId::next();
    let f = failure(instance, 0);
    let reported = vec![
        marking(op(instance, 0), nullified(), 0),
        marking(op(instance, 1), MarkingKind::Short { transferred: 3 }, 0),
    ];
    let setup = || {
        let mut oracle = three_writes_one_failure(instance);
        for m in &reported {
            let entry: TestEntry = Entry::Marked {
                failure: f,
                marking: m.clone(),
            };
            oracle.observe(&entry).expect("a marking");
        }
        oracle.healed(f);
        oracle
    };
    let abandoned = |markings: Vec<Marking<V>>| -> TestEntry {
        Entry::Abandoned {
            failure: f,
            suspect: suspect(instance, &[0, 1, 2]),
            markings,
        }
    };
    let healed = |markings: Vec<Marking<V>>| -> TestEntry {
        Entry::Healed {
            failure: f,
            suspect: suspect(instance, &[0, 1, 2]),
            markings,
        }
    };
    let fewer = reported[..1].to_vec();
    let mut more = reported.clone();
    more.push(marking(op(instance, 2), MarkingKind::Covered, 0));
    let mut reordered = reported.clone();
    reordered.reverse();
    for wrong in [fewer, more, reordered] {
        for entry in [abandoned(wrong.clone()), healed(wrong.clone())] {
            assert!(
                matches!(
                    setup().observe(&entry),
                    Err(Violation::MarkingsChanged { .. })
                ),
                "{entry:?}"
            );
        }
    }
    setup()
        .observe(&abandoned(reported.clone()))
        .expect("abandoned with its record");
    setup()
        .observe(&healed(reported.clone()))
        .expect("healed with its record");
}

#[test]
fn healed_is_accepted_once_and_only_for_a_failure_the_consumer_healed() {
    let instance = InstanceId::next();
    let f = failure(instance, 0);
    let healed = |writes: &[u64]| -> TestEntry {
        Entry::Healed {
            failure: f,
            suspect: suspect(instance, writes),
            markings: Vec::new(),
        }
    };
    let mut oracle = three_writes_one_failure(instance);
    assert!(
        matches!(
            oracle.observe(&healed(&[0, 1, 2])),
            Err(Violation::HealedUnknown { .. })
        ),
        "the consumer never healed it"
    );

    let mut oracle = three_writes_one_failure(instance);
    oracle.healed(f);
    assert!(matches!(
        oracle.observe(&healed(&[0, 1])),
        Err(Violation::HealedSuspectChanged { .. })
    ));

    let mut oracle = three_writes_one_failure(instance);
    oracle.healed(f);
    oracle.observe(&healed(&[0, 1, 2])).expect("healed");
    assert!(
        matches!(
            oracle.observe(&healed(&[0, 1, 2])),
            Err(Violation::HealedUnknown { .. })
        ),
        "a failure ends once"
    );
}
