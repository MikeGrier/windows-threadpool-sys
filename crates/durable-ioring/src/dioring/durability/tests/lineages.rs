// Copyright (c) 2026 Mike Grier
//! Minted lineages (DI-D-19, DI-D-30, DI-D-40, DI-D-41), without a ring: each lineage's seals and
//! mark are its own; ending one abandons what it had not made durable and absorbs what it had in
//! flight; retiring one is refused while anything holds it; and a failure spanning lineages holds
//! each, and heals in each separately.

use super::{
    A, B, DEFAULT, FAILED, FULL, IO_DEVICE, Lineage, Ops, Seen, flush, flush_of, lineage, quiet,
    seen, submitted, tokens,
};
use crate::dioring::durability::{DEFAULT_LINEAGE, Key, Reach, State};
use crate::ids::FailureId;
use crate::types::{Cause, ImportScope, LineageBusy, MarkingKind, Resolution};

fn import(lineage: &mut Lineage, only: Option<Key>) -> super::Due<u64, &'static str> {
    let cause = Cause::Imported {
        scope: ImportScope::All,
    };
    lineage.0.import(cause, &Reach::All, only).1
}

/// A core with one lineage minted beside its default.
fn two() -> (Lineage, Key) {
    let mut lineage = lineage();
    let minted = lineage.mint(Some("log".to_owned()));
    (lineage, minted)
}

/// Whether the core still records `key`, live or draining.
fn recorded(lineage: &Lineage, key: Key) -> bool {
    lineage.0.lineages.contains_key(&key)
}

/// A refusal's busy report, by its counts and its failures' sequence numbers.
fn busy(refusal: LineageBusy<crate::ids::DioringIds<u64>>) -> (usize, usize, usize, Vec<u64>) {
    (
        refusal.in_flight,
        refusal.held_for_gate,
        refusal.uncovered,
        refusal.failures.iter().map(|f| f.seq).collect(),
    )
}

#[test]
fn a_minted_lineage_is_listed_with_its_description_beside_the_default() {
    let (lineage, minted) = two();
    assert_ne!(minted, DEFAULT_LINEAGE);
    let listed = lineage.lineages();
    assert_eq!(listed.len(), 2);
    assert!(listed[0].is_default && listed[0].description.is_none());
    assert_eq!(listed[1].lineage.seq, minted);
    assert!(!listed[1].is_default);
    assert_eq!(listed[1].description.as_deref(), Some("log"));
    assert_eq!(listed[0].lineage.instance, listed[1].lineage.instance);
    assert!(lineage.is_live(minted));
    assert!(!lineage.is_live(minted + 1), "never minted");
}

#[test]
fn each_lineage_has_its_own_seal_point_and_mark() {
    let (mut lineage, minted) = two();
    assert_eq!(seen(&submitted(lineage.seal(3))), [Seen::Durable(3)]);
    assert_eq!(lineage.0.sealed_through(minted), Some(None));
    assert_eq!(lineage.0.state(minted, 2), Some(State::Open));
    assert_eq!(
        lineage.0.refuses(minted, 2),
        None,
        "3 is the default's seal"
    );

    assert_eq!(
        seen(&submitted(lineage.0.seal(minted, 1))),
        [Seen::DurableIn(minted, 1)]
    );
    assert_eq!(lineage.0.durable_through(minted), Some(Some(1)));
    assert_eq!(
        lineage.durable_through(),
        Some(3),
        "the default is untouched"
    );
    assert_eq!(lineage.0.refuses(minted, 1), Some(1));
    assert_eq!(lineage.0.state(minted, 2), Some(State::Open));
}

