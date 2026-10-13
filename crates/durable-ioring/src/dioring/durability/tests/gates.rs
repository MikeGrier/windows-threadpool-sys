// Copyright (c) 2026 Mike Grier
//! Gates (DI-D-23, DI-D-43), without a ring: held operations released when their gate's lineage
//! reaches it, a seal waiting for the held writes it covers, operations ended as never issued, and
//! the cycles a push or a seal would close.

use std::io;
use std::sync::Arc;

use super::Stamp;
use super::{
    A, B, DEFAULT, FULL, Lineage, Ops, Seen, flush, flush_of, inconsistent, lineage, quiet, seen,
    submitted, target, tokens,
};
use crate::dioring::durability::{
    Accepted, DEFAULT_LINEAGE as D, Due, Gate, GateState, Key, Reach, Sealing, State,
};
use crate::ids::OpId;
use crate::types::{FailureCause, ImportScope, Resolution};

fn at(lineage: Key, epoch: u64) -> Gate<u64> {
    Gate { lineage, epoch }
}

/// A core with lineages minted beside its default.
fn minted(count: usize) -> (Lineage, Vec<Key>) {
    let mut lineage = lineage();
    let keys = (0..count).map(|_| lineage.mint(None)).collect();
    (lineage, keys)
}

fn two() -> (Lineage, Key) {
    let (lineage, keys) = minted(1);
    (lineage, keys[0])
}

impl Ops {
    fn mint(&mut self) -> OpId {
        let op = OpId {
            instance: self.instance,
            seq: self.next,
        };
        self.next += 1;
        op
    }
}

/// Hold a write to `A` tagged `own`, gated on `gate`, as the push path would: the gate waiting, and
/// no cycle.
fn hold_write(lineage: &mut Lineage, ops: &mut Ops, own: Gate<u64>, gate: Gate<u64>) -> OpId {
    assert_eq!(lineage.gate_state(gate), GateState::Waiting, "{gate:?}");
    assert_eq!(lineage.write_cycle(own, gate), None, "{own:?} on {gate:?}");
    let op = ops.mint();
    let write = Accepted {
        op,
        lineage: own.lineage,
        epoch: own.epoch,
        file: A,
        target: target(A),
        routing: DEFAULT,
        domains: Arc::from([]),
    };
    lineage.0.hold(op, Some(write), gate);
    op
}

/// Hold a read gated on `gate`.
fn hold_read(lineage: &mut Lineage, ops: &mut Ops, gate: Gate<u64>) -> OpId {
    assert_eq!(lineage.gate_state(gate), GateState::Waiting, "{gate:?}");
    let op = ops.mint();
    lineage.0.hold(op, None, gate);
    op
}

/// Push every operation `due` released, as the I/O core would, returning their sequence numbers.
fn issue(lineage: &mut Lineage, due: &Due<u64, &'static str, Stamp>) -> Vec<u64> {
    for &op in &due.released {
        lineage.issued(op);
    }
    due.released.iter().map(|op| op.seq).collect()
}

fn import(lineage: &mut Lineage) -> Due<u64, &'static str, Stamp> {
    let cause = FailureCause::Imported {
        scope: ImportScope::All,
    };
    lineage.0.import(cause, &Reach::All, None).1
}

fn cycle(sealing: Sealing<u64, &'static str, Stamp>) -> Vec<Gate<u64>> {
    match sealing {
        Sealing::Cycle(cycle) => cycle,
        Sealing::Submitted(due) => panic!("expected a cycle, got a seal: {due:?}"),
        Sealing::AlreadySealed(state) => panic!("expected a cycle, got {state:?}"),
    }
}

#[test]
fn a_gate_is_waiting_durable_abandoned_or_unknown() {
    let (mut lineage, minted) = two();
    assert_eq!(lineage.gate_state(at(D, 2)), GateState::Waiting);
    submitted(lineage.seal(2));
    assert_eq!(lineage.gate_state(at(D, 2)), GateState::Durable);
    assert_eq!(lineage.gate_state(at(D, 1)), GateState::Durable);
    assert_eq!(lineage.gate_state(at(D, 3)), GateState::Waiting);
    assert_eq!(lineage.gate_state(at(minted, 1)), GateState::Waiting);
    assert_eq!(lineage.gate_state(at(minted + 1, 1)), GateState::Unknown);

    let mut ops = Ops::new();
    ops.push(&mut lineage, 4, A, DEFAULT);
    let token = tokens(import(&mut lineage)).remove(0);
    lineage
        .resolve(vec![(token, Resolution::Abandon)])
        .expect("abandon");
    assert_eq!(lineage.gate_state(at(D, 4)), GateState::Abandoned);

    lineage.end(minted);
    assert_eq!(
        lineage.gate_state(at(minted, 1)),
        GateState::Unknown,
        "an ended lineage is one the instance cannot answer for"
    );
}

