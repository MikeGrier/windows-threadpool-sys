// Copyright (c) 2026 Mike Grier
//! The window's places, held and released by their three holders, without a core around it.

use super::{Holder, Window};

/// A window of the places the tests here need.
fn window() -> Window {
    Window::new(16).expect("a table of 16")
}

#[test]
fn an_empty_window_holds_no_place() {
    let window = window();
    assert_eq!(window.places(), 0);
}

#[test]
fn the_first_hold_makes_the_place_and_the_last_release_ends_it() {
    let mut window = window();
    window.hold(7, Holder::Operation).expect("a new hold");
    assert_eq!(window.places(), 1);
    window.release(7, Holder::Operation).expect("held");
    assert_eq!(window.places(), 0);
}

#[test]
fn holders_of_one_operation_share_its_one_place() {
    let mut window = window();
    window.hold(7, Holder::Operation).expect("a new hold");
    window.hold(7, Holder::Uncovered).expect("a new hold");
    window.hold(7, Holder::Suspect).expect("a new hold");
    assert_eq!(window.places(), 1, "one operation, one place");
}

#[test]
fn a_place_ends_only_when_every_holder_has_let_go_in_any_order() {
    for order in [
        [Holder::Operation, Holder::Uncovered, Holder::Suspect],
        [Holder::Suspect, Holder::Operation, Holder::Uncovered],
        [Holder::Uncovered, Holder::Suspect, Holder::Operation],
    ] {
        let mut window = window();
        for holder in [Holder::Operation, Holder::Uncovered, Holder::Suspect] {
            window.hold(3, holder).expect("a new hold");
        }
        for (done, holder) in order.into_iter().enumerate() {
            assert_eq!(window.places(), 1, "{holder:?} has not yet let go ({done})");
            window.release(3, holder).expect("held");
        }
        assert_eq!(window.places(), 0, "{order:?}");
    }
}

#[test]
fn each_unresolved_failure_holds_the_place_once() {
    let mut window = window();
    window.hold(1, Holder::Suspect).expect("the first failure");
    window.hold(1, Holder::Suspect).expect("the second failure");
    window
        .release(1, Holder::Suspect)
        .expect("the first resolved");
    assert_eq!(window.places(), 1, "the second still holds it");
    window
        .release(1, Holder::Suspect)
        .expect("the second resolved");
    assert_eq!(window.places(), 0);
}

#[test]
fn operations_hold_places_of_their_own() {
    let mut window = window();
    for op in 0..10 {
        window.hold(op, Holder::Operation).expect("a new hold");
    }
    assert_eq!(window.places(), 10);
    for op in (0..10).step_by(2) {
        window.release(op, Holder::Operation).expect("held");
    }
    assert_eq!(window.places(), 5);
}

#[test]
fn an_operation_admitted_twice_is_inconsistent() {
    let mut window = window();
    window.hold(2, Holder::Operation).expect("a new hold");
    assert_eq!(
        window.hold(2, Holder::Operation),
        Err("an operation was admitted twice")
    );
    assert_eq!(window.places(), 1, "nothing was made by the refused hold");
}

#[test]
fn a_write_recorded_uncovered_twice_is_inconsistent() {
    let mut window = window();
    window.hold(2, Holder::Uncovered).expect("a new hold");
    assert_eq!(
        window.hold(2, Holder::Uncovered),
        Err("a write was recorded uncovered twice")
    );
}

#[test]
fn releasing_a_place_no_one_holds_is_inconsistent() {
    let mut window = window();
    assert_eq!(
        window.release(9, Holder::Operation),
        Err("a place was released that no one holds")
    );
}

#[test]
fn releasing_by_a_holder_that_did_not_hold_is_inconsistent_and_changes_nothing() {
    let mut window = window();
    window.hold(4, Holder::Operation).expect("a new hold");
    for holder in [Holder::Uncovered, Holder::Suspect] {
        assert_eq!(
            window.release(4, holder),
            Err("a place was released by a holder that did not hold it"),
            "{holder:?}"
        );
    }
    assert_eq!(window.places(), 1);
    window.release(4, Holder::Operation).expect("still held");
    assert_eq!(
        window.release(4, Holder::Operation),
        Err("a place was released that no one holds"),
        "a second release finds the place gone"
    );
}

#[test]
fn what_each_holder_holds_is_reported_with_the_failures_counted() {
    let mut window = window();
    window.hold(1, Holder::Operation).expect("a new hold");
    window.hold(1, Holder::Suspect).expect("a new hold");
    window.hold(1, Holder::Suspect).expect("a new hold");
    window.hold(2, Holder::Uncovered).expect("a new hold");
    assert_eq!(window.held_by(Holder::Operation), [(1, 1)].into());
    assert_eq!(window.held_by(Holder::Uncovered), [(2, 1)].into());
    assert_eq!(window.held_by(Holder::Suspect), [(1, 2)].into());
}
