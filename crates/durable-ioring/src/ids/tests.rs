// Copyright (c) 2026 Mike Grier

use std::cmp::Ordering;
use std::collections::HashSet;
use std::sync::{Arc, Mutex, Weak};

use crate::contract::Identities;
use crate::ids::{
    DioringIds, FailureId, FailureToken, Home, InstanceId, Lineage, LineageHandle, LineageSlots,
    OpId,
};

/// A home with `slots` slots, recording every lineage released to it.
struct Recording {
    slots: LineageSlots,
    released: Mutex<Vec<u64>>,
}

impl Home for Recording {
    fn release(&self, lineage: Lineage) {
        self.released.lock().expect("unpoisoned").push(lineage.seq);
    }

    fn slots(&self) -> &LineageSlots {
        &self.slots
    }
}

fn recording(slots: usize) -> (Arc<Recording>, Weak<dyn Home>) {
    let home = Arc::new(Recording {
        slots: LineageSlots::new(slots).expect("small"),
        released: Mutex::new(Vec::new()),
    });
    let weak: Weak<Recording> = Arc::downgrade(&home);
    (home, weak)
}

fn released(home: &Recording) -> Vec<u64> {
    home.released.lock().expect("unpoisoned").clone()
}

fn lineage(instance: InstanceId, seq: u64) -> Lineage {
    Lineage { instance, seq }
}

#[test]
fn the_last_copy_released_ends_its_lineage_once() {
    let instance = InstanceId::next();
    let (home, weak) = recording(2);
    let handle = LineageHandle::mint(lineage(instance, 5), weak).expect("a slot");
    let copies: Vec<_> = (0..3).map(|_| handle.clone()).collect();
    assert_eq!(home.slots.in_use(), 1, "copies share the one slot");
    assert!(!handle.is_last());
    drop(copies);
    assert!(released(&home).is_empty(), "a copy is left");
    assert!(handle.is_last());
    drop(handle);
    assert_eq!(released(&home), [5]);
    assert_eq!(home.slots.in_use(), 0, "the slot is free again");
}

#[test]
fn taking_the_last_copy_ends_nothing_and_frees_the_slot() {
    let instance = InstanceId::next();
    let (home, weak) = recording(1);
    let handle = LineageHandle::mint(lineage(instance, 3), weak.clone()).expect("a slot");
    let copy = handle.clone();
    let handle = handle.take_last().expect_err("a copy is out");
    drop(copy);
    assert_eq!(handle.take_last().expect("the last"), lineage(instance, 3));
    assert!(released(&home).is_empty(), "taken, not released");
    assert_eq!(home.slots.in_use(), 0);
    let again = LineageHandle::mint(lineage(instance, 4), weak).expect("the freed slot");
    drop(again);
    assert_eq!(released(&home), [4]);
}

#[test]
fn a_mint_with_every_slot_in_use_is_refused() {
    let instance = InstanceId::next();
    let (home, weak) = recording(2);
    let first = LineageHandle::mint(lineage(instance, 1), weak.clone()).expect("a slot");
    let second = LineageHandle::mint(lineage(instance, 2), weak.clone()).expect("a slot");
    assert!(LineageHandle::mint(lineage(instance, 3), weak.clone()).is_none());
    drop(first);
    let third = LineageHandle::mint(lineage(instance, 3), weak).expect("the first's slot");
    assert_eq!(home.slots.in_use(), 2);
    drop((second, third));
    assert_eq!(released(&home), [1, 2, 3]);
}

#[test]
fn a_handle_whose_home_is_gone_counts_and_ends_nothing() {
    let instance = InstanceId::next();
    let (home, weak) = recording(1);
    let handle = LineageHandle::mint(lineage(instance, 1), weak.clone()).expect("a slot");
    drop(home);
    let copy = handle.clone();
    assert!(!handle.is_last(), "no count to read");
    let handle = handle.take_last().expect_err("nothing to take");
    drop((handle, copy));
    assert!(
        LineageHandle::mint(lineage(instance, 2), weak).is_none(),
        "no home to mint at"
    );
}

