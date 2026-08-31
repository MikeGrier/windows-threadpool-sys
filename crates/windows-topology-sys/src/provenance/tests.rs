// Copyright (c) 2026 Mike Grier
//! Tests for [`Provenance`](super::Provenance).

use super::Provenance;

#[test]
fn the_default_is_the_untrusted_value() {
    // The load-bearing property. If this ever flips, every construction that
    // forgets to name a provenance silently starts asserting it measured the
    // machine, which is the exact failure this type exists to prevent.
    assert_eq!(Provenance::default(), Provenance::Synthetic);
    assert!(!Provenance::default().is_measured());
}

#[test]
fn the_ordering_is_the_trust_order() {
    assert!(Provenance::Synthetic < Provenance::Restored);
    assert!(Provenance::Restored < Provenance::Measured);
}

#[test]
fn only_measured_reports_as_measured() {
    assert!(Provenance::Measured.is_measured());
    assert!(!Provenance::Restored.is_measured());
    assert!(!Provenance::Synthetic.is_measured());
}

#[test]
fn downgrading_lowers_a_higher_claim() {
    assert_eq!(
        Provenance::Measured.downgraded_to(Provenance::Restored),
        Provenance::Restored
    );
}

#[test]
fn downgrading_leaves_an_equal_or_lower_claim_alone() {
    // The ceiling is a maximum, not an assignment: a synthetic description must
    // not be promoted to restored just because it passed through a loader.
    assert_eq!(
        Provenance::Restored.downgraded_to(Provenance::Restored),
        Provenance::Restored
    );
    assert_eq!(
        Provenance::Synthetic.downgraded_to(Provenance::Restored),
        Provenance::Synthetic
    );
}

#[test]
fn downgrading_never_raises_for_any_pair() {
    // Exhaustive over the whole type, so a variant added later cannot quietly
    // acquire an upgrade path.
    let all = [
        Provenance::Synthetic,
        Provenance::Restored,
        Provenance::Measured,
    ];
    for value in all {
        for ceiling in all {
            let result = value.downgraded_to(ceiling);
            assert!(
                result <= value,
                "{value:?} downgraded to {ceiling:?} produced the higher {result:?}"
            );
            assert!(
                result <= ceiling,
                "{value:?} downgraded to {ceiling:?} exceeded the ceiling"
            );
        }
    }
}

#[test]
fn the_untrusted_labels_are_louder_than_the_trusted_one() {
    // Deliberate asymmetry: a tainted value must stand out in any string it
    // reaches, and a reader scanning output should not have to know the
    // vocabulary to notice something is off.
    assert_eq!(Provenance::Synthetic.to_string(), "SYNTHETIC");
    assert_eq!(Provenance::Restored.to_string(), "RESTORED");
    assert_eq!(Provenance::Measured.to_string(), "measured");

    for tainted in [Provenance::Synthetic, Provenance::Restored] {
        let rendered = tainted.to_string();
        assert_eq!(
            rendered,
            rendered.to_uppercase(),
            "{tainted:?} does not render in capitals"
        );
    }
    assert_ne!(
        Provenance::Measured.to_string(),
        Provenance::Measured.to_string().to_uppercase()
    );
}
