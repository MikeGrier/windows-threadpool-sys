// Copyright (c) 2026 Mike Grier
//! The submit window's places, as the I/O core holds them (DI-D-100, DI-D-107): an operation holds
//! its place from its push until its completion is popped, a write also while it is uncovered,
//! and a failure also while it is unresolved; a place is one, however many hold it. Every call into
//! the core here is also checked, inside it, against what it has in flight, held and queued.

use super::{KEY, Rig, V};
use crate::ids::FailureToken;
use crate::memory::{Fault, FaultScript, ScriptOp, When};
use crate::types::{Entry, Epoch, ImportScope, PushRefusal, ReadOptions, Resolution, WriteOptions};

/// The tokens of the failures in the queue, draining it.
fn tokens(rig: &mut Rig) -> Vec<FailureToken> {
    rig.entries()
        .into_iter()
        .filter_map(|entry| match entry {
            Entry::Failed(failed) => Some(failed.token),
            _ => None,
        })
        .collect()
}

fn gated(gate: Epoch<V>) -> WriteOptions<V> {
    WriteOptions::new().gate(gate)
}

#[test]
fn an_ended_lineages_write_keeps_its_place_until_the_commit_out_for_it_completes() {
    let script = FaultScript::new().on(When::any().op(ScriptOp::Commit), Fault::Hold);
    let mut rig = Rig::scripted(script);
    let minted = rig.core.mint_lineage(None);
    rig.write_with(KEY, Epoch::new(minted, 1), 0, b"x", Default::default())
        .expect("accepted");
    rig.run();
    rig.entries();
    rig.seal_in(minted, 1);
    rig.run();
    assert_eq!(rig.backend.held().len(), 1, "the commit is out");
    let actions = rig.core.end_lineage(minted).expect("live, not the default");
    rig.act(actions);
    assert_eq!(
        rig.core.window_places(),
        1,
        "the lineage ended, and its commit has not answered"
    );
    rig.backend.release_all();
    rig.run();
    rig.entries();
    assert_eq!(rig.core.window_places(), 0, "the commit answered");
}

#[test]
fn a_new_core_holds_no_place() {
    assert_eq!(Rig::new().core.window_places(), 0);
}

#[test]
fn a_read_holds_its_place_until_its_completion_is_popped() {
    let mut rig = Rig::new();
    rig.read(0, 4).expect("accepted");
    assert_eq!(rig.core.window_places(), 1, "pushed");
    rig.run();
    assert_eq!(rig.core.window_places(), 1, "completed, and queued");
    rig.entries();
    assert_eq!(rig.core.window_places(), 0, "popped");
}

#[test]
fn a_write_holds_its_place_until_it_is_covered_and_its_completion_popped() {
    let mut rig = Rig::new();
    rig.write(1, 0, b"x").expect("accepted");
    assert_eq!(rig.core.window_places(), 1);
    rig.run();
    rig.entries();
    assert_eq!(
        rig.core.window_places(),
        1,
        "popped, and still uncovered: the core keeps its record"
    );
    rig.seal(1);
    rig.run();
    rig.entries();
    assert_eq!(rig.core.window_places(), 0);
}

#[test]
fn a_write_covered_before_its_completion_is_popped_keeps_its_place_until_it_is() {
    let mut rig = Rig::new();
    rig.write(1, 0, b"x").expect("accepted");
    rig.seal(1);
    rig.run();
    assert_eq!(
        rig.core.window_places(),
        1,
        "covered by its seal, and its completion is still queued"
    );
    rig.entries();
    assert_eq!(rig.core.window_places(), 0);
}

#[test]
fn places_are_counted_per_operation_across_reads_and_writes() {
    let mut rig = Rig::new();
    rig.write(1, 0, b"a").expect("accepted");
    rig.write(1, 1, b"b").expect("accepted");
    rig.read(0, 1).expect("accepted");
    assert_eq!(rig.core.window_places(), 3);
    rig.run();
    rig.entries();
    assert_eq!(
        rig.core.window_places(),
        2,
        "the read's is gone; the writes are uncovered"
    );
}

#[test]
fn a_refused_push_takes_no_place() {
    let mut rig = Rig::new();
    rig.seal(3);
    rig.run();
    rig.entries();
    let refused = rig.write(2, 0, b"x").expect_err("at or below the seal");
    assert!(matches!(refused.reason, PushRefusal::Sealed { .. }));
    let unknown = rig
        .write_to(super::OTHER, 5, 0, b"x")
        .expect_err("no such target");
    assert!(matches!(unknown.reason, PushRefusal::UnknownFile(_)));
    assert_eq!(rig.core.window_places(), 0);
}

