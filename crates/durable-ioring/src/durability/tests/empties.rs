// Copyright (c) 2026 Mike Grier
//! Seals that cover no writes (DI-D-99): kept in a slot rather than recorded, overwritten by a later
//! empty seal, whose pair ends `Superseded`, and evaluated in order -- after every seal before them.

use std::io;
use std::sync::Arc;

use super::{
    A, B, DEFAULT, FULL, One, Ops, Seen, State, durable, failed, flush, lineage, quiet, seen,
    submitted, tokens,
};
use crate::diagnostics::tests::Recording;
use crate::domains::DomainId;
use crate::durability::Reach;
use crate::test_support::ManualClock;
use crate::types::{FailureCause, ImportScope, Resolution};

/// A core whose diagnostic sink records every event it is given.
fn recorded() -> (One, Arc<Recording>) {
    let sink = Recording::wanting();
    (One::reporting(ManualClock::default(), sink.clone()), sink)
}

/// A flush that failed.
fn bad() -> io::Result<()> {
    Err(io::Error::from_raw_os_error(1117))
}

/// How many seal pairs the sink saw end with `outcome`.
fn ended(sink: &Recording, outcome: &str) -> usize {
    sink.seen()
        .iter()
        .filter(|event| event.contains("seal") && event.ends_with(outcome))
        .count()
}

#[test]
fn a_later_empty_seal_replaces_an_earlier_one_waiting_behind_a_flush() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let write = ops.push(&mut lineage, 1, A, DEFAULT);
    lineage.completed(write, FULL);
    assert_eq!(submitted(lineage.seal(1)).flushes, [flush(1, A)]);
    assert!(
        quiet(&submitted(lineage.seal(3))),
        "waits for the seal through 1"
    );
    assert!(quiet(&submitted(lineage.seal(5))), "and so does this");
    assert_eq!(
        durable(&lineage.flushed(1, A, Ok(()))),
        [1, 5],
        "5 answers for 3, which it overwrote"
    );
    assert_eq!(lineage.state(3), State::Durable);
}

#[test]
fn an_overwritten_empty_seals_pair_ends_superseded() {
    let (mut lineage, sink) = recorded();
    let mut ops = Ops::new();
    let write = ops.push(&mut lineage, 1, A, DEFAULT);
    lineage.completed(write, FULL);
    submitted(lineage.seal(1));
    submitted(lineage.seal(3));
    assert_eq!(ended(&sink, "superseded"), 0);
    submitted(lineage.seal(5));
    assert_eq!(ended(&sink, "superseded"), 1, "3's pair, at once");
    submitted(lineage.seal(7));
    assert_eq!(ended(&sink, "superseded"), 2, "then 5's");
    lineage.flushed(1, A, Ok(()));
    assert_eq!(ended(&sink, "durable"), 2, "1's and 7's");
}

#[test]
fn an_empty_seal_waits_for_the_seal_before_it() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let write = ops.push(&mut lineage, 1, A, DEFAULT);
    lineage.completed(write, FULL);
    submitted(lineage.seal(1));
    submitted(lineage.seal(4));
    assert_eq!(lineage.state(4), State::Pending);
    assert_eq!(lineage.durable_through(), None);
    assert_eq!(durable(&lineage.flushed(1, A, Ok(()))), [1, 4]);
    assert_eq!(lineage.durable_through(), Some(4));
}

#[test]
fn an_empty_seal_with_nothing_before_it_is_durable_at_once_and_takes_no_slot() {
    let mut lineage = lineage();
    assert_eq!(durable(&submitted(lineage.seal(2))), [2]);
    assert_eq!(
        durable(&submitted(lineage.seal(4))),
        [4],
        "nothing was overwritten"
    );
}

#[test]
fn a_write_in_flight_in_the_span_makes_the_seal_a_record() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let write = ops.push(&mut lineage, 2, A, DEFAULT);
    assert!(quiet(&submitted(lineage.seal(3))), "it waits for the write");
    assert!(
        quiet(&submitted(lineage.seal(5))),
        "empty, so it trails the seal through 3"
    );
    assert_eq!(lineage.completed(write, FULL).flushes, [flush(3, A)]);
    assert_eq!(durable(&lineage.flushed(3, A, Ok(()))), [3, 5]);
}

#[test]
fn a_write_below_the_seal_point_does_not_make_a_later_seal_a_record() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let write = ops.push(&mut lineage, 2, A, DEFAULT);
    submitted(lineage.seal(3));
    submitted(lineage.seal(6));
    submitted(lineage.seal(9));
    lineage.completed(write, FULL);
    assert_eq!(
        durable(&lineage.flushed(3, A, Ok(()))),
        [3, 9],
        "the write belongs to the seal through 3, so 6 and 9 were both empty"
    );
}

#[test]
fn an_empty_seal_behind_a_failure_is_answered_blocked_and_so_is_its_replacement() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let write = ops.push(&mut lineage, 1, A, DEFAULT);
    lineage.completed(write, FULL);
    submitted(lineage.seal(1));
    let token = tokens(lineage.flushed(1, A, bad())).remove(0);
    assert_eq!(seen(&submitted(lineage.seal(3))), [Seen::Blocked(3, 0)]);
    assert_eq!(
        seen(&submitted(lineage.seal(5))),
        [Seen::Blocked(5, 0)],
        "the replacement is a seal of its own, and is answered"
    );
    let due = lineage
        .resolve(vec![(token, Resolution::Abandon)])
        .expect("abandon");
    assert_eq!(durable(&due), [1, 5], "not 3, which 5 replaced");
}

