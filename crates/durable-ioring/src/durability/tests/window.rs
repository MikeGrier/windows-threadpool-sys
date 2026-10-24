// Copyright (c) 2026 Mike Grier
//! The submit window's places, as the durability core holds them (DI-D-100, DI-D-107): a write
//! holds its place while it is uncovered, and each unresolved failure suspecting it holds the place
//! until the failure is resolved. Every change here is also checked, inside the core, against a
//! census of the uncovered writes and the failures (`window_agrees`).

use super::{A, B, DEFAULT, FULL, Ops, failed, lineage, submitted, tokens};
use crate::durability::Reach;
use crate::types::{FailureCause, ImportScope, Resolution};

/// A failure the consumer learned of, reaching every uncovered write: its token.
fn import(lineage: &mut super::Lineage) -> crate::ids::FailureToken {
    let (_, due) = lineage.import(
        FailureCause::Imported {
            scope: ImportScope::All,
        },
        &Reach::All,
    );
    tokens(due).remove(0)
}

#[test]
fn a_core_with_no_write_holds_no_place() {
    assert_eq!(lineage().window_places(), 0);
}

#[test]
fn each_uncovered_write_holds_a_place_of_its_own() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    for epoch in 1..=4 {
        ops.push(&mut lineage, epoch, A, DEFAULT);
        assert_eq!(lineage.window_places(), epoch as usize);
    }
}

#[test]
fn a_write_that_fails_lets_go_of_its_place_when_no_failure_suspects_it() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let write = ops.push(&mut lineage, 1, A, DEFAULT);
    assert_eq!(lineage.window_places(), 1);
    lineage.completed(write, failed());
    assert_eq!(lineage.window_places(), 0);
}

#[test]
fn a_write_a_seal_covers_lets_go_of_its_place() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let write = ops.push(&mut lineage, 1, A, DEFAULT);
    lineage.completed(write, FULL);
    submitted(lineage.seal(1));
    assert_eq!(lineage.window_places(), 1, "the flush has not answered");
    lineage.flushed(1, A, Ok(()));
    assert_eq!(lineage.window_places(), 0);
}

#[test]
fn a_failure_holds_the_place_of_a_write_covered_after_it_suspected_it() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let write = ops.push(&mut lineage, 1, A, DEFAULT);
    lineage.completed(write, FULL);
    let token = import(&mut lineage);
    submitted(lineage.seal(1));
    lineage.flushed(1, A, Ok(()));
    assert_eq!(
        lineage.window_places(),
        1,
        "covered, and so no longer uncovered, but still suspected"
    );
    lineage
        .resolve(vec![(token, Resolution::Abandon)])
        .expect("abandon");
    assert_eq!(lineage.window_places(), 0, "the failure was resolved");
}

#[test]
fn a_failure_holds_the_place_of_a_write_that_failed_after_it_suspected_it() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let write = ops.push(&mut lineage, 1, A, DEFAULT);
    let token = import(&mut lineage);
    lineage.completed(write, failed());
    assert_eq!(
        lineage.window_places(),
        1,
        "the write is not uncovered any more, and its failure keeps the place"
    );
    lineage
        .resolve(vec![(token, Resolution::Abandon)])
        .expect("abandon");
    assert_eq!(lineage.window_places(), 0);
}

#[test]
fn each_failure_suspecting_a_write_holds_its_place_until_every_one_is_resolved() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let write = ops.push(&mut lineage, 1, A, DEFAULT);
    lineage.completed(write, FULL);
    let first = import(&mut lineage);
    let second = import(&mut lineage);
    submitted(lineage.seal(1));
    lineage.flushed(1, A, Ok(()));
    assert_eq!(lineage.window_places(), 1, "one place, two failures");
    lineage
        .resolve(vec![(first, Resolution::Abandon)])
        .expect("abandon");
    assert_eq!(lineage.window_places(), 1, "the second still suspects it");
    lineage
        .resolve(vec![(second, Resolution::Abandon)])
        .expect("abandon");
    assert_eq!(lineage.window_places(), 0);
}

#[test]
fn a_healed_failure_lets_go_when_the_heal_takes_effect_and_not_when_it_is_asked_for() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let write = ops.push(&mut lineage, 1, A, DEFAULT);
    lineage.completed(write, FULL);
    let token = import(&mut lineage);
    lineage
        .resolve(vec![(token, Resolution::Heal)])
        .expect("heal");
    assert_eq!(
        lineage.window_places(),
        1,
        "a heal waits for a seal, and holds what the failure held"
    );
    submitted(lineage.seal(1));
    assert_eq!(lineage.window_places(), 1, "the seal's flush is out");
    lineage.flushed(1, A, Ok(()));
    assert!(lineage.failures().is_empty(), "the heal took effect");
    assert_eq!(lineage.window_places(), 0);
}

#[test]
fn a_failure_holds_only_the_writes_it_suspects() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let early = ops.push(&mut lineage, 1, A, DEFAULT);
    lineage.completed(early, FULL);
    let token = import(&mut lineage);
    let late = ops.push(&mut lineage, 2, B, DEFAULT);
    lineage.completed(late, FULL);
    submitted(lineage.seal(2));
    lineage.flushed(2, A, Ok(()));
    lineage.flushed(2, B, Ok(()));
    assert_eq!(
        lineage.window_places(),
        1,
        "the write pushed after the failure was covered, and is not suspected"
    );
    lineage
        .resolve(vec![(token, Resolution::Abandon)])
        .expect("abandon");
    assert_eq!(lineage.window_places(), 0);
}

