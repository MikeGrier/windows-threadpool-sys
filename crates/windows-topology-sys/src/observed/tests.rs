// Copyright (c) 2026 Mike Grier
//! Tests for [`Observed`].

use super::Observed;

#[test]
fn the_two_absences_are_not_equal_to_each_other() {
    // The whole point of the type. If these compared equal, it would spell
    // three facts with two values and be no better than `Option`.
    assert_ne!(Observed::<u32>::Absent, Observed::<u32>::NotObserved);
}

#[test]
fn known_discards_the_reason_which_is_why_it_is_the_narrow_accessor() {
    assert_eq!(Observed::Known(7_u32).known(), Some(7));
    assert_eq!(Observed::<u32>::Absent.known(), None);
    assert_eq!(Observed::<u32>::NotObserved.known(), None);
}

#[test]
fn was_observed_separates_an_answer_from_a_gap() {
    // Both `Known` and `Absent` are answers; only `NotObserved` is a gap. A
    // caller uses this to decide whether re-deriving could help -- it cannot,
    // if the platform already said there is none.
    assert!(Observed::Known(0_u32).was_observed());
    assert!(Observed::<u32>::Absent.was_observed());
    assert!(!Observed::<u32>::NotObserved.was_observed());
}

#[test]
fn a_known_zero_is_an_answer_not_an_absence() {
    // The sentinel collision D-11 and D-13 exist to prevent: `0` is a real
    // value and must not read as "missing".
    let zero = Observed::Known(0_u32);
    assert!(zero.was_observed());
    assert_eq!(zero.known(), Some(0));
    assert_ne!(zero, Observed::Absent);
}

#[test]
fn map_preserves_which_absence_it_was() {
    assert_eq!(Observed::Known(2_u32).map(|v| v * 2), Observed::Known(4));
    assert_eq!(
        Observed::<u32>::Absent.map(|v| v * 2),
        Observed::<u32>::Absent
    );
    assert_eq!(
        Observed::<u32>::NotObserved.map(|v| v * 2),
        Observed::<u32>::NotObserved
    );
}

#[test]
fn the_default_claims_nothing() {
    // Same reasoning as Provenance defaulting to Synthetic (D-12): forgetting
    // to set a field must not assert something about the machine.
    assert_eq!(Observed::<u32>::default(), Observed::NotObserved);
    assert!(!Observed::<u32>::default().was_observed());
}

#[test]
fn it_holds_values_that_are_not_copy() {
    let held: Observed<String> = Observed::Known("l3".to_string());
    assert_eq!(held.clone().known().as_deref(), Some("l3"));
    assert_eq!(held.map(|s| s.len()), Observed::Known(2));
}

#[cfg(feature = "serde")]
#[test]
fn the_two_absences_survive_a_round_trip_distinctly() {
    // A wire format that collapsed them would undo the type at the boundary,
    // which is exactly where a hand-written description enters this crate.
    for value in [
        Observed::Known(3_u32),
        Observed::Absent,
        Observed::NotObserved,
    ] {
        let json = serde_json::to_string(&value).expect("serialize");
        let back: Observed<u32> = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, value, "round trip changed {json}");
    }

    assert_ne!(
        serde_json::to_string(&Observed::<u32>::Absent).expect("serialize"),
        serde_json::to_string(&Observed::<u32>::NotObserved).expect("serialize"),
        "the two absences must not share a wire representation"
    );
}