#[test]
fn a_held_operation_is_released_when_its_gates_lineage_reaches_it_and_only_then() {
    let (mut lineage, minted) = two();
    let mut ops = Ops::new();
    let write = hold_write(&mut lineage, &mut ops, at(minted, 1), at(D, 2));
    let read = hold_read(&mut lineage, &mut ops, at(D, 2));
    let due = submitted(lineage.seal(1));
    assert_eq!(seen(&due), [Seen::Durable(1)]);
    assert!(due.released.is_empty(), "1 is below the gate");

    let due = submitted(lineage.seal(2));
    assert_eq!(seen(&due), [Seen::Durable(2)]);
    assert_eq!(issue(&mut lineage, &due), [write.seq, read.seq]);

    // The write is in flight now, in its own lineage, and its seal flushes it once it completes.
    assert!(quiet(&submitted(lineage.0.seal(minted, 1))), "in flight");
    let due = lineage.completed(write, FULL);
    assert_eq!(due.flushes, [flush_of(minted, 1, A)]);
    assert_eq!(
        seen(&lineage.0.flushed(minted, 1, A, Ok(()))),
        [Seen::DurableIn(minted, 1)]
    );
}

#[test]
fn operations_released_together_go_in_push_order_whatever_their_gates() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let late = hold_read(&mut lineage, &mut ops, at(D, 3));
    let early = hold_read(&mut lineage, &mut ops, at(D, 1));
    let middle = hold_read(&mut lineage, &mut ops, at(D, 2));
    let after = hold_read(&mut lineage, &mut ops, at(D, 4));
    let due = submitted(lineage.seal(3));
    assert_eq!(issue(&mut lineage, &due), [late.seq, early.seq, middle.seq]);
    let due = submitted(lineage.seal(4));
    assert_eq!(issue(&mut lineage, &due), [after.seq]);
}

#[test]
fn a_seal_waits_for_the_held_writes_it_covers() {
    let (mut lineage, minted) = two();
    let mut ops = Ops::new();
    let write = hold_write(&mut lineage, &mut ops, at(minted, 1), at(D, 5));
    assert!(quiet(&submitted(lineage.0.seal(minted, 3))));
    assert_eq!(lineage.0.state(minted, 1), Some(State::Pending));
    let due = submitted(lineage.seal(5));
    assert_eq!(issue(&mut lineage, &due), [write.seq]);
    assert!(
        quiet(&submitted(lineage.0.seal(minted, 4))),
        "issued, so in flight, and still waited for"
    );
    let due = lineage.completed(write, FULL);
    assert_eq!(due.flushes, [flush_of(minted, 3, A)]);
    assert_eq!(
        seen(&lineage.0.flushed(minted, 3, A, Ok(()))),
        [Seen::DurableIn(minted, 3), Seen::DurableIn(minted, 4)]
    );
}

#[test]
fn abandoning_a_gate_ends_what_waits_for_it_as_never_issued() {
    let (mut lineage, minted) = two();
    let mut ops = Ops::new();
    let suspect = ops.push(&mut lineage, 1, B, DEFAULT);
    let token = tokens(import(&mut lineage)).remove(0);
    let read = hold_read(&mut lineage, &mut ops, at(D, 1));
    let write = hold_write(&mut lineage, &mut ops, at(minted, 3), at(D, 1));
    let other = hold_read(&mut lineage, &mut ops, at(D, 2));
    let due = lineage
        .resolve(vec![(token, Resolution::Abandon)])
        .expect("abandon");
    assert_eq!(
        seen(&due),
        [
            Seen::Abandoned(0, vec![suspect.seq], vec![]),
            Seen::NeverIssued(read.seq, D, 1),
            Seen::NeverIssued(write.seq, D, 1),
        ]
    );
    assert_eq!(
        seen(&submitted(lineage.0.seal(minted, 3))),
        [Seen::DurableIn(minted, 3)],
        "the never-issued write no longer holds its lineage's seal"
    );
    lineage.completed(suspect, FULL);
    assert_eq!(submitted(lineage.seal(2)).flushes, [flush(2, B)]);
    let due = lineage.flushed(2, B, Ok(()));
    assert_eq!(
        issue(&mut lineage, &due),
        [other.seq],
        "2 was not abandoned"
    );
}

