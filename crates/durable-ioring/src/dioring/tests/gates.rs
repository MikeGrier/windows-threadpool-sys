// Copyright (c) 2026 Mike Grier
//! Gated operations (DI-3.2.5.2), against real files: held until their gate is durable, released by
//! the call that reports it, ended as never issued when an epoch they need is abandoned, and the
//! cycles a push or a seal would close refused with what they took. Every entry is reported to the
//! conformance oracle, so each test also checks that nothing completes before its gate.

use windows_ioring_sys::RegisteredSpan;

use super::V;
use super::empty;
use super::pushes::{GIVEN, Harness, given, instance, temp};
use crate::contract::{DurableRing, RegisteredBufferRing};
use crate::ids::{FailureToken, LineageHandle, OpId};
use crate::types::{
    DurabilityRequest, Entry, Epoch, ImportScope, OpKind, Outcome, PushRefusal, ReadOptions,
    Resolution, SealError, WriteOptions,
};

/// Write eight bytes at `offset`, tagged with `lineage`'s `epoch`, gated on `gate`.
pub(super) fn write_gated(
    harness: &mut Harness,
    lineage: &LineageHandle,
    offset: u64,
    epoch: u64,
    context: u32,
    gate: Epoch<V>,
) -> OpId {
    let id = harness
        .ring
        .write_with(
            GIVEN,
            offset,
            vec![context as u8; 8],
            lineage.at(epoch),
            context,
            WriteOptions::new().gate(gate),
        )
        .expect("push a gated write");
    let kind = OpKind::Write {
        epoch: Epoch::new(lineage.lineage(), epoch),
    };
    harness
        .oracle
        .pushed_gated(id, kind, context, gate)
        .expect("the oracle accepts the push");
    id
}

pub(super) fn read_gated(harness: &mut Harness, context: u32, gate: Epoch<V>) -> OpId {
    let id = harness
        .ring
        .read_with(GIVEN, 0, vec![0; 8], context, ReadOptions::new().gate(gate))
        .expect("push a gated read");
    harness
        .oracle
        .pushed_gated(id, OpKind::Read, context, gate)
        .expect("the oracle accepts the push");
    id
}

/// The next entry, which must be `Durable` through `through`.
pub(super) fn expect_durable(harness: &mut Harness, through: Epoch<V>) {
    match harness.next_entry() {
        Entry::Durable { through: reported } => assert_eq!(reported, through),
        other => panic!("expected Durable through {through:?}, got {other:?}"),
    }
}

/// The next entry, which must be `id`'s completion.
pub(super) fn expect_done(harness: &mut Harness, id: OpId) -> Outcome<V> {
    match harness.next_entry() {
        Entry::Op(done) if done.id == id => done.outcome,
        other => panic!("expected {id:?}'s completion, got {other:?}"),
    }
}

/// An instance with one file, and a lineage minted beside its default.
pub(super) fn two(temp: &super::TempFile) -> (Harness, LineageHandle) {
    let mut harness = Harness::new(instance(vec![given(GIVEN, temp)], vec![vec![0; 16]]));
    let minted = harness.ring.mint_lineage(None);
    (harness, minted)
}

/// Import a failure on the whole instance, and abandon it.
fn import_and_abandon(harness: &mut Harness) {
    harness
        .ring
        .import_failure(ImportScope::All)
        .expect("import");
    let token: FailureToken = match harness.next_entry() {
        Entry::Failed(failed) => failed.token,
        other => panic!("expected Failed, got {other:?}"),
    };
    harness
        .ring
        .resolve(vec![(token, Resolution::Abandon)])
        .expect("abandon");
    assert!(matches!(harness.next_entry(), Entry::Abandoned { .. }));
}

#[test]
fn a_gated_write_waits_for_its_gate_and_completes_after_it() {
    let temp = temp(&[0; 64]);
    let (mut harness, minted) = two(&temp);
    let gate = harness.epoch(2);
    let write = write_gated(&mut harness, &minted, 0, 1, 1, gate);
    assert!(harness.ring.pop().expect("pop").is_none(), "held");
    assert_eq!(harness.seal(1), DurabilityRequest::Submitted);
    let one = harness.epoch(1);
    expect_durable(&mut harness, one);
    assert!(
        harness.ring.pop().expect("pop").is_none(),
        "1 is below the gate"
    );

    assert_eq!(harness.seal(2), DurabilityRequest::Submitted);
    expect_durable(&mut harness, gate);
    assert!(matches!(
        expect_done(&mut harness, write),
        Outcome::Transferred(8)
    ));
    assert_eq!(harness.seal_in(&minted, 1), DurabilityRequest::Submitted);
    expect_durable(&mut harness, Epoch::new(minted.lineage(), 1));
    harness.finish();
}

