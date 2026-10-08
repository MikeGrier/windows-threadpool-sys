// Copyright (c) 2026 Mike Grier
//! Unit tests for the conformance oracle and the readiness check: each rule both refused when
//! broken and accepted when kept, and the readiness check failing for an implementation that
//! breaks either half of DI-D-28's rule.

use std::io;
use std::time::Duration;

use super::{ConformanceOracle, ReadinessFailure, Violation, check_readiness};
use crate::fake::{Fake, Signals};
use crate::ids::{DioringIds, InstanceId, Lineage, OpId};
use crate::types::{Entry, Epoch, OpCompletion, OpKind, Outcome};

type V = DioringIds<u64>;
type Oracle = ConformanceOracle<V, u32>;
type TestEntry = Entry<V, Vec<u8>, u32>;

/// Generous: the readiness check passing waits for one pool callback.
const BOUND: Duration = Duration::from_secs(5);
/// Spent in full by each failing readiness check, so short.
const FAILING_BOUND: Duration = Duration::from_millis(200);

fn op(instance: InstanceId, seq: u64) -> OpId {
    OpId { instance, seq }
}

fn write_kind(instance: InstanceId, id: u64) -> OpKind<V> {
    OpKind::Write {
        epoch: Epoch::new(Lineage { instance, seq: 0 }, id),
    }
}

fn completion(id: OpId, kind: OpKind<V>, outcome: Outcome<V>, context: u32) -> TestEntry {
    Entry::Op(OpCompletion {
        id,
        kind,
        outcome,
        buffer: None,
        context,
    })
}

fn done(id: OpId, kind: OpKind<V>, context: u32) -> TestEntry {
    completion(id, kind, Outcome::Transferred(4), context)
}

#[test]
fn an_operation_completing_once_with_what_it_was_pushed_with_is_accepted() {
    let instance = InstanceId::next();
    let mut oracle = Oracle::new();
    let id = op(instance, 0);
    oracle
        .pushed(id, write_kind(instance, 1), 7)
        .expect("a first push");
    assert_eq!(oracle.outstanding(), 1);
    oracle
        .observe(&done(id, write_kind(instance, 1), 7))
        .expect("its completion");
    assert_eq!(oracle.outstanding(), 0);
    oracle.finish().expect("nothing outstanding");
}

#[test]
fn completions_in_any_order_are_accepted() {
    let instance = InstanceId::next();
    let mut oracle = Oracle::new();
    for seq in 0..4 {
        oracle
            .pushed(op(instance, seq), OpKind::Read, seq as u32)
            .expect("a push");
    }
    for seq in [2, 0, 3, 1] {
        oracle
            .observe(&done(op(instance, seq), OpKind::Read, seq as u32))
            .expect("completions in an order other than the pushes'");
    }
    oracle.finish().expect("all four completed");
}

#[test]
fn short_zero_and_failed_outcomes_are_completions() {
    let instance = InstanceId::next();
    let mut oracle = Oracle::new();
    let outcomes = [
        Outcome::Transferred(1),
        Outcome::Transferred(0),
        Outcome::Failed(io::Error::from(io::ErrorKind::PermissionDenied)),
    ];
    for (seq, outcome) in outcomes.into_iter().enumerate() {
        let id = op(instance, seq as u64);
        oracle.pushed(id, OpKind::Read, 0).expect("a push");
        oracle
            .observe(&completion(id, OpKind::Read, outcome, 0))
            .expect("any outcome is a completion");
    }
    oracle.finish().expect("all completed");
}

#[test]
fn entries_later_steps_define_are_accepted_unexamined() {
    let instance = InstanceId::next();
    let mut oracle = Oracle::new();
    let ended: TestEntry = Entry::LineageEnded {
        lineage: Lineage { instance, seq: 1 },
        abandoned_through: Some(3),
    };
    oracle.observe(&ended).expect("a LineageEnded entry");
    oracle.finish().expect("no operations");
}

