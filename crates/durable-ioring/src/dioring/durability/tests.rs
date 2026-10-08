// Copyright (c) 2026 Mike Grier
//! Unit tests for one lineage's durability state, without a ring.

use std::sync::Arc;

use super::{Due, Durability, Event, Flush, Routing, Sealing, State};
use crate::dioring::DomainId;
use crate::ids::{DioringIds, FailureToken, InstanceId, Lineage as LineageId, OpId};
use crate::types::{FileKey, SuspectWrite};

// Failures, their reach, and their resolution: DI-3.2.4.
mod failures;

type Lineage = Durability<u64, &'static str>;

const A: FileKey = FileKey(1);
const B: FileKey = FileKey(2);
const C: FileKey = FileKey(3);
const DEFAULT: Routing = Routing {
    default: true,
    provider: false,
};

fn lineage() -> Lineage {
    Durability::new(LineageId {
        instance: InstanceId::next(),
        seq: 0,
    })
}

fn target(file: FileKey) -> &'static str {
    match file {
        A => "a",
        B => "b",
        _ => "c",
    }
}

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
        self.push_in(lineage, epoch, file, routing, &[])
    }

    /// Push a write to a file declared with `domains`.
    fn push_in(
        &mut self,
        lineage: &mut Lineage,
        epoch: u64,
        file: FileKey,
        routing: Routing,
        domains: &[u32],
    ) -> OpId {
        let op = OpId {
            instance: self.instance,
            seq: self.next,
        };
        self.next += 1;
        let domains: Arc<[DomainId]> = domains.iter().map(|&d| DomainId(d)).collect();
        lineage.pushed(op, epoch, file, target(file), routing, domains);
        op
    }
}

fn flush(through: u64, file: FileKey) -> Flush<u64, &'static str> {
    Flush {
        through,
        file,
        target: target(file),
    }
}

/// An entry a change made due, in a form a test can compare: failures by sequence number and
/// suspect sets by the sequence numbers of their writes.
#[derive(Debug, PartialEq, Eq)]
enum Seen {
    Durable(u64),
    Blocked(u64, u64),
    Failed(u64, Vec<u64>),
    Abandoned(u64, Vec<u64>),
}

fn seen(due: &Due<u64, &'static str>) -> Vec<Seen> {
    let ops = |writes: &[SuspectWrite<DioringIds<u64>>]| -> Vec<u64> {
        writes.iter().map(|w| w.op.seq).collect()
    };
    due.events
        .iter()
        .map(|event| match event {
            Event::Durable(through) => Seen::Durable(*through),
            Event::Blocked { through, by } => Seen::Blocked(*through, by.seq),
            Event::Failed(failed) => Seen::Failed(failed.id.seq, ops(failed.suspect.writes())),
            Event::Abandoned { failure, suspect } => {
                Seen::Abandoned(failure.seq, ops(suspect.writes()))
            }
        })
        .collect()
}

fn durable(due: &Due<u64, &'static str>) -> Vec<u64> {
    seen(due)
        .into_iter()
        .filter_map(|s| match s {
            Seen::Durable(through) => Some(through),
            _ => None,
        })
        .collect()
}

/// The tokens of every failure a change observed.
fn tokens(due: Due<u64, &'static str>) -> Vec<FailureToken> {
    due.events
        .into_iter()
        .filter_map(|event| match event {
            Event::Failed(failed) => Some(failed.token),
            _ => None,
        })
        .collect()
}

fn quiet(due: &Due<u64, &'static str>) -> bool {
    due.flushes.is_empty() && due.events.is_empty()
}

