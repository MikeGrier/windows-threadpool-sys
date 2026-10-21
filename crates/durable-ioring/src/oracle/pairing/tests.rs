// Copyright (c) 2026 Mike Grier
//! The pairing rule (DI-19.2.1): each rule shown to accept a stream that keeps it and to refuse
//! one that breaks it.

use super::{PairingChecker, PairingViolation};
use crate::correlation::CorrelationId;
use crate::diagnostics::{Diagnostic, DiagnosticKind, Edge, FlushOutcome, FlushPart, SealOutcome};
use crate::types::{FlushDomain, IoTargetKey};

fn id(n: u64) -> CorrelationId {
    CorrelationId::chosen(n)
}

/// Feed one event, built in place: `fmt::Arguments` borrows temporaries the statement holds.
macro_rules! feed {
    ($checker:expr, $event:expr) => {
        $checker.observe(&$event)
    };
}

fn seal_began(
    checker: &mut PairingChecker,
    n: u64,
    lineage: u64,
    through: u64,
) -> Result<(), PairingViolation> {
    feed!(
        checker,
        Diagnostic::SealBegan {
            instance: 1,
            lineage,
            through: format_args!("{through}"),
            id: id(n)
        }
    )
}

fn seal_ended(
    checker: &mut PairingChecker,
    n: u64,
    lineage: u64,
    through: u64,
) -> Result<(), PairingViolation> {
    feed!(
        checker,
        Diagnostic::SealEnded {
            instance: 1,
            lineage,
            through: format_args!("{through}"),
            id: id(n),
            outcome: SealOutcome::Durable
        }
    )
}

fn flush_began(checker: &mut PairingChecker, n: u64, file: u64) -> Result<(), PairingViolation> {
    feed!(
        checker,
        Diagnostic::FlushBegan {
            instance: 1,
            lineage: 0,
            through: format_args!("3"),
            part: FlushPart::File(IoTargetKey(file)),
            id: id(n)
        }
    )
}

fn flush_ended(checker: &mut PairingChecker, n: u64, file: u64) -> Result<(), PairingViolation> {
    feed!(
        checker,
        Diagnostic::FlushEnded {
            instance: 1,
            lineage: 0,
            through: format_args!("3"),
            part: FlushPart::File(IoTargetKey(file)),
            id: id(n),
            outcome: FlushOutcome::Succeeded
        }
    )
}

fn lock_began(
    checker: &mut PairingChecker,
    n: u64,
    lock: &'static str,
) -> Result<(), PairingViolation> {
    feed!(
        checker,
        Diagnostic::LockWaitBegan {
            instance: 1,
            lock,
            id: id(n)
        }
    )
}

fn lock_ended(
    checker: &mut PairingChecker,
    n: u64,
    lock: &'static str,
) -> Result<(), PairingViolation> {
    feed!(
        checker,
        Diagnostic::LockWaitEnded {
            instance: 1,
            lock,
            id: id(n)
        }
    )
}

#[test]
fn a_begin_then_its_end_is_accepted_for_every_kind() {
    let mut checker = PairingChecker::new();
    seal_began(&mut checker, 1, 0, 5).expect("a begin");
    seal_ended(&mut checker, 1, 0, 5).expect("its end");
    flush_began(&mut checker, 2, 7).expect("a begin");
    flush_ended(&mut checker, 2, 7).expect("its end");
    lock_began(&mut checker, 3, "engine").expect("a begin");
    lock_ended(&mut checker, 3, "engine").expect("its end");
    checker.finish(&[]).expect("nothing open");
}

#[test]
fn every_outcome_ends_a_pair() {
    let mut checker = PairingChecker::new();
    for (n, outcome) in [
        SealOutcome::Durable,
        SealOutcome::Failed,
        SealOutcome::Blocked,
        SealOutcome::Ended,
    ]
    .into_iter()
    .enumerate()
    {
        let n = n as u64 + 1;
        seal_began(&mut checker, n, 0, n).expect("a begin");
        feed!(
            checker,
            Diagnostic::SealEnded {
                instance: 1,
                lineage: 0,
                through: format_args!("{n}"),
                id: id(n),
                outcome
            }
        )
        .expect("its end");
    }
    let domain = FlushDomain::new(b"log".as_slice());
    for (n, outcome) in [
        FlushOutcome::Succeeded,
        FlushOutcome::Failed,
        FlushOutcome::Abandoned,
    ]
    .into_iter()
    .enumerate()
    {
        let n = n as u64 + 10;
        feed!(
            checker,
            Diagnostic::FlushBegan {
                instance: 1,
                lineage: 0,
                through: format_args!("1"),
                part: FlushPart::Domain(&domain),
                id: id(n)
            }
        )
        .expect("a begin");
        feed!(
            checker,
            Diagnostic::FlushEnded {
                instance: 1,
                lineage: 0,
                through: format_args!("1"),
                part: FlushPart::Domain(&domain),
                id: id(n),
                outcome
            }
        )
        .expect("its end");
    }
    checker.finish(&[]).expect("nothing open");
}

