// Copyright (c) 2026 Mike Grier
//! The activity map in memory, with ids made by a counter in place of the call.

use std::cell::Cell;

use durable_ioring::{CorrelationId, Diagnostic, DiagnosticKind, DiagnosticSink};
use win_etw_sys::Guid;

use super::Activities;

struct Wanting;

impl DiagnosticSink for Wanting {
    fn wants(&self, _: DiagnosticKind) -> bool {
        true
    }

    fn report(&self, _: &Diagnostic<'_>) {}
}

fn pair() -> CorrelationId {
    CorrelationId::for_pair(&Wanting, DiagnosticKind::Seal).expect("wanted")
}

/// Ids from a counter, and how many were made.
struct Maker(Cell<u128>);

impl Maker {
    fn new() -> Self {
        Self(Cell::new(0))
    }

    fn make(&self) -> Option<Guid> {
        self.0.set(self.0.get() + 1);
        Some(Guid::from_u128(self.0.get()))
    }

    fn made(&self) -> u128 {
        self.0.get()
    }
}

#[test]
fn a_begin_makes_an_activity_and_its_end_gives_the_same_one_back() {
    let (mut map, maker, p) = (Activities::new(), Maker::new(), pair());
    let began = map.begun(p, || maker.make());
    assert_eq!(began, Some(Guid::from_u128(1)));
    assert_eq!(map.ended(p), began);
}

#[test]
fn an_ended_pair_holds_nothing() {
    let (mut map, maker, p) = (Activities::new(), Maker::new(), pair());
    map.begun(p, || maker.make());
    map.ended(p);
    assert_eq!(map.len(), 0);
}

#[test]
fn an_end_whose_begin_was_not_written_has_no_activity() {
    let mut map = Activities::new();
    assert_eq!(map.ended(pair()), None);
    assert_eq!(map.len(), 0);
}

#[test]
fn two_pairs_have_two_activities() {
    let (mut map, maker) = (Activities::new(), Maker::new());
    let (a, b) = (pair(), pair());
    let first = map.begun(a, || maker.make());
    let second = map.begun(b, || maker.make());
    assert_ne!(first, second);
    assert_eq!(map.ended(b), second);
    assert_eq!(map.ended(a), first);
}

#[test]
fn pairs_end_in_any_order() {
    let (mut map, maker) = (Activities::new(), Maker::new());
    let pairs: Vec<_> = (0..5).map(|_| pair()).collect();
    let ids: Vec<_> = pairs
        .iter()
        .map(|&p| map.begun(p, || maker.make()))
        .collect();
    for i in [3, 0, 4, 1, 2] {
        assert_eq!(map.ended(pairs[i]), ids[i]);
    }
    assert_eq!(map.len(), 0);
}

#[test]
fn a_begin_written_twice_shares_one_activity_and_holds_it_until_both_ends() {
    let (mut map, maker, p) = (Activities::new(), Maker::new(), pair());
    let first = map.begun(p, || maker.make());
    let second = map.begun(p, || maker.make());
    assert_eq!(first, second);
    assert_eq!(maker.made(), 1, "the second begin makes none");
    assert_eq!(map.ended(p), first);
    assert_eq!(map.len(), 1, "held for the second end");
    assert_eq!(map.ended(p), first);
    assert_eq!(map.len(), 0);
}

#[test]
fn an_end_beyond_its_begins_has_no_activity() {
    let (mut map, maker, p) = (Activities::new(), Maker::new(), pair());
    let began = map.begun(p, || maker.make());
    assert_eq!(map.ended(p), began);
    assert_eq!(map.ended(p), None);
}

#[test]
fn a_failed_creation_leaves_the_pair_with_none_and_holds_nothing() {
    let (mut map, p) = (Activities::new(), pair());
    assert_eq!(map.begun(p, || None), None);
    assert_eq!(map.len(), 0);
    assert_eq!(map.ended(p), None);
}

#[test]
fn a_pair_begun_again_after_it_ended_has_a_new_activity() {
    let (mut map, maker, p) = (Activities::new(), Maker::new(), pair());
    let first = map.begun(p, || maker.make());
    map.ended(p);
    let second = map.begun(p, || maker.make());
    assert_ne!(first, second);
}

#[test]
fn the_process_map_is_left_holding_nothing_by_a_pair_that_ended() {
    let p = pair();
    let id = super::begun(p, || Some(Guid::from_u128(0xfeed)));
    assert_eq!(id, Some(Guid::from_u128(0xfeed)));
    assert_eq!(super::ended(p), id);
    assert_eq!(super::ended(p), None, "let go");
}
