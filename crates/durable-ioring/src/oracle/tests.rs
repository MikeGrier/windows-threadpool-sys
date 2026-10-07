// Copyright (c) 2026 Mike Grier
//! Unit tests for the conformance oracle and the readiness check: each rule both refused when
//! broken and accepted when kept, and the readiness check failing for an implementation that
//! breaks either half of DI-D-28's rule.

use std::collections::VecDeque;
use std::io;
use std::time::Duration;

use win_shared_os_owned_handle::SharedHandle;
use win_sync_sys::{Event, ResetMode};

use super::{ConformanceOracle, ReadinessFailure, Violation, check_readiness};
use crate::contract::{DurableRing, EntryOf, PushResult};
use crate::ids::{DioringIds, InstanceId, Lineage, OpId};
use crate::types::{
    AddFileError, Entry, Epoch, FileKey, FileOptions, LineageInfo, OpCompletion, OpKind, Outcome,
    ReadOptions, WriteOptions,
};

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
    let durable: TestEntry = Entry::Durable {
        through: Epoch::new(Lineage { instance, seq: 0 }, 3),
    };
    oracle.observe(&durable).expect("a Durable entry");
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

/// How the fake sets its readiness signal.
#[derive(Clone, Copy, PartialEq)]
enum Signals {
    /// On every empty-to-non-empty transition, as DI-D-28 requires.
    OnEveryTransition,
    /// Never.
    Never,
    /// On the first transition only.
    OnceOnly,
}

/// The least implementation the readiness check can run against: a queue and a signal, with an
/// operation that completes the moment it is pushed.
struct Fake {
    instance: InstanceId,
    signal: Event,
    signals: Signals,
    queue: VecDeque<EntryOf<Self>>,
    next: u64,
    transitions: u32,
}

impl Fake {
    fn new(signals: Signals) -> Self {
        Self {
            instance: InstanceId::next(),
            signal: Event::new(ResetMode::Auto, false).expect("create the signal"),
            signals,
            queue: VecDeque::new(),
            next: 0,
            transitions: 0,
        }
    }

    fn complete_one(&mut self) {
        let id = op(self.instance, self.next);
        self.next += 1;
        let was_empty = self.queue.is_empty();
        self.queue.push_back(done(id, OpKind::Read, 0));
        if was_empty {
            self.transitions += 1;
            let set = match self.signals {
                Signals::OnEveryTransition => true,
                Signals::Never => false,
                Signals::OnceOnly => self.transitions == 1,
            };
            if set {
                self.signal.set().expect("set the signal");
            }
        }
    }
}

impl DurableRing for Fake {
    type Ids = V;
    type Buffer = Vec<u8>;
    type Context = u32;

    fn readiness(&mut self) -> io::Result<Event> {
        self.signal.try_clone()
    }

    fn add_file_with(
        &mut self,
        _key: FileKey,
        _file: SharedHandle,
        _options: FileOptions,
    ) -> Result<(), AddFileError> {
        Ok(())
    }

    fn default_lineage(&self) -> Lineage {
        Lineage {
            instance: self.instance,
            seq: 0,
        }
    }

    fn lineages(&self) -> Vec<LineageInfo<V>> {
        Vec::new()
    }

    fn write_with(
        &mut self,
        _file: FileKey,
        _offset: u64,
        _buffer: Vec<u8>,
        _epoch: Epoch<V>,
        _context: u32,
        _options: WriteOptions<V>,
    ) -> PushResult<Self> {
        unimplemented!("the readiness check pushes through its own closure")
    }

    fn read_with(
        &mut self,
        _file: FileKey,
        _offset: u64,
        _buffer: Vec<u8>,
        _context: u32,
        _options: ReadOptions<V>,
    ) -> PushResult<Self> {
        unimplemented!("the readiness check pushes through its own closure")
    }

    fn pop(&mut self) -> io::Result<Option<EntryOf<Self>>> {
        Ok(self.queue.pop_front())
    }
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