#[test]
fn an_operation_held_for_its_gate_holds_a_place_while_it_waits_and_after_it_is_released() {
    let mut rig = Rig::new();
    rig.write(1, 0, b"a").expect("accepted");
    let gate = rig.epoch(1);
    rig.read_with(0, 1, ReadOptions::new().gate(gate))
        .expect("held");
    rig.write_with(KEY, rig.epoch(2), 1, b"b", gated(gate))
        .expect("held");
    assert_eq!(rig.core.window_places(), 3, "one pushed, two held");
    rig.seal(1);
    rig.run();
    rig.entries();
    assert_eq!(
        rig.core.window_places(),
        1,
        "the first write and the read are done; the gated write at 2 is uncovered"
    );
    rig.seal(2);
    rig.run();
    rig.entries();
    assert_eq!(rig.core.window_places(), 0);
}

#[test]
fn an_operation_that_never_issues_holds_its_place_until_that_completion_is_popped() {
    let mut rig = Rig::new();
    rig.write(1, 0, b"x").expect("accepted");
    rig.run();
    rig.entries();
    let gate = rig.epoch(1);
    rig.write_with(KEY, rig.epoch(2), 0, b"held", gated(gate))
        .expect("held");
    let (_, actions) = rig.core.import_failure(ImportScope::All).expect("in scope");
    rig.act(actions);
    let token = tokens(&mut rig).remove(0);
    let actions = rig
        .core
        .resolve(vec![(token, Resolution::Abandon)])
        .expect("our token");
    rig.act(actions);
    let before = rig.core.window_places();
    let entries = rig.entries();
    assert!(
        entries.iter().any(|entry| matches!(entry, Entry::Op(_))),
        "the held write's NeverIssued completion is queued: {entries:?}"
    );
    assert_eq!(
        rig.core.window_places(),
        before - 1,
        "popping it let go of that operation's place and no other"
    );
}

#[test]
fn a_failure_holds_the_places_of_the_writes_it_suspects_after_they_are_covered_and_popped() {
    let mut rig = Rig::new();
    rig.write(1, 0, b"x").expect("accepted");
    rig.run();
    rig.entries();
    let (_, actions) = rig.core.import_failure(ImportScope::All).expect("in scope");
    rig.act(actions);
    let token = tokens(&mut rig).remove(0);
    rig.seal(1);
    rig.run();
    rig.entries();
    assert_eq!(
        rig.core.window_places(),
        1,
        "covered and popped, but a failure still suspects it"
    );
    let actions = rig
        .core
        .resolve(vec![(token, Resolution::Abandon)])
        .expect("our token");
    rig.act(actions);
    rig.entries();
    assert_eq!(
        rig.core.window_places(),
        0,
        "resolving the failure cleared it"
    );
}

#[test]
fn a_consumer_that_leaves_failures_unresolved_keeps_the_places_they_hold() {
    let mut rig = Rig::new();
    let mut tokens_held = Vec::new();
    for epoch in 1..=3 {
        rig.write(epoch, u64::from(epoch as u8), b"x")
            .expect("accepted");
        rig.run();
        rig.entries();
        let (_, actions) = rig.core.import_failure(ImportScope::All).expect("in scope");
        rig.act(actions);
        tokens_held.extend(tokens(&mut rig));
    }
    rig.seal(3);
    rig.run();
    rig.entries();
    assert_eq!(
        rig.core.window_places(),
        3,
        "three writes, each held by the failures that suspected it"
    );
    for token in tokens_held {
        let actions = rig
            .core
            .resolve(vec![(token, Resolution::Abandon)])
            .expect("our token");
        rig.act(actions);
    }
    rig.entries();
    assert_eq!(rig.core.window_places(), 0);
}

#[test]
fn a_failure_holds_the_place_of_a_write_that_was_in_flight_when_it_was_observed() {
    let mut rig = Rig::new();
    rig.write(1, 0, b"x").expect("accepted");
    let (_, actions) = rig.core.import_failure(ImportScope::All).expect("in scope");
    rig.act(actions);
    let token = tokens(&mut rig).remove(0);
    rig.run();
    rig.entries();
    rig.seal(1);
    rig.run();
    rig.entries();
    assert_eq!(
        rig.core.window_places(),
        1,
        "the failure suspected it while it was in flight"
    );
    let actions = rig
        .core
        .resolve(vec![(token, Resolution::Abandon)])
        .expect("our token");
    rig.act(actions);
    rig.entries();
    assert_eq!(rig.core.window_places(), 0);
}
