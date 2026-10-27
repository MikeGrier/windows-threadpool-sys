// Copyright (c) 2026 Mike Grier
//! The kept writes on their own: kept once each in push order, a release dropping what it picks,
//! and the room made for them refused past.

use super::{Gone, Kept};
use crate::ids::{InstanceId, OpId};
use crate::types::IoTargetKey;

fn op(instance: InstanceId, seq: u64) -> OpId {
    OpId { instance, seq }
}

/// Write `seq`, gone: what the rule reads plays no part in these.
fn gone(instance: InstanceId, seq: u64) -> Gone<u64> {
    Gone {
        op: op(instance, seq),
        lineage: 0,
        epoch: 0,
        file: IoTargetKey(0),
        domains: crate::domain_lists::DomainList::EMPTY,
        born: 0,
        until: 1,
    }
}

#[test]
fn writes_are_read_in_push_order_not_the_order_kept() {
    let instance = InstanceId::next();
    let mut kept = Kept::new(4).expect("small");
    for seq in [3, 1, 2] {
        kept.keep(gone(instance, seq)).expect("room");
    }
    assert_eq!(kept.ops().collect::<Vec<_>>(), [1, 2, 3]);
    let read: Vec<u64> = kept.writes().map(|write| write.op.seq).collect();
    assert_eq!(read, [1, 2, 3]);
}

#[test]
fn a_kept_write_is_found_by_its_push_sequence() {
    let instance = InstanceId::next();
    let mut kept = Kept::new(2).expect("small");
    kept.keep(gone(instance, 7)).expect("room");
    assert_eq!(kept.get(7).map(|write| write.op), Some(op(instance, 7)));
    assert!(kept.get(8).is_none(), "8 was never kept");
}

#[test]
fn a_release_drops_the_writes_it_picks() {
    let instance = InstanceId::next();
    let mut kept = Kept::new(2).expect("small");
    for seq in 1..=2 {
        kept.keep(gone(instance, seq)).expect("room");
    }
    kept.release_where(|write| write.op.seq == 1);
    assert_eq!(kept.ops().collect::<Vec<_>>(), [2]);
    kept.release_where(|write| write.op.seq == 1);
    kept.release_where(|write| write.op.seq == 9);
    assert_eq!(
        kept.ops().count(),
        1,
        "releasing what is not kept changes nothing"
    );
    kept.keep(gone(instance, 3))
        .expect("the released room is taken again");
    assert_eq!(kept.ops().collect::<Vec<_>>(), [2, 3]);
}

#[test]
fn a_write_is_kept_once() {
    let instance = InstanceId::next();
    let mut kept = Kept::new(2).expect("small");
    kept.keep(gone(instance, 1)).expect("room");
    assert_eq!(kept.keep(gone(instance, 1)), Err("a write was kept twice"));
    assert_eq!(kept.ops().count(), 1);
}

#[test]
fn a_write_past_the_places_is_refused() {
    let instance = InstanceId::next();
    let mut kept = Kept::<u64>::new(2).expect("small");
    kept.keep(gone(instance, 1)).expect("room");
    kept.keep(gone(instance, 2)).expect("room");
    assert_eq!(
        kept.keep(gone(instance, 3)),
        Err("more writes were kept than there are places")
    );
    assert_eq!(kept.ops().collect::<Vec<_>>(), [1, 2]);
}