#[test]
fn a_held_write_whose_own_epoch_is_abandoned_is_never_issued() {
    let (mut lineage, minted) = two();
    let mut ops = Ops::new();
    let issued = ops.push_to(&mut lineage, minted, 3, A, DEFAULT, &[]);
    let held = hold_write(&mut lineage, &mut ops, at(minted, 3), at(D, 9));
    let due = import(&mut lineage);
    assert_eq!(
        seen(&due),
        [Seen::Failed(0, vec![issued.seq])],
        "a held write was never issued, so nothing put it at risk"
    );
    let token = tokens(due).remove(0);
    let due = lineage
        .resolve(vec![(token, Resolution::Abandon)])
        .expect("abandon");
    assert_eq!(
        seen(&due),
        [
            Seen::Abandoned(0, vec![issued.seq], vec![]),
            Seen::NeverIssued(held.seq, minted, 3),
        ],
        "no write may join an abandoned epoch (DI-D-35)"
    );
    assert!(submitted(lineage.seal(9)).released.is_empty());
}

#[test]
fn a_gate_and_its_own_epoch_both_abandoned_name_the_gate() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    ops.push(&mut lineage, 2, A, DEFAULT);
    ops.push(&mut lineage, 5, A, DEFAULT);
    let held = hold_write(&mut lineage, &mut ops, at(D, 5), at(D, 2));
    let token = tokens(import(&mut lineage)).remove(0);
    let due = lineage
        .resolve(vec![(token, Resolution::Abandon)])
        .expect("abandon");
    assert_eq!(seen(&due)[1], Seen::NeverIssued(held.seq, D, 2));
}

#[test]
fn a_released_write_whose_own_epoch_is_then_abandoned_is_never_issued() {
    let (mut lineage, minted) = two();
    let mut ops = Ops::new();
    let issued = ops.push_to(&mut lineage, minted, 3, A, DEFAULT, &[]);
    let held = hold_write(&mut lineage, &mut ops, at(minted, 3), at(D, 1));
    let released = submitted(lineage.seal(1));
    assert_eq!(released.released, [held], "released, and not yet pushed");
    let token = tokens(import(&mut lineage)).remove(0);
    let due = lineage
        .resolve(vec![(token, Resolution::Abandon)])
        .expect("abandon");
    assert_eq!(
        seen(&due),
        [
            Seen::Abandoned(0, vec![issued.seq], vec![]),
            Seen::NeverIssued(held.seq, minted, 3),
        ]
    );
}

#[test]
fn ending_a_lineage_ends_its_held_writes_and_what_waits_for_it() {
    let (mut lineage, minted) = two();
    let mut ops = Ops::new();
    let own = hold_write(&mut lineage, &mut ops, at(minted, 7), at(D, 5));
    let waiting = hold_write(&mut lineage, &mut ops, at(D, 1), at(minted, 2));
    let read = hold_read(&mut lineage, &mut ops, at(minted, 4));
    assert_eq!(
        seen(&lineage.end(minted)),
        [
            Seen::LineageEnded(minted, Some(7)),
            Seen::NeverIssued(own.seq, minted, 7),
            Seen::NeverIssued(waiting.seq, minted, 2),
            Seen::NeverIssued(read.seq, minted, 4),
        ],
        "a held write is a write of the lineage, so it counts in what is abandoned"
    );
    assert_eq!(
        seen(&submitted(lineage.seal(1))),
        [Seen::Durable(1)],
        "the never-issued write no longer holds the default's seal"
    );
}

#[test]
fn an_operation_released_before_its_gates_lineage_ends_is_still_issued() {
    let (mut lineage, minted) = two();
    let mut ops = Ops::new();
    let read = hold_read(&mut lineage, &mut ops, at(minted, 1));
    let due = submitted(lineage.0.seal(minted, 1));
    assert_eq!(due.released, [read]);
    assert_eq!(
        seen(&lineage.end(minted)),
        [Seen::LineageEnded(minted, None)]
    );
    lineage.issued(read);
}

