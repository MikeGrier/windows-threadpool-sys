// Copyright (c) 2026 Mike Grier
//! The completion queue within its derived bound (DI-D-112, DI-D-113): a marking is no entry; a
//! `Blocked` withdrawn once its seal is gone; a lineage counted until its `LineageEnded` is
//! popped; and the queue made for `Capacity::entries`. Every call in the rig also checks, inside
//! the core, that the queue keeps the rules the bound rests on.

use std::sync::Arc;

use super::{Core, KEY, Rig, Ticks, V};
use crate::capacity::{AtCapacity, Capacity, Limit};
use crate::diagnostics::NoDiagnostics;
use crate::ids::FailureToken;
use crate::memory::MemoryTarget;
use crate::test_support::CAPACITY;
use crate::types::{Entry, Epoch, ImportScope, Resolution, WriteOptions};

/// A rig over a core built with `CAPACITY` changed by `f`, with `KEY` attached.
fn rig(f: impl FnOnce(&mut Capacity)) -> Rig {
    let mut capacity = CAPACITY;
    f(&mut capacity);
    let core = Core::new(Ticks::default(), Arc::new(NoDiagnostics), capacity).expect("valid");
    let mut rig = Rig::over(core);
    rig.attach(KEY, MemoryTarget::new());
    rig
}