#[test]
fn a_gate_already_durable_holds_nothing() {
    let temp = temp(&[0; 64]);
    let (mut harness, minted) = two(&temp);
    harness.seal(3);
    let three = harness.epoch(3);
    expect_durable(&mut harness, three);
    let gate = harness.epoch(2);
    let write = write_gated(&mut harness, &minted, 0, 1, 1, gate);
    let read = read_gated(&mut harness, 2, gate);
    let mut done = [harness.next().id.seq, harness.next().id.seq];
    done.sort_unstable();
    assert_eq!(done, [write.seq, read.seq]);
    harness.finish();
}

#[test]
fn a_seal_waits_for_the_held_write_it_covers() {
    let temp = temp(&[0; 64]);
    let (mut harness, minted) = two(&temp);
    let gate = harness.epoch(5);
    let write = write_gated(&mut harness, &minted, 0, 1, 1, gate);
    assert_eq!(harness.seal_in(&minted, 2), DurabilityRequest::Submitted);
    assert!(
        harness.ring.pop().expect("pop").is_none(),
        "no Durable while the write is held"
    );
    harness.seal(5);
    expect_durable(&mut harness, gate);
    expect_done(&mut harness, write);
    expect_durable(&mut harness, Epoch::new(minted.lineage(), 2));
    harness.finish();
}

#[test]
fn gated_span_operations_are_released_too() {
    let temp = temp(&[0; 64]);
    let (mut harness, minted) = two(&temp);
    let gate = harness.epoch(1);
    let span = RegisteredSpan {
        buffer_index: 0,
        offset: 0,
        len: 4,
    };
    let write = harness
        .ring
        .write_registered_with(
            GIVEN,
            0,
            span,
            minted.at(1),
            1,
            WriteOptions::new().gate(gate),
        )
        .expect("a gated span write");
    let kind = OpKind::Write {
        epoch: Epoch::new(minted.lineage(), 1),
    };
    harness
        .oracle
        .pushed_gated(write, kind, 1, gate)
        .expect("ok");
    let read = harness
        .ring
        .read_registered_with(GIVEN, 0, span, 2, ReadOptions::new().gate(gate))
        .expect("a gated span read");
    harness
        .oracle
        .pushed_gated(read, OpKind::Read, 2, gate)
        .expect("ok");
    assert!(harness.ring.pop().expect("pop").is_none());
    harness.seal(1);
    expect_durable(&mut harness, gate);
    let mut done: Vec<_> = harness.next_n(2).into_iter().map(|c| c.id.seq).collect();
    done.sort_unstable();
    assert_eq!(done, [write.seq, read.seq]);
    harness.finish();
}

#[test]
fn abandoning_a_gate_ends_what_waits_for_it_with_what_it_took() {
    let temp = temp(&[0; 64]);
    let (mut harness, minted) = two(&temp);
    // Suspected by the import, so abandoning it abandons 1.
    harness.write(GIVEN, 16, b"suspect", 1, 9);
    harness.next();
    let gate = harness.epoch(1);
    let write = write_gated(&mut harness, &minted, 0, 1, 1, gate);
    let read = read_gated(&mut harness, 2, gate);
    import_and_abandon(&mut harness);
    for (id, context, buffer) in [(write, 1, vec![1; 8]), (read, 2, vec![0; 8])] {
        match harness.next_entry() {
            Entry::Op(done) => {
                assert_eq!(done.id, id);
                assert!(
                    matches!(done.outcome, Outcome::NeverIssued { abandoned } if abandoned == gate)
                );
                assert_eq!((done.buffer, done.context), (Some(buffer), context));
            }
            other => panic!("expected NeverIssued, got {other:?}"),
        }
    }
    let error = harness
        .ring
        .read_with(GIVEN, 0, vec![3], 3, ReadOptions::new().gate(gate))
        .expect_err("a gate on an abandoned epoch");
    assert!(matches!(error.reason, PushRefusal::GateAbandoned { gate: g } if g == gate));
    assert_eq!((error.buffer, error.context), (Some(vec![3]), 3));
    harness.finish();
}