/// The delay the engineer accepted (DI-D-99, "I choose a"): while a failure holds an epoch between
/// two empty seals, the later one's replacing the earlier holds the mark below the earlier's
/// `through` until the failure is resolved. A record per empty seal would have made 5 durable when
/// the seal through 1 was.
#[test]
fn a_failure_between_two_empty_seals_holds_the_mark_below_the_earlier_until_resolved() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let first = ops.push_in(&mut lineage, 1, A, DEFAULT, &[1]);
    lineage.completed(first, FULL);
    submitted(lineage.seal(1));
    let sixth = ops.push_in(&mut lineage, 6, B, DEFAULT, &[2]);
    let (_, due) = lineage.import(
        FailureCause::Imported {
            scope: ImportScope::All,
        },
        &Reach::Domain(DomainId(2)),
    );
    let token = tokens(due).remove(0);
    lineage.completed(sixth, failed());
    assert!(
        quiet(&submitted(lineage.seal(5))),
        "empty: behind the seal through 1"
    );
    assert_eq!(
        seen(&submitted(lineage.seal(8))),
        [Seen::Blocked(8, 0)],
        "empty too, now that the write at 6 is gone: it replaces 5, and the failure holding 6 blocks it"
    );

    assert_eq!(durable(&lineage.flushed(1, A, Ok(()))), [1]);
    assert_eq!(lineage.durable_through(), Some(1));
    assert_eq!(
        lineage.state(5),
        State::Pending,
        "5 is held back by the failure holding 6"
    );

    let due = lineage
        .resolve(vec![(token, Resolution::Abandon)])
        .expect("abandon");
    assert_eq!(durable(&due), [8]);
}

#[test]
fn ending_a_lineage_ends_its_empty_seals_pairs() {
    let (mut lineage, sink) = recorded();
    let mut ops = Ops::new();
    let minted = lineage.mint(None);
    let write = ops.push_to(&mut lineage, minted, 1, A, DEFAULT, &[]);
    lineage.completed(write, FULL);
    lineage.0.seal(minted, 1);
    lineage.0.seal(minted, 3);
    assert_eq!(ended(&sink, "ended unanswered"), 0);
    lineage.end(minted);
    assert_eq!(
        ended(&sink, "ended unanswered"),
        2,
        "the recorded seal's and the one trailing it"
    );
}

#[test]
fn dropping_the_core_ends_an_empty_seals_pair() {
    let (mut lineage, sink) = recorded();
    let mut ops = Ops::new();
    let write = ops.push(&mut lineage, 1, A, DEFAULT);
    lineage.completed(write, FULL);
    submitted(lineage.seal(1));
    submitted(lineage.seal(2));
    drop(lineage);
    assert_eq!(ended(&sink, "ended unanswered"), 2);
}

#[test]
fn an_empty_seal_made_after_a_heal_releases_it_in_order() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let write = ops.push(&mut lineage, 1, A, DEFAULT);
    lineage.completed(write, FULL);
    submitted(lineage.seal(1));
    let token = tokens(lineage.flushed(1, A, bad())).remove(0);
    lineage
        .resolve(vec![(token, Resolution::Heal)])
        .expect("heal");
    let due = submitted(lineage.seal(2));
    assert!(
        seen(&due)
            .iter()
            .any(|seen| matches!(seen, Seen::Healed(..))),
        "an empty seal after the heal, reaching what was pushed, releases it"
    );
    assert_eq!(durable(&due), [1, 2]);
}

#[test]
fn two_files_two_flushes_and_an_empty_seal_after_them() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    for file in [A, B] {
        let write = ops.push(&mut lineage, 1, file, DEFAULT);
        lineage.completed(write, FULL);
    }
    submitted(lineage.seal(1));
    submitted(lineage.seal(2));
    assert!(quiet(&lineage.flushed(1, B, Ok(()))), "A has not answered");
    assert_eq!(durable(&lineage.flushed(1, A, Ok(()))), [1, 2]);
}

/// An empty seal takes a sequence number like any other (DI-D-99), so one made before a heal is
/// known to be before it, and does not release it when it finishes afterwards.
#[test]
fn an_empty_seal_made_before_a_heal_does_not_release_it() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let first = ops.push(&mut lineage, 1, A, DEFAULT);
    lineage.completed(first, FULL);
    submitted(lineage.seal(1));
    let token = tokens(lineage.flushed(1, A, bad())).remove(0);
    let second = ops.push(&mut lineage, 2, A, DEFAULT);
    submitted(lineage.seal(2));
    submitted(lineage.seal(3));
    lineage
        .resolve(vec![(token, Resolution::Heal)])
        .expect("heal");

    lineage.completed(second, FULL);
    lineage.flushed(2, A, Ok(()));
    assert_eq!(
        lineage.failures().len(),
        1,
        "the seals through 2 and 3 were both made before the heal"
    );
    let due = submitted(lineage.seal(4));
    assert!(
        seen(&due)
            .iter()
            .any(|seen| matches!(seen, Seen::Healed(..))),
        "4 is the first seal after it"
    );
}