#[test]
fn a_completion_no_push_returned_is_refused() {
    let instance = InstanceId::next();
    let mut oracle = Oracle::new();
    let id = op(instance, 9);
    assert!(matches!(
        oracle.observe(&done(id, OpKind::Read, 0)),
        Err(Violation::NeverPushed { op }) if op == id
    ));
}

#[test]
fn a_second_completion_is_refused() {
    let instance = InstanceId::next();
    let mut oracle = Oracle::new();
    let id = op(instance, 0);
    oracle.pushed(id, OpKind::Read, 1).expect("a push");
    oracle
        .observe(&done(id, OpKind::Read, 1))
        .expect("the first completion");
    assert!(matches!(
        oracle.observe(&done(id, OpKind::Read, 1)),
        Err(Violation::CompletedTwice { op }) if op == id
    ));
}

#[test]
fn a_completion_of_another_kind_is_refused() {
    let instance = InstanceId::next();
    let mut oracle = Oracle::new();
    let id = op(instance, 0);
    oracle
        .pushed(id, write_kind(instance, 5), 1)
        .expect("a write");
    let reported = write_kind(instance, 6);
    assert!(matches!(
        oracle.observe(&done(id, reported, 1)),
        Err(Violation::KindChanged { op, pushed, completed })
            if op == id && pushed == write_kind(instance, 5) && completed == reported
    ));
    let read = op(instance, 1);
    oracle.pushed(read, OpKind::Read, 2).expect("a read");
    assert!(matches!(
        oracle.observe(&done(read, write_kind(instance, 1), 2)),
        Err(Violation::KindChanged { .. })
    ));
}

#[test]
fn a_completion_with_another_context_is_refused() {
    let instance = InstanceId::next();
    let mut oracle = Oracle::new();
    let id = op(instance, 0);
    oracle.pushed(id, OpKind::Read, 1).expect("a push");
    assert!(matches!(
        oracle.observe(&done(id, OpKind::Read, 2)),
        Err(Violation::ContextChanged { op }) if op == id
    ));
}

#[test]
fn an_identity_returned_twice_is_refused_while_outstanding_and_after_completing() {
    let instance = InstanceId::next();
    let mut oracle = Oracle::new();
    let id = op(instance, 0);
    oracle.pushed(id, OpKind::Read, 1).expect("a push");
    assert!(matches!(
        oracle.pushed(id, OpKind::Read, 1),
        Err(Violation::DuplicateIdentity { op }) if op == id
    ));
    oracle
        .observe(&done(id, OpKind::Read, 1))
        .expect("its completion");
    assert!(matches!(
        oracle.pushed(id, OpKind::Read, 1),
        Err(Violation::DuplicateIdentity { op }) if op == id
    ));
}

#[test]
fn finish_names_every_operation_still_outstanding() {
    let instance = InstanceId::next();
    let mut oracle = Oracle::new();
    for seq in 0..3 {
        oracle
            .pushed(op(instance, seq), OpKind::Read, 0)
            .expect("a push");
    }
    oracle
        .observe(&done(op(instance, 1), OpKind::Read, 0))
        .expect("one completion");
    let Err(Violation::Outstanding { mut ops }) = oracle.finish() else {
        panic!("two operations are outstanding");
    };
    ops.sort_by_key(|op| op.seq);
    assert_eq!(ops, [op(instance, 0), op(instance, 2)]);
}

fn epoch(instance: InstanceId, id: u64) -> Epoch<V> {
    Epoch::new(Lineage { instance, seq: 0 }, id)
}