fn submitted(sealing: Sealing<u64, &'static str>) -> Due<u64, &'static str> {
    match sealing {
        Sealing::Submitted(due) => due,
        Sealing::AlreadySealed(state) => panic!("expected a new seal, got {state:?}"),
    }
}

fn already(sealing: Sealing<u64, &'static str>) -> State {
    match sealing {
        Sealing::AlreadySealed(state) => state,
        Sealing::Submitted(due) => panic!("expected a request below the seal, got {due:?}"),
    }
}

#[test]
fn a_new_lineage_has_sealed_nothing_and_every_epoch_is_open() {
    let lineage = lineage();
    assert_eq!(lineage.sealed_through(), None);
    assert_eq!(lineage.durable_through(), None);
    assert_eq!(lineage.state(0), State::Open);
    assert_eq!(lineage.refuses(0), None);
}

#[test]
fn an_inconsistency_is_recorded_for_the_consumer_to_assert_on_not_panicked_on() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let write = ops.push(&mut lineage, 1, A, DEFAULT);
    assert!(quiet(&lineage.completed(write, true)));
    assert_eq!(lineage.inconsistency(), None, "an ordinary completion");
    assert!(quiet(&lineage.completed(write, true)));
    assert_eq!(lineage.inconsistency(), Some("a write completed twice"));

    let mut lineage = self::lineage();
    assert!(quiet(&lineage.flushed(1, A, Ok(()))));
    assert_eq!(
        lineage.inconsistency(),
        Some("a flush completed for a seal that does not exist")
    );
}

#[test]
fn a_seal_with_nothing_to_flush_is_durable_at_once() {
    let mut lineage = lineage();
    let due = submitted(lineage.seal(3));
    assert!(due.flushes.is_empty());
    assert_eq!(seen(&due), [Seen::Durable(3)]);
    assert_eq!(lineage.sealed_through(), Some(3));
    assert_eq!(lineage.durable_through(), Some(3));
    assert_eq!(lineage.state(3), State::Durable);
    assert_eq!(lineage.state(1), State::Durable);
    assert_eq!(lineage.state(4), State::Open);
}

#[test]
fn a_seal_waits_for_its_writes_then_flushes_their_file() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let write = ops.push(&mut lineage, 1, A, DEFAULT);
    assert!(
        quiet(&submitted(lineage.seal(2))),
        "the write is still in flight"
    );
    assert_eq!(lineage.state(1), State::Pending);

    let due = lineage.completed(write, true);
    assert_eq!(due.flushes, [flush(2, A)]);
    assert!(due.events.is_empty(), "the flush has not answered");

    assert_eq!(durable(&lineage.flushed(2, A, Ok(()))), [2]);
    assert_eq!(lineage.state(1), State::Durable);
}

#[test]
fn writes_above_the_seal_do_not_hold_it() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    ops.push(&mut lineage, 5, A, DEFAULT);
    assert_eq!(durable(&submitted(lineage.seal(3))), [3]);
}

#[test]
fn a_seal_flushes_each_file_once_and_waits_for_every_flush() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let writes = [
        ops.push(&mut lineage, 1, A, DEFAULT),
        ops.push(&mut lineage, 1, B, DEFAULT),
        ops.push(&mut lineage, 2, A, DEFAULT),
    ];
    for write in writes {
        assert!(quiet(&lineage.completed(write, true)));
    }
    let due = submitted(lineage.seal(2));
    assert_eq!(due.flushes, [flush(2, A), flush(2, B)]);
    assert!(quiet(&lineage.flushed(2, B, Ok(()))), "A has not answered");
    assert_eq!(durable(&lineage.flushed(2, A, Ok(()))), [2]);
}

#[test]
fn a_failed_write_is_named_to_no_flush() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let write = ops.push(&mut lineage, 1, A, DEFAULT);
    lineage.completed(write, false);
    let due = submitted(lineage.seal(1));
    assert!(due.flushes.is_empty());
    assert_eq!(seen(&due), [Seen::Durable(1)]);
}

#[test]
fn a_write_at_or_below_the_seal_is_refused_and_one_above_is_not() {
    let mut lineage = lineage();
    submitted(lineage.seal(3));
    assert_eq!(lineage.refuses(2), Some(3));
    assert_eq!(lineage.refuses(3), Some(3));
    assert_eq!(lineage.refuses(4), None);
}