#[test]
fn pairs_that_interleave_or_nest_are_accepted() {
    let mut checker = PairingChecker::new();
    seal_began(&mut checker, 1, 0, 1).expect("A begins");
    flush_began(&mut checker, 2, 1).expect("B begins");
    flush_began(&mut checker, 3, 2).expect("C begins");
    flush_ended(&mut checker, 2, 1).expect("B ends");
    lock_began(&mut checker, 4, "engine").expect("D begins");
    seal_ended(&mut checker, 1, 0, 1).expect("A ends before C");
    lock_ended(&mut checker, 4, "engine").expect("D ends");
    flush_ended(&mut checker, 3, 2).expect("C ends last");
    checker.finish(&[]).expect("nothing open");
}

#[test]
fn many_pairs_ended_in_reverse_are_accepted() {
    let mut checker = PairingChecker::new();
    for n in 1..=50 {
        flush_began(&mut checker, n, n).expect("a begin");
    }
    assert_eq!(checker.open().len(), 50);
    for n in (1..=50).rev() {
        flush_ended(&mut checker, n, n).expect("its end");
    }
    assert!(checker.open().is_empty());
    checker.finish(&[]).expect("nothing open");
}

#[test]
fn an_event_that_is_no_pair_is_passed_over() {
    let mut checker = PairingChecker::new();
    feed!(
        checker,
        Diagnostic::Inconsistency {
            instance: 1,
            what: "x",
            detail: format_args!("y")
        }
    )
    .expect("no pair");
    seal_began(&mut checker, 1, 0, 1).expect("a begin");
    feed!(
        checker,
        Diagnostic::LockWait {
            instance: 1,
            lock: "engine",
            waited: std::time::Duration::ZERO
        }
    )
    .expect("no pair");
    seal_ended(&mut checker, 1, 0, 1).expect("its end");
    checker.finish(&[]).expect("nothing open");
}

#[test]
fn an_end_with_no_begin_is_refused() {
    let mut checker = PairingChecker::new();
    assert_eq!(
        flush_ended(&mut checker, 9, 1),
        Err(PairingViolation::EndWithoutBegin {
            id: id(9),
            kind: DiagnosticKind::Flush
        })
    );
}

#[test]
fn an_id_begun_twice_is_refused() {
    let mut checker = PairingChecker::new();
    seal_began(&mut checker, 4, 0, 1).expect("a begin");
    assert_eq!(
        seal_began(&mut checker, 4, 0, 1),
        Err(PairingViolation::BegunTwice { id: id(4) })
    );
}

#[test]
fn the_same_id_begun_again_by_another_kind_is_refused_too() {
    let mut checker = PairingChecker::new();
    seal_began(&mut checker, 4, 0, 1).expect("a begin");
    assert_eq!(
        lock_began(&mut checker, 4, "engine"),
        Err(PairingViolation::BegunTwice { id: id(4) })
    );
}

#[test]
fn an_end_of_another_kind_is_refused() {
    let mut checker = PairingChecker::new();
    seal_began(&mut checker, 5, 0, 1).expect("a begin");
    assert_eq!(
        lock_ended(&mut checker, 5, "engine"),
        Err(PairingViolation::KindMismatch {
            id: id(5),
            began: DiagnosticKind::Seal,
            ended: DiagnosticKind::LockWait
        })
    );
}

#[test]
fn an_end_that_does_not_repeat_its_begins_identity_is_refused() {
    for (began, ended) in [((0, 5), (1, 5)), ((0, 5), (0, 6))] {
        let mut checker = PairingChecker::new();
        seal_began(&mut checker, 1, began.0, began.1).expect("a begin");
        assert!(matches!(
            seal_ended(&mut checker, 1, ended.0, ended.1),
            Err(PairingViolation::SubjectMismatch { .. })
        ));
    }
    let mut checker = PairingChecker::new();
    flush_began(&mut checker, 2, 7).expect("a begin");
    assert!(
        matches!(
            flush_ended(&mut checker, 2, 8),
            Err(PairingViolation::SubjectMismatch { .. })
        ),
        "another file"
    );
    let mut checker = PairingChecker::new();
    lock_began(&mut checker, 3, "engine").expect("a begin");
    assert!(
        matches!(
            lock_ended(&mut checker, 3, "slot"),
            Err(PairingViolation::SubjectMismatch { .. })
        ),
        "another lock"
    );
    let mut checker = PairingChecker::new();
    feed!(
        checker,
        Diagnostic::LockWaitBegan {
            instance: 1,
            lock: "engine",
            id: id(4)
        }
    )
    .expect("a begin");
    assert!(
        matches!(
            feed!(
                checker,
                Diagnostic::LockWaitEnded {
                    instance: 2,
                    lock: "engine",
                    id: id(4)
                }
            ),
            Err(PairingViolation::SubjectMismatch { .. })
        ),
        "another instance"
    );
}