/// Each entry, briefly: its kind and what tells it apart, a lineage by whether it is the default.
fn kinds(rig: &Rig, entries: &[Entry<V, Vec<u8>, &'static str>]) -> Vec<String> {
    let default = rig.core.default_lineage();
    let at = |through: &Epoch<V>| {
        let lineage = if through.lineage == default { "" } else { "L" };
        format!("{lineage}{}", through.id)
    };
    entries
        .iter()
        .map(|entry| match entry {
            Entry::Op(done) => format!("op{}", done.id.seq),
            Entry::Durable { through } => format!("durable {}", at(through)),
            Entry::Blocked { through, by } => format!("blocked {} by f{}", at(through), by.seq),
            Entry::Failed(failed) => format!("failed f{}", failed.id.seq),
            Entry::Abandoned { failure } => format!("abandoned f{}", failure.seq),
            Entry::Healed { failure } => format!("healed f{}", failure.seq),
            Entry::LineageEnded { .. } => "ended".to_owned(),
        })
        .collect()
}

/// Every entry, popped, as [`kinds`] shows it.
fn popped(rig: &mut Rig) -> Vec<String> {
    let entries = rig.entries();
    kinds(rig, &entries)
}

/// Import a failure reaching every uncovered write, and pop its `Failed`: its token.
fn import(rig: &mut Rig) -> FailureToken {
    let (_, actions) = rig
        .core
        .import_failure(ImportScope::All, |_| {})
        .expect("room");
    rig.act(actions);
    match rig.pop_entry() {
        Some(Entry::Failed(failed)) => failed.token,
        other => panic!("expected Failed, got {other:?}"),
    }
}

fn resolve(rig: &mut Rig, token: FailureToken, resolution: Resolution) {
    let actions = rig
        .core
        .resolve(vec![(token, resolution)])
        .expect("a live token");
    rig.act(actions);
}

#[test]
fn the_queue_is_made_for_the_derived_bound() {
    for (served, rig) in [(0, rig(|_| {})), (0, rig(|c| c.operations = 3))] {
        let capacity = rig.core.capacity;
        let bound = capacity.entries(served).expect("fits");
        assert_eq!(
            bound,
            3 * capacity.operations
                + 2 * capacity.failures(served).expect("fits")
                + 3 * capacity.lineages
        );
        assert!(rig.core.queue_room() >= bound, "room for {bound}");
    }
}

/// A marking is no entry (DI-D-113): a write two failures suspect, marked once, queues nothing, and
/// the observer is told of it for each failure, in observation order.
#[test]
fn a_marking_queues_nothing_and_goes_to_the_observer_for_each_failure() {
    let mut rig = rig(|_| {});
    rig.write(1, 0, b"x").expect("room");
    rig.run();
    rig.entries();
    let first = import(&mut rig);
    let second = import(&mut rig);
    rig.seal(1);
    rig.run();
    assert_eq!(rig.core.queued(), 1, "only the Blocked");
    for id in [first.id(), second.id()] {
        assert_eq!(
            rig.told.markings(id).map(|told| told.len()),
            Some(1),
            "told once of the one marking"
        );
    }
    resolve(&mut rig, first, Resolution::Abandon);
    let entries = rig.entries();
    assert_eq!(kinds(&rig, &entries), ["blocked 1 by f0", "abandoned f0"]);
}

/// A failure resolved before a write is marked is not told of the marking.
#[test]
fn a_failure_resolved_before_a_marking_is_not_told_of_it() {
    let mut rig = rig(|_| {});
    rig.write(1, 0, b"x").expect("room");
    rig.run();
    rig.entries();
    let first = import(&mut rig);
    let second = import(&mut rig);
    let (first_id, second_id) = (first.id(), second.id());
    resolve(&mut rig, first, Resolution::Abandon);
    rig.seal(1);
    rig.run();
    assert_eq!(popped(&mut rig), ["abandoned f0", "blocked 1 by f1"]);
    assert_eq!(rig.told.markings(first_id), Some(Vec::new()));
    assert_eq!(rig.told.markings(second_id).map(|told| told.len()), Some(1));
}

/// The `Blocked` entries reach twice the operations -- a recorded seal and the empty seal trailing
/// it, for each write a seal pins -- and the `Durable` resolving the failure withdraws every one
/// not yet popped (DI-D-112).
#[test]
fn the_blocked_entries_fill_to_twice_the_operations_and_a_durable_withdraws_them() {
    let mut rig = rig(|c| c.operations = 2);
    rig.write(1, 0, b"a").expect("room");
    rig.write(3, 8, b"b").expect("room");
    rig.run();
    rig.entries();
    let token = import(&mut rig);
    for through in 1..=4 {
        rig.seal(through);
        rig.run();
    }
    assert_eq!(rig.core.queued(), 4, "four Blocked");
    resolve(&mut rig, token, Resolution::Abandon);
    let entries = rig.entries();
    assert_eq!(
        kinds(&rig, &entries),
        ["abandoned f0", "durable 4"],
        "every Blocked withdrawn by the Durable through 4"
    );
}

/// An empty seal behind a failure replaces the one before it, which is answered `Blocked`; the
/// replaced one's entry is withdrawn, so twenty empty seals leave one (DI-D-112).
#[test]
fn empty_seals_behind_a_failure_leave_one_blocked_for_the_last() {
    let mut rig = rig(|_| {});
    rig.write(1, 0, b"x").expect("room");
    rig.run();
    rig.entries();
    let _token = import(&mut rig);
    rig.seal(1);
    rig.run();
    for through in 2..=21 {
        rig.seal(through);
    }
    assert_eq!(popped(&mut rig), ["blocked 1 by f0", "blocked 21 by f0"]);
}

/// A lineage's end withdraws its un-popped `Blocked`: its seals are gone.
#[test]
fn a_lineage_end_withdraws_its_blocked() {
    let mut rig = rig(|_| {});
    let minted = rig.core.mint_lineage(None).expect("room");
    rig.write_with(KEY, Epoch::new(minted, 1), 0, b"x", WriteOptions::new())
        .expect("room");
    rig.run();
    rig.entries();
    let _token = import(&mut rig);
    rig.seal_in(minted, 1);
    rig.run();
    let actions = rig.core.end_lineage(minted).expect("live");
    rig.act(actions);
    assert_eq!(popped(&mut rig), ["ended"]);
}

/// Minting, sealing and ending a lineage queues a `Durable` and a `LineageEnded` and holds nothing
/// else, so a consumer that never popped could repeat it for ever: the lineage counts until its
/// `LineageEnded` is popped, and the mint after it is refused until then (DI-D-112).
#[test]
fn an_ended_lineage_counts_until_its_end_is_popped_so_the_queue_cannot_grow() {
    let mut rig = rig(|c| c.lineages = 2);
    for _ in 0..3 {
        let minted = rig.core.mint_lineage(None).expect("room");
        rig.seal_in(minted, 1);
        let actions = rig.core.end_lineage(minted).expect("live");
        rig.act(actions);
        assert_eq!(rig.core.queued(), 2, "its Durable, and its LineageEnded");
        assert_eq!(
            rig.core.mint_lineage(None).err(),
            Some(AtCapacity {
                limit: Limit::Lineages,
                declared: 2
            }),
            "drained, but its LineageEnded is not popped"
        );
        assert_eq!(popped(&mut rig), ["durable L1", "ended"]);
    }
}

/// Retiring a live lineage withdraws its un-popped `Durable`: the consumer is done with it, and
/// it no longer counts (DI-D-112).
#[test]
fn retiring_a_live_lineage_withdraws_its_durable() {
    let mut rig = rig(|c| c.lineages = 2);
    let minted = rig.core.mint_lineage(None).expect("room");
    rig.seal_in(minted, 1);
    rig.seal(1);
    rig.core.retire_lineage(minted).expect("nothing holds it");
    assert_eq!(popped(&mut rig), ["durable 1"]);
    rig.core
        .mint_lineage(None)
        .expect("the retired one no longer counts");
}

/// Every kind of entry at once, in a small declaration: completions, a failure, a `Blocked`, a
/// `Durable` replaced, and a `LineageEnded` -- the queue's rules checked after every call, and the
/// entries popped in the order they happened.
#[test]
fn every_kind_of_entry_queued_at_once_pops_in_order() {
    let mut rig = rig(|c| {
        c.operations = 2;
        c.lineages = 2;
        c.imports = 1;
    });
    rig.write(1, 0, b"a").expect("room");
    rig.write(1, 8, b"b").expect("room");
    rig.run();
    let (failure, actions) = rig
        .core
        .import_failure(ImportScope::All, |_| {})
        .expect("room");
    rig.act(actions);
    rig.seal(1);
    rig.run();
    let minted = rig.core.mint_lineage(None).expect("room");
    rig.seal_in(minted, 1);
    rig.seal_in(minted, 2);
    let actions = rig.core.end_lineage(minted).expect("live");
    rig.act(actions);
    assert_eq!(rig.core.queued(), 6);
    assert!(rig.core.queued() <= rig.core.capacity.entries(0).expect("fits"));
    let entries = rig.entries();
    assert_eq!(
        kinds(&rig, &entries),
        [
            "op0",
            "op1",
            format!("failed f{}", failure.failure.seq).as_str(),
            "blocked 1 by f0",
            "durable L2",
            "ended"
        ]
    );
}