#[test]
fn asking_again_reports_the_state_and_changes_nothing() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let write = ops.push(&mut lineage, 1, A, DEFAULT);
    submitted(lineage.seal(3));
    assert_eq!(already(lineage.seal(2)), State::Pending);
    assert_eq!(already(lineage.seal(3)), State::Pending);
    assert_eq!(
        lineage.sealed_through(),
        Some(3),
        "a request below does not lower the seal"
    );

    lineage.completed(write, true);
    lineage.flushed(3, A, Ok(()));
    assert_eq!(already(lineage.seal(1)), State::Durable);
}

#[test]
fn a_later_seal_done_first_waits_for_the_earlier_one() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let first = ops.push(&mut lineage, 1, A, DEFAULT);
    let second = ops.push(&mut lineage, 2, B, DEFAULT);
    lineage.completed(first, true);
    lineage.completed(second, true);
    assert_eq!(submitted(lineage.seal(1)).flushes, [flush(1, A)]);
    assert_eq!(submitted(lineage.seal(2)).flushes, [flush(2, B)]);

    assert!(
        quiet(&lineage.flushed(2, B, Ok(()))),
        "1 is not yet durable, and nothing failed, so 2 is not Blocked"
    );
    assert_eq!(lineage.state(2), State::Pending);
    assert_eq!(
        durable(&lineage.flushed(1, A, Ok(()))),
        [1, 2],
        "both, in order"
    );
    assert_eq!(lineage.durable_through(), Some(2));
}

#[test]
fn each_write_is_named_by_the_first_seal_at_or_above_it_and_by_no_other() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let low = ops.push(&mut lineage, 1, A, DEFAULT);
    let high = ops.push(&mut lineage, 2, B, DEFAULT);
    lineage.completed(low, true);
    lineage.completed(high, true);
    assert_eq!(submitted(lineage.seal(1)).flushes, [flush(1, A)]);
    lineage.flushed(1, A, Ok(()));
    assert_eq!(
        submitted(lineage.seal(2)).flushes,
        [flush(2, B)],
        "A's write was already named by the seal through 1"
    );
}

#[test]
fn a_seal_names_completed_writes_by_epoch_not_by_push_order() {
    let mut lineage = lineage();
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
    lineage.flushed(4, A, Ok(()));
    lineage.flushed(4, B, Ok(()));
    assert_eq!(submitted(lineage.seal(5)).flushes, [flush(5, A)]);
}

#[test]
fn a_seal_waits_for_a_lower_write_in_flight_while_a_higher_one_has_completed() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let lower = ops.push(&mut lineage, 1, A, DEFAULT);
    let higher = ops.push(&mut lineage, 2, B, DEFAULT);
    lineage.completed(higher, true);
    assert!(quiet(&submitted(lineage.seal(2))));
    let due = lineage.completed(lower, true);
    assert_eq!(due.flushes, [flush(2, A), flush(2, B)]);
}

#[test]
fn a_write_a_provider_must_answer_for_leaves_its_seal_pending() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let only = Routing {
        default: false,
        provider: true,
    };
    let write = ops.push(&mut lineage, 1, A, only);
    lineage.completed(write, true);
    assert!(
        quiet(&submitted(lineage.seal(1))),
        "no default flush, and no provider yet"
    );
    assert_eq!(lineage.state(1), State::Pending);
}

#[test]
fn a_file_both_answer_for_is_flushed_and_still_waits_for_the_provider() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let both = Routing {
        default: true,
        provider: true,
    };
    let write = ops.push(&mut lineage, 1, A, both);
    lineage.completed(write, true);
    assert_eq!(submitted(lineage.seal(1)).flushes, [flush(1, A)]);
    assert!(quiet(&lineage.flushed(1, A, Ok(()))));
    assert_eq!(lineage.state(1), State::Pending);
}