#[test]
fn a_gate_on_a_lineage_the_instance_cannot_answer_for_is_refused() {
    let temp = temp(&[0; 64]);
    let (mut harness, minted) = two(&temp);
    let foreign = empty().default_lineage().lineage();
    let ended = minted.lineage();
    drop(minted);
    assert!(matches!(harness.next_entry(), Entry::LineageEnded { .. }));
    for lineage in [foreign, ended] {
        let error = harness
            .ring
            .read_with(
                GIVEN,
                0,
                vec![4],
                4,
                ReadOptions::new().gate(Epoch::new(lineage, 1)),
            )
            .expect_err("a gate it cannot answer for");
        assert!(matches!(error.reason, PushRefusal::UnknownLineage(l) if l == lineage));
        assert_eq!((error.buffer, error.context), (Some(vec![4]), 4));
    }
    harness.finish();
}

#[test]
fn ending_a_lineage_ends_its_held_writes_and_what_waits_for_it() {
    let temp = temp(&[0; 64]);
    let (mut harness, minted) = two(&temp);
    let lineage = minted.lineage();
    let five = harness.epoch(5);
    let own = write_gated(&mut harness, &minted, 0, 7, 1, five);
    let waiting = read_gated(&mut harness, 2, Epoch::new(lineage, 3));
    drop(minted);
    match harness.next_entry() {
        Entry::LineageEnded {
            abandoned_through, ..
        } => assert_eq!(abandoned_through, Some(7)),
        other => panic!("expected LineageEnded, got {other:?}"),
    }
    for (id, abandoned) in [
        (own, Epoch::new(lineage, 7)),
        (waiting, Epoch::new(lineage, 3)),
    ] {
        assert!(matches!(
            expect_done(&mut harness, id),
            Outcome::NeverIssued { abandoned: a } if a == abandoned
        ));
    }
    harness.finish();
}

#[test]
fn a_push_that_would_close_a_cycle_is_refused_with_what_it_took() {
    let temp = temp(&[0; 64]);
    let (mut harness, minted) = two(&temp);
    let m = |id| Epoch::new(minted.lineage(), id);
    let nine = harness.epoch(9);
    write_gated(&mut harness, &minted, 0, 4, 1, nine);
    let default = harness.default.clone();
    let error = harness
        .ring
        .write_with(
            GIVEN,
            8,
            vec![2; 8],
            default.at(9),
            2,
            WriteOptions::new().gate(m(4)),
        )
        .expect_err("a cycle");
    match error.reason {
        PushRefusal::GateCycle { cycle } => {
            assert_eq!(cycle, [nine, m(4), m(4), nine]);
        }
        other => panic!("expected GateCycle, got {other:?}"),
    }
    assert_eq!((error.buffer, error.context), (Some(vec![2; 8]), 2));
    assert!(harness.ring.pop().expect("pop").is_none());
    // The write still held at 4 ends with its lineage, so the stream finishes.
    drop(minted);
    assert!(matches!(harness.next_entry(), Entry::LineageEnded { .. }));
    assert!(matches!(
        harness.next_entry(),
        Entry::Op(done) if matches!(done.outcome, Outcome::NeverIssued { .. })
    ));
    harness.finish();
}

#[test]
fn a_seal_that_would_close_a_cycle_is_refused_and_a_lower_one_untangles_it() {
    let temp = temp(&[0; 64]);
    let (mut harness, minted) = two(&temp);
    let m = |id| Epoch::new(minted.lineage(), id);
    let nine = harness.epoch(9);
    let w1 = write_gated(&mut harness, &minted, 0, 4, 1, nine);
    let default = harness.default.clone();
    let w2 = write_gated(&mut harness, &default, 8, 9, 2, m(3));
    assert_eq!(
        harness.ring.make_durable_through(minted.at(5)),
        Err(SealError::GateCycle {
            cycle: vec![m(4), nine, nine, m(3)]
        })
    );
    assert_eq!(
        harness.ring.sealed_through(minted.lineage()),
        Ok(None),
        "nothing was sealed"
    );
    harness.seal_in(&minted, 3);
    expect_durable(&mut harness, m(3));
    expect_done(&mut harness, w2);
    harness.seal(9);
    expect_durable(&mut harness, nine);
    expect_done(&mut harness, w1);
    harness.seal_in(&minted, 5);
    expect_durable(&mut harness, m(5));
    harness.finish();
}