#[test]
fn ending_a_lineage_lets_go_of_its_completed_writes_and_a_write_in_flight_keeps_its_own() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let minted = lineage.mint(None);
    let done = ops.push_to(&mut lineage, minted, 1, A, DEFAULT, &[]);
    let flying = ops.push_to(&mut lineage, minted, 2, B, DEFAULT, &[]);
    lineage.completed(done, FULL);
    assert_eq!(lineage.window_places(), 2);
    lineage.end(minted);
    assert_eq!(lineage.window_places(), 1, "the one in flight");
    lineage.completed(flying, FULL);
    assert_eq!(lineage.window_places(), 0);
}

#[test]
fn an_ended_lineage_keeps_the_places_of_writes_a_flush_out_was_issued_for_until_it_answers() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let minted = lineage.mint(None);
    let sealed = ops.push_to(&mut lineage, minted, 1, A, DEFAULT, &[]);
    let unsealed = ops.push_to(&mut lineage, minted, 2, B, DEFAULT, &[]);
    lineage.completed(sealed, FULL);
    lineage.completed(unsealed, FULL);
    submitted(lineage.0.seal(minted, 1));
    lineage.end(minted);
    assert_eq!(
        lineage.window_places(),
        1,
        "the write the flush out is for; the unsealed one went with its lineage"
    );
    lineage.0.flushed(minted, 1, A, Ok(()));
    assert_eq!(lineage.window_places(), 0);
}

#[test]
fn a_flush_out_that_fails_lets_go_of_an_ended_lineages_places_all_the_same() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let minted = lineage.mint(None);
    let write = ops.push_to(&mut lineage, minted, 1, A, DEFAULT, &[]);
    lineage.completed(write, FULL);
    submitted(lineage.0.seal(minted, 1));
    lineage.end(minted);
    assert_eq!(lineage.window_places(), 1);
    let fault = super::fault(std::io::Error::from_raw_os_error(1117));
    assert!(super::quiet(&lineage.0.flushed(minted, 1, A, Err(fault))));
    assert_eq!(lineage.window_places(), 0);
    assert!(
        lineage.failures().is_empty(),
        "an ended lineage reports nothing"
    );
}

#[test]
fn an_ended_lineage_lets_go_of_each_files_places_as_that_files_flush_answers() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let minted = lineage.mint(None);
    for (epoch, file) in [(1, A), (1, A), (2, B)] {
        let write = ops.push_to(&mut lineage, minted, epoch, file, DEFAULT, &[]);
        lineage.completed(write, FULL);
    }
    submitted(lineage.0.seal(minted, 2));
    lineage.end(minted);
    assert_eq!(lineage.window_places(), 3);
    lineage.0.flushed(minted, 2, A, Ok(()));
    assert_eq!(lineage.window_places(), 1, "B's is out");
    lineage.0.flushed(minted, 2, B, Ok(()));
    assert_eq!(lineage.window_places(), 0);
}

#[test]
fn a_write_seal_end_loop_holds_places_only_for_the_commits_still_out() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let mut out = Vec::new();
    for round in 1..=8_usize {
        let minted = lineage.mint(None);
        let write = ops.push_to(&mut lineage, minted, 1, A, DEFAULT, &[]);
        lineage.completed(write, FULL);
        submitted(lineage.0.seal(minted, 1));
        lineage.end(minted);
        out.push(minted);
        assert_eq!(lineage.window_places(), round, "one per commit out");
    }
    for (answered, minted) in out.into_iter().enumerate() {
        lineage.0.flushed(minted, 1, A, Ok(()));
        assert_eq!(lineage.window_places(), 7 - answered);
    }
}

#[test]
fn an_ended_lineages_answer_lets_go_only_the_places_of_the_seal_it_answers() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let minted = lineage.mint(None);
    for epoch in [1, 2] {
        let write = ops.push_to(&mut lineage, minted, epoch, A, DEFAULT, &[]);
        lineage.completed(write, FULL);
        submitted(lineage.0.seal(minted, epoch));
    }
    lineage.end(minted);
    assert_eq!(lineage.window_places(), 2, "two seals, one flush of A each");
    lineage.0.flushed(minted, 1, A, Ok(()));
    assert_eq!(lineage.window_places(), 1, "the second seal's is still out");
    lineage.0.flushed(minted, 2, A, Ok(()));
    assert_eq!(lineage.window_places(), 0);
}

#[test]
fn an_ended_lineages_answer_lets_go_nothing_of_another_lineage() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let minted = lineage.mint(None);
    let own = ops.push(&mut lineage, 1, A, DEFAULT);
    let theirs = ops.push_to(&mut lineage, minted, 1, A, DEFAULT, &[]);
    lineage.completed(own, FULL);
    lineage.completed(theirs, FULL);
    submitted(lineage.seal(1));
    submitted(lineage.0.seal(minted, 1));
    lineage.end(minted);
    assert_eq!(lineage.window_places(), 2);
    lineage.0.flushed(minted, 1, A, Ok(()));
    assert_eq!(
        lineage.window_places(),
        1,
        "the default's seal has the same number, and its flush is out"
    );
}

#[test]
fn ending_a_lineage_lets_go_nothing_of_another_lineage() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let minted = lineage.mint(None);
    let own = ops.push(&mut lineage, 1, A, DEFAULT);
    lineage.completed(own, FULL);
    lineage.end(minted);
    assert_eq!(lineage.window_places(), 1);
}

#[test]
fn places_are_counted_across_lineages() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let minted = lineage.mint(None);
    ops.push(&mut lineage, 1, A, DEFAULT);
    ops.push_to(&mut lineage, minted, 1, A, DEFAULT, &[]);
    ops.push_to(&mut lineage, minted, 2, B, DEFAULT, &[]);
    assert_eq!(lineage.window_places(), 3);
}
