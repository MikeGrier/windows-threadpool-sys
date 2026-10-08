// Copyright (c) 2026 Mike Grier
//! Unit tests for one lineage's durability state, without a ring.

use super::{Due, Durability, Flush, Routing, Sealing, State};
use crate::ids::{InstanceId, OpId};
use crate::types::FileKey;

type Lineage = Durability<u64, &'static str>;

const A: FileKey = FileKey(1);
const B: FileKey = FileKey(2);
const DEFAULT: Routing = Routing {
    default: true,
    provider: false,
};

struct Ops {
    instance: InstanceId,
    next: u64,
}

impl Ops {
    fn new() -> Self {
        Self {
            instance: InstanceId::next(),
            next: 0,
        }
    }

    fn push(&mut self, lineage: &mut Lineage, epoch: u64, file: FileKey, routing: Routing) -> OpId {
        let op = OpId {
            instance: self.instance,
            seq: self.next,
        };
        self.next += 1;
        let target = if file == A { "a" } else { "b" };
        lineage.pushed(op, epoch, file, target, routing);
        op
    }
}

fn flush(through: u64, file: FileKey) -> Flush<u64, &'static str> {
    Flush {
        through,
        file,
        target: if file == A { "a" } else { "b" },
    }
}

fn submitted(sealing: Sealing<u64, &'static str>) -> Due<u64, &'static str> {
    match sealing {
        Sealing::Submitted(due) => due,
        Sealing::AlreadySealed(state) => panic!("expected a new seal, got {state:?}"),
    }
}

fn nothing() -> Due<u64, &'static str> {
    Due::default()
}

#[test]
fn a_new_lineage_has_sealed_nothing_and_every_epoch_is_open() {
    let lineage = Lineage::new();
    assert_eq!(lineage.sealed_through(), None);
    assert_eq!(lineage.durable_through(), None);
    assert_eq!(lineage.state(0), State::Open);
    assert_eq!(lineage.refuses(0), None);
}

#[test]
fn a_seal_with_nothing_to_flush_is_durable_at_once() {
    let mut lineage = Lineage::new();
    let due = submitted(lineage.seal(3));
    assert_eq!(
        due,
        Due {
            flushes: Vec::new(),
            durable: vec![3],
        }
    );
    assert_eq!(lineage.sealed_through(), Some(3));
    assert_eq!(lineage.durable_through(), Some(3));
    assert_eq!(lineage.state(3), State::Durable);
    assert_eq!(lineage.state(1), State::Durable);
    assert_eq!(lineage.state(4), State::Open);
}

#[test]
fn a_seal_waits_for_its_writes_then_flushes_their_file() {
    let mut lineage = Lineage::new();
    let mut ops = Ops::new();
    let write = ops.push(&mut lineage, 1, A, DEFAULT);
    assert_eq!(
        submitted(lineage.seal(2)),
        nothing(),
        "the write is still in flight"
    );
    assert_eq!(lineage.state(1), State::Pending);

    let due = lineage.completed(write, true);
    assert_eq!(due.flushes, [flush(2, A)]);
    assert!(due.durable.is_empty(), "the flush has not answered");

    assert_eq!(lineage.flushed(2, A, true).durable, [2]);
    assert_eq!(lineage.state(1), State::Durable);
}

#[test]
fn writes_above_the_seal_do_not_hold_it() {
    let mut lineage = Lineage::new();
    let mut ops = Ops::new();
    ops.push(&mut lineage, 5, A, DEFAULT);
    assert_eq!(submitted(lineage.seal(3)).durable, [3]);
}

#[test]
fn a_seal_flushes_each_file_once_and_waits_for_every_flush() {
    let mut lineage = Lineage::new();
    let mut ops = Ops::new();
    let writes = [
        ops.push(&mut lineage, 1, A, DEFAULT),
        ops.push(&mut lineage, 1, B, DEFAULT),
        ops.push(&mut lineage, 2, A, DEFAULT),
    ];
    for write in writes {
        assert_eq!(lineage.completed(write, true), nothing());
    }
    let due = submitted(lineage.seal(2));
    assert_eq!(due.flushes, [flush(2, A), flush(2, B)]);
    assert_eq!(lineage.flushed(2, B, true), nothing(), "A has not answered");
    assert_eq!(lineage.flushed(2, A, true).durable, [2]);
}

#[test]
fn a_failed_write_is_named_to_no_flush() {
    let mut lineage = Lineage::new();
    let mut ops = Ops::new();
    let write = ops.push(&mut lineage, 1, A, DEFAULT);
    lineage.completed(write, false);
    assert_eq!(
        submitted(lineage.seal(1)),
        Due {
            flushes: Vec::new(),
            durable: vec![1],
        }
    );
}

#[test]
fn a_write_at_or_below_the_seal_is_refused_and_one_above_is_not() {
    let mut lineage = Lineage::new();
    submitted(lineage.seal(3));
    assert_eq!(lineage.refuses(2), Some(3));
    assert_eq!(lineage.refuses(3), Some(3));
    assert_eq!(lineage.refuses(4), None);
}