#[test]
fn retiring_is_refused_while_an_operation_is_held_in_or_on_the_lineage() {
    let (mut lineage, minted) = two();
    let mut ops = Ops::new();
    let own = hold_write(&mut lineage, &mut ops, at(minted, 1), at(D, 1));
    let on = hold_read(&mut lineage, &mut ops, at(minted, 2));
    let busy = lineage.retire(minted).expect_err("held");
    assert_eq!((busy.held_for_gate, busy.in_flight), (2, 0));

    let due = submitted(lineage.seal(1));
    assert_eq!(issue(&mut lineage, &due), [own.seq]);
    let busy = lineage
        .retire(minted)
        .expect_err("in flight, and one waiting on it");
    assert_eq!((busy.held_for_gate, busy.in_flight), (1, 1));
    lineage.completed(own, FULL);
    submitted(lineage.0.seal(minted, 2));
    let due = lineage.0.flushed(minted, 2, A, Ok(()));
    assert_eq!(due.released, [on], "released as its lineage passed 2");
    lineage.retire(minted).expect("nothing holds it");
}

#[test]
fn issuing_what_was_never_held_or_not_yet_released_is_inconsistent() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let held = hold_read(&mut lineage, &mut ops, at(D, 1));
    inconsistent(
        "an operation was issued before its gate was durable",
        || lineage.issued(held),
    );

    let mut lineage = super::lineage();
    let never = ops.mint();
    inconsistent("an operation was issued that was never held", || {
        lineage.issued(never)
    });

    let (mut lineage, minted) = two();
    lineage.end(minted);
    let op = ops.mint();
    inconsistent(
        "an operation was held on a lineage that is not live",
        || lineage.0.hold(op, None, at(minted, 1)),
    );
}

#[test]
fn a_gate_on_the_writes_own_lineage_at_or_above_its_epoch_is_a_cycle() {
    let lineage = lineage();
    assert_eq!(
        lineage.write_cycle(at(D, 3), at(D, 3)),
        Some(vec![at(D, 3), at(D, 3)])
    );
    assert_eq!(
        lineage.write_cycle(at(D, 3), at(D, 5)),
        Some(vec![at(D, 3), at(D, 5)])
    );
    assert_eq!(lineage.write_cycle(at(D, 3), at(D, 2)), None);
}

#[test]
fn a_gate_back_across_two_lineages_is_a_cycle_and_names_it() {
    let (mut lineage, minted) = two();
    let mut ops = Ops::new();
    hold_write(&mut lineage, &mut ops, at(minted, 4), at(D, 9));
    assert_eq!(
        lineage.write_cycle(at(D, 9), at(minted, 4)),
        Some(vec![at(D, 9), at(minted, 4), at(minted, 4), at(D, 9)])
    );
    assert_eq!(
        lineage.write_cycle(at(D, 8), at(minted, 4)),
        Some(vec![at(D, 8), at(minted, 4), at(minted, 4), at(D, 9)]),
        "the default reaching 9 needs its write at 8"
    );
    assert_eq!(
        lineage.write_cycle(at(D, 10), at(minted, 4)),
        None,
        "nothing reaching 9 waits for 10"
    );
    assert_eq!(
        lineage.write_cycle(at(D, 9), at(minted, 3)),
        None,
        "reaching 3 needs only the writes at or below 3"
    );
}

#[test]
fn a_seal_already_made_widens_what_reaching_an_epoch_needs() {
    let (mut lineage, minted) = two();
    let mut ops = Ops::new();
    ops.push_to(&mut lineage, minted, 1, A, DEFAULT, &[]);
    hold_write(&mut lineage, &mut ops, at(minted, 4), at(D, 9));
    submitted(lineage.0.seal(minted, 5));
    assert_eq!(
        lineage.write_cycle(at(D, 9), at(minted, 3)),
        Some(vec![at(D, 9), at(minted, 3), at(minted, 4), at(D, 9)]),
        "reaching 3 now needs the seal through 5 to finish"
    );
}

