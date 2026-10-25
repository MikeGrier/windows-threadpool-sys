// Copyright (c) 2026 Mike Grier
//! The kept writes on their own: kept and marked once each, the markings read in the order
//! observed, a release dropping a write's marking with it, and the room made for them refused past.

use super::{Gone, Kept};
use crate::ids::{InstanceId, OpId};
use crate::types::{IoTargetKey, MarkingKind};

/// The test identities, to name the epoch type the markings carry.
type V = crate::ids::DioringIds<u64, u64>;

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

fn covered() -> MarkingKind {
    MarkingKind::Covered {
        flushed: true,
        provider: false,
    }
}

/// The writes `kept`'s markings name, among those `suspects` picks, in the order read.
fn marked(kept: &Kept<u64, u64>, suspects: impl Fn(OpId) -> bool) -> Vec<(u64, u64)> {
    kept.markings(suspects)
        .map(|marking: crate::types::Marking<V>| (marking.write.seq, marking.observed))
        .collect()
}

#[test]
fn markings_are_read_in_the_order_observed_not_the_order_pushed() {
    let instance = InstanceId::next();
    let mut kept = Kept::new(4).expect("small");
    for seq in [3, 1, 2] {
        kept.keep(gone(instance, seq)).expect("room");
    }
    kept.mark(op(instance, 2), 10, covered()).expect("kept");
    kept.mark(op(instance, 3), 20, covered()).expect("kept");
    kept.mark(op(instance, 1), 30, covered()).expect("kept");
    assert_eq!(marked(&kept, |_| true), [(2, 10), (3, 20), (1, 30)]);
}

#[test]
fn a_failure_reads_only_the_markings_of_its_suspects() {
    let instance = InstanceId::next();
    let mut kept = Kept::new(4).expect("small");
    for seq in 1..=3 {
        kept.keep(gone(instance, seq)).expect("room");
        kept.mark(op(instance, seq), seq * 10, covered())
            .expect("kept");
    }
    assert_eq!(marked(&kept, |write| write.seq != 2), [(1, 10), (3, 30)]);
    assert!(marked(&kept, |_| false).is_empty());
}

#[test]
fn a_kept_write_with_no_marking_is_read_as_none() {
    let instance = InstanceId::next();
    let mut kept = Kept::new(2).expect("small");
    kept.keep(gone(instance, 1)).expect("room");
    kept.keep(gone(instance, 2)).expect("room");
    kept.mark(op(instance, 2), 5, covered()).expect("kept");
    assert_eq!(marked(&kept, |_| true), [(2, 5)], "1 was let go unmarked");
}

#[test]
fn a_release_drops_the_write_and_its_marking() {
    let instance = InstanceId::next();
    let mut kept = Kept::new(2).expect("small");
    for seq in 1..=2 {
        kept.keep(gone(instance, seq)).expect("room");
        kept.mark(op(instance, seq), seq, covered()).expect("kept");
    }
    kept.release_where(|write| write.op.seq == 1);
    assert_eq!(marked(&kept, |_| true), [(2, 2)]);
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
    kept.mark(op(instance, 3), 3, covered()).expect("kept");
    assert_eq!(marked(&kept, |_| true), [(2, 2), (3, 3)]);
}

#[test]
fn a_write_is_kept_and_marked_once() {
    let instance = InstanceId::next();
    let mut kept = Kept::new(2).expect("small");
    kept.keep(gone(instance, 1)).expect("room");
    assert_eq!(kept.keep(gone(instance, 1)), Err("a write was kept twice"));
    kept.mark(op(instance, 1), 1, covered()).expect("kept");
    assert_eq!(
        kept.mark(op(instance, 1), 2, covered()),
        Err("a kept write was marked twice")
    );
    assert_eq!(
        marked(&kept, |_| true),
        [(1, 1)],
        "the first marking stands"
    );
}

#[test]
fn a_write_not_kept_cannot_be_marked() {
    let instance = InstanceId::next();
    let mut kept = Kept::<u64, u64>::new(2).expect("small");
    assert_eq!(
        kept.mark(op(instance, 4), 1, covered()),
        Err("a suspected write was marked that was not kept")
    );
    assert!(marked(&kept, |_| true).is_empty());
}

#[test]
fn a_write_past_the_places_is_refused() {
    let instance = InstanceId::next();
    let mut kept = Kept::<u64, u64>::new(2).expect("small");
    kept.keep(gone(instance, 1)).expect("room");
    kept.keep(gone(instance, 2)).expect("room");
    assert_eq!(
        kept.keep(gone(instance, 3)),
        Err("more writes were kept than there are places")
    );
    assert_eq!(kept.ops().collect::<Vec<_>>(), [1, 2]);
}
