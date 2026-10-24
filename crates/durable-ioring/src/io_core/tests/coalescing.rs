// Copyright (c) 2026 Mike Grier
//! A lineage has at most one un-popped `Durable` (DI-D-101): a later one removes the older and goes
//! to the tail. Every `act` in the rig also checks, inside the core, that no queue holds two for
//! one lineage.

use super::{Rig, shown};
use crate::types::Entry;

/// How many `Durable` entries `entries` holds.
fn durables(entries: &[Entry<super::V, Vec<u8>, &'static str>]) -> usize {
    entries
        .iter()
        .filter(|entry| matches!(entry, Entry::Durable { .. }))
        .count()
}

#[test]
fn a_later_durable_replaces_an_older_one_nobody_has_popped() {
    let mut rig = Rig::new();
    rig.write(1, 0, b"a").expect("accepted");
    rig.seal(1);
    rig.run();
    rig.write(2, 1, b"b").expect("accepted");
    rig.seal(2);
    rig.run();
    assert_eq!(
        shown(&mut rig),
        ["op0 write@1 1 write", "op1 write@2 1 write", "durable 2"],
        "the Durable through 1 was removed, and the one through 2 stands at the tail"
    );
}

#[test]
fn a_durable_already_popped_is_not_replaced_so_the_consumer_sees_both() {
    let mut rig = Rig::new();
    rig.write(1, 0, b"a").expect("accepted");
    rig.seal(1);
    rig.run();
    assert_eq!(shown(&mut rig), ["op0 write@1 1 write", "durable 1"]);
    rig.write(2, 1, b"b").expect("accepted");
    rig.seal(2);
    rig.run();
    assert_eq!(shown(&mut rig), ["op1 write@2 1 write", "durable 2"]);
}

#[test]
fn the_replacing_durable_goes_to_the_tail_not_into_the_olders_place() {
    let mut rig = Rig::new();
    rig.write(1, 0, b"a").expect("accepted");
    rig.seal(1);
    rig.run();
    // The queue holds the first write's completion and the Durable through 1. Another write
    // completes behind them, then its seal is answered.
    rig.write(2, 1, b"b").expect("accepted");
    rig.run();
    rig.seal(2);
    rig.run();
    let entries = rig.entries();
    assert_eq!(durables(&entries), 1);
    assert!(
        matches!(entries.last(), Some(Entry::Durable { through }) if through.id == 2),
        "the Durable is last, after the completion that came between them: {entries:?}"
    );
}

#[test]
fn each_lineage_keeps_its_own_un_popped_durable() {
    let mut rig = Rig::new();
    let minted = rig.core.mint_lineage(None);
    rig.seal(1);
    rig.seal_in(minted, 1);
    rig.seal(2);
    let entries = rig.entries();
    let through: Vec<_> = entries
        .iter()
        .filter_map(|entry| match entry {
            Entry::Durable { through } => Some((through.lineage == minted, through.id)),
            _ => None,
        })
        .collect();
    assert_eq!(
        through,
        [(true, 1), (false, 2)],
        "the minted lineage's, and the default's replaced once: {entries:?}"
    );
}

#[test]
fn a_replacing_durable_does_not_raise_readiness_again() {
    let mut rig = Rig::new();
    let (_, first) = rig.sealed(rig.epoch(1));
    assert!(first.deliver, "the queue became non-empty");
    rig.act(first);
    let (_, second) = rig.sealed(rig.epoch(2));
    assert!(
        !second.deliver,
        "the queue held an entry before and still does: readiness is already raised"
    );
    rig.act(second);
    let entries = rig.entries();
    assert_eq!(durables(&entries), 1);
    assert!(matches!(
        entries.as_slice(),
        [Entry::Durable { through }] if through.id == 2
    ));
}

#[test]
fn many_seals_answered_before_a_pop_leave_one_durable_naming_the_last() {
    let mut rig = Rig::new();
    for epoch in 1..=20 {
        rig.seal(epoch);
    }
    let entries = rig.entries();
    assert_eq!(durables(&entries), 1);
    assert!(matches!(
        entries.as_slice(),
        [Entry::Durable { through }] if through.id == 20
    ));
    assert_eq!(
        rig.core.durable_through(rig.core.default_lineage()),
        Ok(Some(20)),
        "the mark itself is not coalesced"
    );
}

#[test]
fn a_blocked_answer_is_not_replaced_only_durable_is() {
    let mut rig = Rig::new();
    rig.write(1, 0, b"x").expect("accepted");
    rig.run();
    rig.entries();
    let (_, actions) = rig
        .core
        .import_failure(crate::types::ImportScope::All)
        .expect("in scope");
    rig.act(actions);
    rig.seal(1);
    rig.run();
    rig.seal(2);
    rig.run();
    let entries = rig.entries();
    let blocked = entries
        .iter()
        .filter(|entry| matches!(entry, Entry::Blocked { .. }))
        .count();
    assert!(
        blocked >= 1,
        "a seal held by a failure is answered Blocked: {entries:?}"
    );
    assert_eq!(durables(&entries), 0);
}