#[test]
fn asking_again_reports_the_state_and_changes_nothing() {
    let mut lineage = Lineage::new();
    let mut ops = Ops::new();
    let write = ops.push(&mut lineage, 1, A, DEFAULT);
    submitted(lineage.seal(3));
    assert_eq!(lineage.seal(2), Sealing::AlreadySealed(State::Pending));
    assert_eq!(lineage.seal(3), Sealing::AlreadySealed(State::Pending));
    assert_eq!(
        lineage.sealed_through(),
        Some(3),
        "a request below does not lower the seal"
    );

    lineage.completed(write, true);
    lineage.flushed(3, A, true);
    assert_eq!(lineage.seal(1), Sealing::AlreadySealed(State::Durable));
}

#[test]
fn a_later_seal_done_first_waits_for_the_earlier_one() {
    let mut lineage = Lineage::new();
    let mut ops = Ops::new();
    let first = ops.push(&mut lineage, 1, A, DEFAULT);
    let second = ops.push(&mut lineage, 2, B, DEFAULT);
    lineage.completed(first, true);
    lineage.completed(second, true);
    assert_eq!(submitted(lineage.seal(1)).flushes, [flush(1, A)]);
    assert_eq!(submitted(lineage.seal(2)).flushes, [flush(2, B)]);

    assert_eq!(
        lineage.flushed(2, B, true),
        nothing(),
        "1 is not yet durable"
    );
    assert_eq!(lineage.state(2), State::Pending);
    assert_eq!(
        lineage.flushed(1, A, true).durable,
        [1, 2],
        "both, in order"
    );
    assert_eq!(lineage.durable_through(), Some(2));
}

#[test]
fn each_write_is_named_by_the_first_seal_at_or_above_it_and_by_no_other() {
    let mut lineage = Lineage::new();
    let mut ops = Ops::new();
    let low = ops.push(&mut lineage, 1, A, DEFAULT);
    let high = ops.push(&mut lineage, 2, B, DEFAULT);
    lineage.completed(low, true);
    lineage.completed(high, true);
    assert_eq!(submitted(lineage.seal(1)).flushes, [flush(1, A)]);
    lineage.flushed(1, A, true);
    assert_eq!(
        submitted(lineage.seal(2)).flushes,
        [flush(2, B)],
        "A's write was already named by the seal through 1"
    );
}

#[test]
fn a_seal_names_completed_writes_by_epoch_not_by_push_order() {
    let mut lineage = Lineage::new();
    let mut ops = Ops::new();
    let writes = [
        ops.push(&mut lineage, 5, A, DEFAULT),
        ops.push(&mut lineage, 3, B, DEFAULT),
        ops.push(&mut lineage, 4, A, DEFAULT),
    ];
    for write in writes {
        lineage.completed(write, true);
    }
    assert_eq!(
        submitted(lineage.seal(4)).flushes,
        [flush(4, B), flush(4, A)]
    );
    lineage.flushed(4, A, true);
    lineage.flushed(4, B, true);
    assert_eq!(submitted(lineage.seal(5)).flushes, [flush(5, A)]);
}

#[test]
fn a_seal_waits_for_a_lower_write_in_flight_while_a_higher_one_has_completed() {
    let mut lineage = Lineage::new();
    let mut ops = Ops::new();
    let lower = ops.push(&mut lineage, 1, A, DEFAULT);
    let higher = ops.push(&mut lineage, 2, B, DEFAULT);
    lineage.completed(higher, true);
    assert_eq!(submitted(lineage.seal(2)), nothing());
    let due = lineage.completed(lower, true);
    assert_eq!(due.flushes, [flush(2, A), flush(2, B)]);
}

#[test]
fn a_failed_flush_leaves_its_seal_and_every_later_one_pending() {
    let mut lineage = Lineage::new();
    let mut ops = Ops::new();
    let first = ops.push(&mut lineage, 1, A, DEFAULT);
    lineage.completed(first, true);
    submitted(lineage.seal(1));
    assert_eq!(lineage.flushed(1, A, false), nothing());
    assert_eq!(lineage.state(1), State::Pending);
    assert_eq!(
        submitted(lineage.seal(2)),
        nothing(),
        "the prefix stops at the failure"
    );
    assert_eq!(lineage.durable_through(), None);
}

#[test]
fn a_write_a_provider_must_answer_for_leaves_its_seal_pending() {
    let mut lineage = Lineage::new();
    let mut ops = Ops::new();
    let only = Routing {
        default: false,
        provider: true,
    };
    let write = ops.push(&mut lineage, 1, A, only);
    lineage.completed(write, true);
    assert_eq!(
        submitted(lineage.seal(1)),
        nothing(),
        "no default flush, and no provider yet"
    );
    assert_eq!(lineage.state(1), State::Pending);
}

#[test]
fn a_file_both_answer_for_is_flushed_and_still_waits_for_the_provider() {
    let mut lineage = Lineage::new();
    let mut ops = Ops::new();
    let both = Routing {
        default: true,
        provider: true,
    };
    let write = ops.push(&mut lineage, 1, A, both);
    lineage.completed(write, true);
    assert_eq!(submitted(lineage.seal(1)).flushes, [flush(1, A)]);
    assert_eq!(lineage.flushed(1, A, true), nothing());
    assert_eq!(lineage.state(1), State::Pending);
}