#[test]
fn a_write_in_flight_holds_only_its_own_lineages_seal() {
    let (mut lineage, minted) = two();
    let mut ops = Ops::new();
    let write = ops.push_to(&mut lineage, minted, 1, A, DEFAULT, &[]);
    assert!(quiet(&submitted(lineage.0.seal(minted, 2))), "in flight");
    assert_eq!(
        seen(&submitted(lineage.seal(2))),
        [Seen::Durable(2)],
        "the default has no write of its own"
    );
    let due = lineage.completed(write, FULL);
    assert_eq!(due.flushes, [flush_of(minted, 2, A)]);
    assert_eq!(
        seen(&lineage.0.flushed(minted, 2, A, Ok(()))),
        [Seen::DurableIn(minted, 2)]
    );
}

#[test]
fn each_lineages_seal_flushes_its_own_writes_of_a_shared_file() {
    let (mut lineage, minted) = two();
    let mut ops = Ops::new();
    let own = ops.push(&mut lineage, 1, A, DEFAULT);
    let theirs = ops.push_to(&mut lineage, minted, 1, A, DEFAULT, &[]);
    lineage.completed(own, FULL);
    lineage.completed(theirs, FULL);
    assert_eq!(submitted(lineage.seal(1)).flushes, [flush(1, A)]);
    assert_eq!(
        submitted(lineage.0.seal(minted, 1)).flushes,
        [flush_of(minted, 1, A)]
    );
    assert_eq!(
        seen(&lineage.0.flushed(minted, 1, A, Ok(()))),
        [Seen::DurableIn(minted, 1)],
        "the minted lineage's flush answers its seal alone"
    );
    assert_eq!(lineage.state(1), State::Pending);
    assert_eq!(seen(&lineage.flushed(1, A, Ok(()))), [Seen::Durable(1)]);
}

#[test]
fn ending_abandons_everything_not_yet_durable_and_absorbs_its_flushes() {
    let (mut lineage, minted) = two();
    let mut ops = Ops::new();
    let sealed = ops.push_to(&mut lineage, minted, 2, A, DEFAULT, &[]);
    let open = ops.push_to(&mut lineage, minted, 5, B, DEFAULT, &[]);
    lineage.completed(sealed, FULL);
    lineage.completed(open, FULL);
    assert_eq!(
        submitted(lineage.0.seal(minted, 2)).flushes,
        [flush_of(minted, 2, A)]
    );

    assert_eq!(
        seen(&lineage.end(minted)),
        [Seen::LineageEnded(minted, Some(5))],
        "through the highest epoch written, above the seal"
    );
    assert!(!lineage.is_live(minted));
    assert!(!recorded(&lineage, minted), "nothing in flight: retired");
    assert_eq!(lineage.0.sealed_through(minted), None);
    assert_eq!(lineage.0.durable_through(minted), None);
    assert_eq!(lineage.0.state(minted, 2), None);
    assert_eq!(lineage.lineages().len(), 1);

    assert!(
        quiet(&lineage.0.flushed(minted, 2, A, Ok(()))),
        "the seal's flush answers nothing"
    );
    assert_eq!(lineage.inconsistency(), None);
}

#[test]
fn a_flush_answering_while_an_ended_lineage_drains_reports_nothing() {
    let (mut lineage, minted) = two();
    let mut ops = Ops::new();
    let sealed = ops.push_to(&mut lineage, minted, 2, A, DEFAULT, &[]);
    let in_flight = ops.push_to(&mut lineage, minted, 5, B, DEFAULT, &[]);
    lineage.completed(sealed, FULL);
    assert_eq!(
        submitted(lineage.0.seal(minted, 2)).flushes,
        [flush_of(minted, 2, A)]
    );
    assert_eq!(
        seen(&lineage.end(minted)),
        [Seen::LineageEnded(minted, Some(5))]
    );
    assert!(recorded(&lineage, minted), "draining its write in flight");
    assert!(quiet(&lineage.0.flushed(minted, 2, A, Ok(()))));
    assert!(
        quiet(&lineage.0.flushed(minted, 2, B, Ok(()))),
        "nor does one the seal never issued"
    );
    assert_eq!(lineage.inconsistency(), None);
    assert!(quiet(&lineage.completed(in_flight, FULL)));
    assert!(!recorded(&lineage, minted));
}