#[test]
fn an_end_that_repeats_its_begins_identity_by_other_means_is_accepted() {
    let mut checker = PairingChecker::new();
    let five = 5_u64;
    feed!(
        checker,
        Diagnostic::SealBegan {
            instance: 1,
            lineage: 0,
            through: format_args!("{five}"),
            id: id(1)
        }
    )
    .expect("a begin");
    feed!(
        checker,
        Diagnostic::SealEnded {
            instance: 1,
            lineage: 0,
            through: format_args!("5"),
            id: id(1),
            outcome: SealOutcome::Failed
        }
    )
    .expect("the same text");
}

#[test]
fn a_second_end_is_refused() {
    let mut checker = PairingChecker::new();
    lock_began(&mut checker, 6, "engine").expect("a begin");
    lock_ended(&mut checker, 6, "engine").expect("its end");
    assert_eq!(
        lock_ended(&mut checker, 6, "engine"),
        Err(PairingViolation::AfterEnd {
            id: id(6),
            edge: Edge::End
        })
    );
}

#[test]
fn a_begin_after_its_pair_ended_is_refused() {
    let mut checker = PairingChecker::new();
    lock_began(&mut checker, 6, "engine").expect("a begin");
    lock_ended(&mut checker, 6, "engine").expect("its end");
    assert_eq!(
        lock_began(&mut checker, 6, "engine"),
        Err(PairingViolation::AfterEnd {
            id: id(6),
            edge: Edge::Begin
        })
    );
}

#[test]
fn a_pair_left_open_is_refused_at_the_finish() {
    let mut checker = PairingChecker::new();
    seal_began(&mut checker, 2, 0, 1).expect("a begin");
    flush_began(&mut checker, 1, 1).expect("a begin");
    flush_began(&mut checker, 3, 2).expect("a begin");
    flush_ended(&mut checker, 3, 2).expect("an end");
    assert_eq!(checker.open(), [id(1), id(2)]);
    assert_eq!(
        checker.finish(&[]),
        Err(PairingViolation::LeftOpen {
            ids: vec![id(1), id(2)]
        })
    );
}

#[test]
fn a_pair_named_as_left_open_is_accepted_at_the_finish() {
    let mut checker = PairingChecker::new();
    seal_began(&mut checker, 2, 0, 1).expect("a begin");
    flush_began(&mut checker, 1, 1).expect("a begin");
    checker.finish(&[id(1), id(2)]).expect("both named");
}

#[test]
fn a_pair_named_but_not_all_that_are_open_is_refused() {
    let mut checker = PairingChecker::new();
    seal_began(&mut checker, 2, 0, 1).expect("a begin");
    flush_began(&mut checker, 1, 1).expect("a begin");
    assert_eq!(
        checker.finish(&[id(1)]),
        Err(PairingViolation::LeftOpen { ids: vec![id(2)] })
    );
}

#[test]
fn naming_a_pair_that_is_not_open_is_refused() {
    let mut checker = PairingChecker::new();
    seal_began(&mut checker, 1, 0, 1).expect("a begin");
    seal_ended(&mut checker, 1, 0, 1).expect("its end");
    assert_eq!(
        PairingChecker::new().finish(&[id(7)]),
        Err(PairingViolation::NamedNotOpen { id: id(7) }),
        "never begun"
    );
    assert_eq!(
        checker.finish(&[id(1)]),
        Err(PairingViolation::NamedNotOpen { id: id(1) }),
        "ended"
    );
}

#[test]
fn an_empty_stream_finishes() {
    PairingChecker::new().finish(&[]).expect("nothing open");
}

#[test]
fn each_violation_says_which_pair() {
    let shown = [
        PairingViolation::EndWithoutBegin {
            id: id(11),
            kind: DiagnosticKind::Flush,
        },
        PairingViolation::BegunTwice { id: id(11) },
        PairingViolation::KindMismatch {
            id: id(11),
            began: DiagnosticKind::Seal,
            ended: DiagnosticKind::Flush,
        },
        PairingViolation::SubjectMismatch {
            id: id(11),
            began: "a".to_owned(),
            ended: "b".to_owned(),
        },
        PairingViolation::AfterEnd {
            id: id(11),
            edge: Edge::End,
        },
        PairingViolation::LeftOpen {
            ids: vec![id(11), id(12)],
        },
        PairingViolation::NamedNotOpen { id: id(11) },
    ];
    for violation in shown {
        let text = violation.to_string();
        assert!(text.contains("11"), "{text}");
    }
}
