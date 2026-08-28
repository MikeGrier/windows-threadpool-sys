// Copyright (c) Mike Grier.

//! Tests for the three-state captured value.

use super::Captured;

#[test]
fn not_captured_and_absent_are_distinguishable() {
    // The whole reason this is not an `Option`: both yield no value, and only
    // one of them is a decision.
    let omitted: Captured<u32> = Captured::NotCaptured;
    let asked_and_empty: Captured<u32> = Captured::Absent;

    assert_eq!(omitted.present(), None);
    assert_eq!(asked_and_empty.present(), None);
    assert_ne!(omitted, asked_and_empty);
    assert!(!omitted.was_captured());
    assert!(asked_and_empty.was_captured());
}

#[test]
fn present_reports_captured_and_yields_its_value() {
    let captured = Captured::Present(7u32);
    assert!(captured.was_captured());
    assert_eq!(captured.present(), Some(&7));
}

#[test]
fn as_ref_preserves_the_state() {
    assert_eq!(Captured::<u32>::NotCaptured.as_ref(), Captured::NotCaptured);
    assert_eq!(Captured::<u32>::Absent.as_ref(), Captured::Absent);
    assert_eq!(Captured::Present(3u32).as_ref(), Captured::Present(&3));
}

#[test]
fn map_transforms_only_the_present_case() {
    assert_eq!(
        Captured::<u32>::NotCaptured.map(|value| value + 1),
        Captured::NotCaptured
    );
    assert_eq!(
        Captured::<u32>::Absent.map(|value| value + 1),
        Captured::Absent
    );
    assert_eq!(
        Captured::Present(1u32).map(|value| value + 1),
        Captured::Present(2)
    );
}

#[test]
fn map_does_not_run_its_function_when_there_is_nothing_to_map() {
    let mut ran = false;
    let _ = Captured::<u32>::Absent.map(|value| {
        ran = true;
        value
    });
    assert!(!ran, "map ran its function on an absent value");
}