#[test]
fn ending_a_lineage_with_nothing_pending_abandons_nothing() {
    let (mut lineage, minted) = two();
    assert_eq!(
        seen(&submitted(lineage.0.seal(minted, 4))),
        [Seen::DurableIn(minted, 4)]
    );
    assert_eq!(
        seen(&lineage.end(minted)),
        [Seen::LineageEnded(minted, None)]
    );
}

#[test]
fn a_write_in_flight_at_the_end_is_let_go_and_the_lineage_retires_with_it() {
    let (mut lineage, minted) = two();
    let mut ops = Ops::new();
    let write = ops.push_to(&mut lineage, minted, 1, A, DEFAULT, &[]);
    assert_eq!(
        seen(&lineage.end(minted)),
        [Seen::LineageEnded(minted, Some(1))]
    );
    assert!(recorded(&lineage, minted), "draining");
    assert!(!lineage.is_live(minted), "and not live while it drains");
    assert!(quiet(&lineage.completed(write, FULL)));
    assert!(!recorded(&lineage, minted), "retired with its last write");
    assert_eq!(lineage.inconsistency(), None);
}

#[test]
fn a_failure_observed_before_the_end_is_still_marked_by_its_write_in_flight() {
    let (mut lineage, minted) = two();
    let mut ops = Ops::new();
    let write = ops.push_to(&mut lineage, minted, 1, A, DEFAULT, &[]);
    assert_eq!(
        seen(&import(&mut lineage, None)),
        [Seen::Failed(0, vec![write.seq])]
    );
    lineage.end(minted);
    let nullified = MarkingKind::Nullified {
        code: Some(IO_DEVICE),
    };
    assert_eq!(
        seen(&lineage.completed(write, FAILED)),
        [Seen::Marked(0, write.seq, nullified)]
    );
}

#[test]
fn a_failure_observed_after_the_end_suspects_no_write_of_the_ended_lineage() {
    let (mut lineage, minted) = two();
    let mut ops = Ops::new();
    let gone = ops.push_to(&mut lineage, minted, 1, A, DEFAULT, &[]);
    let kept = ops.push(&mut lineage, 1, A, DEFAULT);
    lineage.end(minted);
    assert_eq!(
        seen(&import(&mut lineage, None)),
        [Seen::Failed(0, vec![kept.seq])],
        "{gone:?} is in flight, and of a lineage that ended"
    );
}

#[test]
fn an_import_confined_to_one_lineage_suspects_only_its_writes() {
    let (mut lineage, minted) = two();
    let mut ops = Ops::new();
    let own = ops.push(&mut lineage, 1, A, DEFAULT);
    let theirs = ops.push_to(&mut lineage, minted, 1, A, DEFAULT, &[]);
    assert_eq!(
        seen(&import(&mut lineage, Some(minted))),
        [Seen::Failed(0, vec![theirs.seq])]
    );
    assert_eq!(
        seen(&import(&mut lineage, Some(DEFAULT_LINEAGE))),
        [Seen::Failed(1, vec![own.seq])]
    );
    assert_eq!(
        seen(&import(&mut lineage, None)),
        [Seen::Failed(2, vec![own.seq, theirs.seq])]
    );
}

#[test]
fn retiring_is_refused_while_anything_holds_the_lineage() {
    let (mut lineage, minted) = two();
    let mut ops = Ops::new();
    let write = ops.push_to(&mut lineage, minted, 1, A, DEFAULT, &[]);
    assert_eq!(
        busy(lineage.retire(minted).expect_err("in flight")),
        (1, 0, 0, vec![])
    );
    lineage.completed(write, FULL);
    let token = tokens(import(&mut lineage, None)).remove(0);
    assert_eq!(
        busy(lineage.retire(minted).expect_err("uncovered, and held")),
        (0, 0, 1, vec![0])
    );
    assert!(lineage.is_live(minted), "a refusal changes nothing");

    submitted(lineage.0.seal(minted, 1));
    assert_eq!(
        seen(&lineage.0.flushed(minted, 1, A, Ok(()))),
        [
            Seen::Marked(0, write.seq, MarkingKind::Covered),
            Seen::BlockedIn(minted, 1, 0)
        ]
    );
    assert_eq!(
        busy(lineage.retire(minted).expect_err("held")),
        (0, 0, 0, vec![0])
    );
    lineage
        .resolve(vec![(token, Resolution::Abandon)])
        .expect("abandon");
    lineage.retire(minted).expect("nothing holds it");
    assert!(!lineage.is_live(minted));
    assert!(!recorded(&lineage, minted));
    assert_eq!(lineage.lineages().len(), 1);
    assert_eq!(lineage.inconsistency(), None);
}