#[test]
fn a_cycle_through_three_lineages_is_found_and_named_whole() {
    let (mut lineage, keys) = minted(2);
    let (l1, l2) = (keys[0], keys[1]);
    let mut ops = Ops::new();
    hold_write(&mut lineage, &mut ops, at(D, 5), at(l1, 5));
    hold_write(&mut lineage, &mut ops, at(l1, 5), at(l2, 5));
    assert_eq!(
        lineage.write_cycle(at(l2, 5), at(D, 5)),
        Some(vec![
            at(l2, 5),
            at(D, 5),
            at(D, 5),
            at(l1, 5),
            at(l1, 5),
            at(l2, 5)
        ])
    );
    assert_eq!(lineage.write_cycle(at(l2, 6), at(D, 5)), None);
}

#[test]
fn gates_in_both_directions_that_close_no_cycle_are_accepted() {
    // A log and its data: pages gated on the log records they follow, and a checkpoint record
    // gated on the pages it covers, written above them.
    let (mut lineage, data) = two();
    let mut ops = Ops::new();
    hold_write(&mut lineage, &mut ops, at(data, 1), at(D, 10));
    hold_write(&mut lineage, &mut ops, at(data, 2), at(D, 11));
    hold_write(&mut lineage, &mut ops, at(D, 20), at(data, 2));
    assert_eq!(
        lineage.write_cycle(at(D, 11), at(data, 2)),
        Some(vec![at(D, 11), at(data, 2), at(data, 2), at(D, 11)]),
        "a checkpoint at or below a page's record is the one that cannot be written"
    );
}

#[test]
fn a_seal_that_would_close_a_cycle_is_refused_and_a_lower_one_is_not() {
    let (mut lineage, minted) = two();
    let mut ops = Ops::new();
    let w1 = hold_write(&mut lineage, &mut ops, at(minted, 4), at(D, 9));
    let w2 = hold_write(&mut lineage, &mut ops, at(D, 9), at(minted, 3));
    assert_eq!(
        cycle(lineage.0.seal(minted, 5)),
        [at(minted, 4), at(D, 9), at(D, 9), at(minted, 3)],
        "reaching 3 would need the seal through 5, which waits for the write at 4"
    );
    assert_eq!(
        lineage.0.sealed_through(minted),
        Some(None),
        "a refused seal changes nothing"
    );
    let due = submitted(lineage.0.seal(minted, 3));
    assert_eq!(seen(&due), [Seen::DurableIn(minted, 3)]);
    assert_eq!(issue(&mut lineage, &due), [w2.seq]);
    assert!(
        quiet(&submitted(lineage.0.seal(minted, 5))),
        "{w1:?} is still held, and the seal waits for it"
    );
}

#[test]
fn a_seal_a_pending_heal_waits_for_can_close_a_cycle() {
    let (mut lineage, minted) = two();
    let mut ops = Ops::new();
    let write = ops.push_to(&mut lineage, minted, 2, A, DEFAULT, &[]);
    lineage.completed(write, FULL);
    submitted(lineage.0.seal(minted, 3));
    let token = tokens(import(&mut lineage)).remove(0);
    assert_eq!(
        seen(&lineage.0.flushed(minted, 3, A, Ok(()))),
        [
            Seen::Marked(0, write.seq, super::COVERED),
            Seen::BlockedIn(minted, 3, 0)
        ]
    );
    lineage
        .resolve(vec![(token, Resolution::Heal)])
        .expect("heal");
    hold_write(&mut lineage, &mut ops, at(minted, 6), at(D, 9));
    hold_write(&mut lineage, &mut ops, at(D, 9), at(minted, 2));
    assert_eq!(
        cycle(lineage.0.seal(minted, 6)),
        [at(minted, 6), at(D, 9), at(D, 9), at(minted, 2)],
        "the heal makes reaching 2 wait for this seal, which waits for the write at 6"
    );
    let due = submitted(lineage.0.seal(minted, 4));
    assert!(due.flushes.is_empty(), "nothing at or below 4 to flush");
    assert_eq!(lineage.failures().len(), 0, "the heal took effect at 4");
}