#[test]
fn a_write_accepted_at_or_below_its_seal_is_a_violation_and_one_above_is_not() {
    let instance = InstanceId::next();
    let mut oracle = Oracle::new();
    oracle.sealed(epoch(instance, 3));
    for (seq, id) in [(0, 3), (1, 2)] {
        assert!(matches!(
            oracle.pushed(op(instance, seq), write_kind(instance, id), 0),
            Err(Violation::WriteAfterSeal {
                sealed_through: 3,
                ..
            })
        ));
    }
    oracle
        .pushed(op(instance, 2), write_kind(instance, 4), 0)
        .expect("a write above the seal");
    oracle
        .pushed(op(instance, 3), OpKind::Read, 0)
        .expect("a read is not sealed");
    let other = InstanceId::next();
    oracle
        .pushed(op(other, 0), write_kind(other, 1), 0)
        .expect("another lineage's epoch 1 is not sealed");
}

#[test]
fn a_lower_seal_reported_later_does_not_lower_the_seal_point() {
    let instance = InstanceId::next();
    let mut oracle = Oracle::new();
    oracle.sealed(epoch(instance, 5));
    oracle.sealed(epoch(instance, 3));
    assert!(matches!(
        oracle.pushed(op(instance, 0), write_kind(instance, 4), 0),
        Err(Violation::WriteAfterSeal {
            sealed_through: 5,
            ..
        })
    ));
}

#[test]
fn durable_is_a_violation_unless_a_seal_covers_it() {
    let instance = InstanceId::next();
    let mut oracle = Oracle::new();
    let durable = |id| -> TestEntry {
        Entry::Durable {
            through: epoch(instance, id),
        }
    };
    assert!(matches!(
        oracle.observe(&durable(3)),
        Err(Violation::DurableNotSealed { .. })
    ));
    oracle.sealed(epoch(instance, 2));
    assert!(matches!(
        oracle.observe(&durable(3)),
        Err(Violation::DurableNotSealed { .. })
    ));
    oracle.sealed(epoch(instance, 3));
    oracle.observe(&durable(3)).expect("sealed through 3");
    oracle.observe(&durable(2)).expect("at or below a seal");
}

#[test]
fn durable_before_a_covered_write_completes_is_a_violation_and_after_is_not() {
    let instance = InstanceId::next();
    let mut oracle = Oracle::new();
    let covered = op(instance, 0);
    oracle
        .pushed(covered, write_kind(instance, 2), 0)
        .expect("a write");
    oracle
        .pushed(op(instance, 1), write_kind(instance, 4), 0)
        .expect("a write above the seal to come");
    let other = InstanceId::next();
    oracle
        .pushed(op(other, 0), write_kind(other, 1), 0)
        .expect("another lineage's write");
    oracle.sealed(epoch(instance, 3));
    let durable: TestEntry = Entry::Durable {
        through: epoch(instance, 3),
    };
    assert!(matches!(
        oracle.observe(&durable),
        Err(Violation::DurableBeforeCompletion { op, .. }) if op == covered
    ));
    oracle
        .observe(&done(covered, write_kind(instance, 2), 0))
        .expect("its completion");
    oracle
        .observe(&durable)
        .expect("a write above the seal, or of another lineage, does not hold it");
}

#[test]
fn the_readiness_check_passes_an_implementation_that_sets_on_every_transition() {
    let mut fake = Fake::new(Signals::OnEveryTransition);
    // A stale entry and a stale signal, which the check must clear before it starts.
    fake.complete_one();
    let entries = check_readiness(&mut fake, Fake::complete_one, BOUND).expect("the rule is kept");
    assert_eq!(
        entries.len(),
        3,
        "the stale entry and one for each push, handed back for the caller's oracle"
    );
}

#[test]
fn the_readiness_check_fails_an_implementation_that_never_sets() {
    let mut fake = Fake::new(Signals::Never);
    assert!(matches!(
        check_readiness(&mut fake, Fake::complete_one, FAILING_BOUND),
        Err(ReadinessFailure::NotSetForFirstEntry)
    ));
}

#[test]
fn the_readiness_check_fails_an_implementation_that_sets_only_once() {
    let mut fake = Fake::new(Signals::OnceOnly);
    assert!(matches!(
        check_readiness(&mut fake, Fake::complete_one, FAILING_BOUND),
        Err(ReadinessFailure::NotSetAgain)
    ));
}