#[test]
fn a_lineage_key_is_never_reused() {
    let (mut lineage, minted) = two();
    lineage.retire(minted).expect("empty");
    let next = lineage.mint(None);
    assert!(next > minted);
    let ended = lineage.mint(None);
    lineage.end(ended);
    assert!(lineage.mint(None) > ended);
}

#[test]
fn a_failure_spanning_two_lineages_holds_each_and_heals_in_each_separately() {
    let (mut lineage, minted) = two();
    let mut ops = Ops::new();
    let own = ops.push(&mut lineage, 1, A, DEFAULT);
    let theirs = ops.push_to(&mut lineage, minted, 1, B, DEFAULT, &[]);
    lineage.completed(own, FULL);
    lineage.completed(theirs, FULL);
    let token = tokens(import(&mut lineage, None)).remove(0);
    lineage
        .resolve(vec![(token, Resolution::Heal)])
        .expect("heal");

    assert_eq!(submitted(lineage.seal(1)).flushes, [flush(1, A)]);
    assert_eq!(
        seen(&lineage.flushed(1, A, Ok(()))),
        [
            Seen::Marked(0, own.seq, MarkingKind::Covered),
            Seen::Durable(1)
        ],
        "the heal took effect in the default lineage, and the failure still holds the other"
    );
    assert_eq!(lineage.failures().len(), 1);
    assert_eq!(lineage.0.state(minted, 1), Some(State::Open));

    assert_eq!(
        submitted(lineage.0.seal(minted, 1)).flushes,
        [flush_of(minted, 1, B)]
    );
    assert_eq!(
        seen(&lineage.0.flushed(minted, 1, B, Ok(()))),
        [
            Seen::Marked(0, theirs.seq, MarkingKind::Covered),
            Seen::Healed(
                0,
                vec![own.seq, theirs.seq],
                vec![
                    (own.seq, MarkingKind::Covered),
                    (theirs.seq, MarkingKind::Covered)
                ]
            ),
            Seen::DurableIn(minted, 1),
        ]
    );
    assert!(lineage.failures().is_empty());
}

#[test]
fn a_failure_spanning_two_lineages_is_blocked_in_each_until_its_heal_reaches_it() {
    let (mut lineage, minted) = two();
    let mut ops = Ops::new();
    let own = ops.push(&mut lineage, 1, A, DEFAULT);
    let theirs = ops.push_to(&mut lineage, minted, 1, B, DEFAULT, &[]);
    lineage.completed(own, FULL);
    lineage.completed(theirs, FULL);
    import(&mut lineage, None);
    submitted(lineage.seal(1));
    submitted(lineage.0.seal(minted, 1));
    assert_eq!(
        seen(&lineage.flushed(1, A, Ok(()))),
        [
            Seen::Marked(0, own.seq, MarkingKind::Covered),
            Seen::Blocked(1, 0)
        ]
    );
    assert_eq!(
        seen(&lineage.0.flushed(minted, 1, B, Ok(()))),
        [
            Seen::Marked(0, theirs.seq, MarkingKind::Covered),
            Seen::BlockedIn(minted, 1, 0)
        ]
    );
    assert_eq!(lineage.state(1), State::Blocked(own_failure(&lineage)));
    assert_eq!(
        lineage.0.state(minted, 1),
        Some(State::Blocked(own_failure(&lineage)))
    );
}