#[test]
fn a_detached_handle_counts_nothing_and_is_never_the_last() {
    let instance = InstanceId::next();
    let handle = LineageHandle::detached(lineage(instance, 0));
    assert_eq!(handle.lineage(), lineage(instance, 0));
    assert!(!handle.is_last());
    let handle = handle.take_last().expect_err("never the last");
    drop(handle.clone());
    assert_eq!(handle.lineage().seq, 0);
}

#[test]
fn a_slot_reclaimed_by_another_lineage_is_not_counted_by_the_old_ones_number() {
    let instance = InstanceId::next();
    let (home, weak) = recording(1);
    let first = LineageHandle::mint(lineage(instance, 1), weak.clone()).expect("a slot");
    let lineage_one = first.take_last().expect("the last");
    let second = LineageHandle::mint(lineage(instance, 2), weak.clone()).expect("the slot");
    // A handle naming the slot under the first lineage's number, as a stale one would.
    let stale = LineageHandle {
        home: weak,
        lineage: lineage_one,
        slot: 0,
    };
    assert!(!stale.is_last(), "the generation does not match");
    drop(stale);
    assert!(released(&home).is_empty(), "the stale drop ended nothing");
    assert!(
        second.is_last(),
        "and did not count down the slot's lineage"
    );
    drop(second);
    assert_eq!(released(&home), [2]);
}

#[test]
fn every_instance_id_is_new() {
    let ids: Vec<InstanceId> = (0..100).map(|_| InstanceId::next()).collect();
    let distinct: HashSet<_> = ids.iter().copied().collect();
    assert_eq!(distinct.len(), ids.len());
}

#[test]
fn operations_order_by_push_order_within_an_instance() {
    let instance = InstanceId::next();
    let first = OpId { instance, seq: 1 };
    let second = OpId { instance, seq: 2 };
    assert_eq!(first.partial_cmp(&second), Some(Ordering::Less));
    assert!(first < second);
    assert_eq!(first.partial_cmp(&first), Some(Ordering::Equal));
}

#[test]
fn operations_of_two_instances_are_incomparable() {
    let a = OpId {
        instance: InstanceId::next(),
        seq: 1,
    };
    let b = OpId {
        instance: InstanceId::next(),
        seq: 2,
    };
    assert_eq!(a.partial_cmp(&b), None);
    assert_ne!(a, b);
}

#[test]
fn identities_carry_their_instance() {
    let (one, two) = (InstanceId::next(), InstanceId::next());
    assert_ne!(
        Lineage {
            instance: one,
            seq: 0
        },
        Lineage {
            instance: two,
            seq: 0
        }
    );
    assert_ne!(
        FailureId {
            instance: one,
            seq: 0
        },
        FailureId {
            instance: two,
            seq: 0
        }
    );
}

#[test]
fn a_token_names_its_failure() {
    let id = FailureId {
        instance: InstanceId::next(),
        seq: 7,
    };
    let (token, _) = FailureToken::mint(id);
    assert_eq!(token.id(), id);
    assert_eq!(<DioringIds<u64, u64> as Identities>::token_id(&token), id);
}

#[test]
fn a_token_is_live_until_it_is_closed_or_dropped() {
    let id = FailureId {
        instance: InstanceId::next(),
        seq: 0,
    };
    let (token, live) = FailureToken::mint(id);
    assert!(live.load(std::sync::atomic::Ordering::Acquire));
    token.close();
    assert!(!live.load(std::sync::atomic::Ordering::Acquire));

    let (token, live) = FailureToken::mint(id);
    drop(token);
    assert!(!live.load(std::sync::atomic::Ordering::Acquire));
}

/// An epoch-id type with no `Hash` and no `Default`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Bare(u8);

#[test]
fn the_marker_is_copy_eq_and_hash_whatever_the_epoch_id() {
    let a = DioringIds::<Bare, u64>(std::marker::PhantomData);
    let b = a;
    assert_eq!(a, b);
    let set: HashSet<_> = [a, b].into_iter().collect();
    assert_eq!(set.len(), 1);
    assert_eq!(format!("{a:?}"), "DioringIds");
    assert_eq!(size_of::<DioringIds<Bare, u64>>(), 0);
}