/// A held write is part of the re-issue a heal waits for (DI-D-46): the releasing seal must reach
/// it, and so waits for it to be released and complete.
#[test]
fn a_held_write_is_part_of_the_re_issue_a_heal_waits_for() {
    let (mut lineage, minted) = two();
    let mut ops = Ops::new();
    let lost = ops.push(&mut lineage, 3, A, DEFAULT);
    lineage.completed(lost, FULL);
    submitted(lineage.seal(3));
    let token = tokens(lineage.flushed(3, A, Err(io::Error::from_raw_os_error(1117)))).remove(0);
    let held = hold_write(&mut lineage, &mut ops, at(D, 8), at(minted, 1));
    lineage
        .resolve(vec![(token, Resolution::Heal)])
        .expect("heal");

    assert_eq!(seen(&submitted(lineage.seal(5))), [Seen::Blocked(5, 0)]);
    assert_eq!(
        lineage.failures().len(),
        1,
        "5 stops short of the held write at 8"
    );
    assert!(
        quiet(&submitted(lineage.seal(8))),
        "8 waits for the held write"
    );

    let due = submitted(lineage.0.seal(minted, 1));
    assert_eq!(issue(&mut lineage, &due), [held.seq]);
    lineage.completed(held, FULL);
    assert_eq!(
        seen(&lineage.flushed(8, A, Ok(()))),
        [
            Seen::Healed(0, vec![lost.seq], vec![]),
            Seen::Durable(3),
            Seen::Durable(5),
            Seen::Durable(8),
        ]
    );
}

/// The cycle search follows the heal to the seal that releases it, not merely the first one made
/// after it (DI-D-46). Here a seal through 5 was made after the heal; under "the first seal after
/// it" reaching 3 would need only that, and pushing the write at `minted:1` would be accepted --
/// a deadlock, since the seal through 10 that does release the heal waits for the held write at 8,
/// which waits for `minted:1`.
#[test]
fn a_push_is_refused_when_the_seal_releasing_a_heal_closes_a_cycle() {
    let (mut lineage, minted) = two();
    let mut ops = Ops::new();
    let lost = ops.push(&mut lineage, 3, A, DEFAULT);
    lineage.completed(lost, FULL);
    submitted(lineage.seal(3));
    let token = tokens(lineage.flushed(3, A, Err(io::Error::from_raw_os_error(1117)))).remove(0);
    let reissued = ops.push(&mut lineage, 10, A, DEFAULT);
    lineage.completed(reissued, FULL);
    lineage
        .resolve(vec![(token, Resolution::Heal)])
        .expect("heal");
    submitted(lineage.seal(5));
    hold_write(&mut lineage, &mut ops, at(D, 8), at(minted, 1));
    submitted(lineage.seal(10));

    assert_eq!(
        lineage.write_cycle(at(minted, 1), at(D, 3)),
        Some(vec![at(minted, 1), at(D, 3), at(D, 8), at(minted, 1)])
    );
}

#[test]
fn a_seal_the_quick_test_suspects_but_that_closes_no_cycle_is_accepted() {
    let (mut lineage, keys) = minted(2);
    let (m, n) = (keys[0], keys[1]);
    let mut ops = Ops::new();
    let in_flight = ops.push_to(&mut lineage, m, 1, A, DEFAULT, &[]);
    hold_write(&mut lineage, &mut ops, at(m, 2), at(n, 1));
    assert!(quiet(&submitted(lineage.0.seal(m, 3))), "waits for 1");
    hold_write(&mut lineage, &mut ops, at(m, 7), at(D, 4));
    hold_write(&mut lineage, &mut ops, at(D, 4), at(m, 3));
    // From every held write of m at or below 8 at once, the search reaches m needing writes
    // through 3, which covers the write at 2; but that write's own gate never comes back to m, and
    // the write at 7 needs m only through 3.
    assert!(quiet(&submitted(lineage.0.seal(m, 8))));
    lineage.completed(in_flight, FULL);
}

#[test]
fn a_flush_that_fails_leaves_a_released_writes_seal_answerable() {
    let (mut lineage, minted) = two();
    let mut ops = Ops::new();
    let write = hold_write(&mut lineage, &mut ops, at(minted, 1), at(D, 1));
    let due = submitted(lineage.seal(1));
    issue(&mut lineage, &due);
    lineage.completed(write, FULL);
    submitted(lineage.0.seal(minted, 1));
    let due = lineage.0.flushed(
        minted,
        1,
        A,
        Err(crate::test_support::fault(io::Error::from_raw_os_error(
            1117,
        ))),
    );
    assert_eq!(seen(&due), [Seen::Failed(0, vec![write.seq])]);
}