/// The one unresolved failure's identity.
fn own_failure(lineage: &Lineage) -> FailureId {
    lineage.failures()[0].id
}

#[test]
fn ending_a_lineage_a_heal_waits_for_lets_the_heal_take_effect() {
    let (mut lineage, minted) = two();
    let mut ops = Ops::new();
    let own = ops.push(&mut lineage, 1, A, DEFAULT);
    let theirs = ops.push_to(&mut lineage, minted, 1, B, DEFAULT, &[]);
    lineage.completed(own, FULL);
    lineage.completed(theirs, FULL);
    let token = tokens(import(&mut lineage, None)).remove(0);
    lineage
        .resolve(vec![(token, Resolution::Heal)])
        .expect("heal");
    submitted(lineage.seal(1));
    lineage.flushed(1, A, Ok(()));
    assert_eq!(lineage.failures().len(), 1, "waiting for the other lineage");

    assert_eq!(
        seen(&lineage.end(minted)),
        [
            Seen::LineageEnded(minted, Some(1)),
            Seen::Healed(
                0,
                vec![own.seq, theirs.seq],
                vec![(own.seq, MarkingKind::Covered)]
            ),
        ]
    );
    assert!(lineage.failures().is_empty());
}

#[test]
fn a_failure_suspecting_nothing_holds_no_lineage_and_heals_at_once() {
    let mut lineage = lineage();
    let token = tokens(import(&mut lineage, None)).remove(0);
    assert_eq!(
        seen(
            &lineage
                .resolve(vec![(token, Resolution::Heal)])
                .expect("heal")
        ),
        [Seen::Healed(0, vec![], vec![])]
    );
    assert!(lineage.failures().is_empty());
}

#[test]
fn abandoning_a_failure_spanning_two_lineages_abandons_its_epochs_in_each() {
    let (mut lineage, minted) = two();
    let mut ops = Ops::new();
    ops.push(&mut lineage, 2, A, DEFAULT);
    ops.push_to(&mut lineage, minted, 3, B, DEFAULT, &[]);
    let token = tokens(import(&mut lineage, None)).remove(0);
    lineage
        .resolve(vec![(token, Resolution::Abandon)])
        .expect("abandon");
    assert!(lineage.is_abandoned(2));
    assert!(!lineage.is_abandoned(3), "3 is the other lineage's");
    assert!(lineage.0.is_abandoned(minted, 3));
    assert!(!lineage.0.is_abandoned(minted, 2));
    assert_eq!(lineage.0.state(minted, 3), Some(State::Abandoned));
}

#[test]
fn a_change_to_a_lineage_that_is_not_live_is_recorded_as_an_inconsistency() {
    let (mut lineage, minted) = two();
    lineage.end(minted);
    let mut ops = Ops::new();
    ops.push_to(&mut lineage, minted, 1, A, DEFAULT, &[]);
    assert_eq!(
        lineage.inconsistency(),
        Some("a write was pushed to a lineage that is not live")
    );

    let (mut lineage, _) = two();
    assert!(quiet(&lineage.end(DEFAULT_LINEAGE)));
    assert_eq!(
        lineage.inconsistency(),
        Some("an end of a lineage that is not live, or of the default")
    );
    assert!(lineage.is_live(DEFAULT_LINEAGE), "the default does not end");

    let (mut lineage, _) = two();
    assert!(lineage.retire(DEFAULT_LINEAGE).is_ok());
    assert_eq!(
        lineage.inconsistency(),
        Some("a retire of a lineage that is not live, or of the default")
    );
    assert!(lineage.is_live(DEFAULT_LINEAGE));

    let (mut lineage, minted) = two();
    lineage.end(minted);
    assert!(quiet(&submitted(lineage.0.seal(minted, 1))));
    assert_eq!(
        lineage.inconsistency(),
        Some("a seal of a lineage that is not live")
    );
}
