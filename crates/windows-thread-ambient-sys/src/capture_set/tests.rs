// Copyright (c) Mike Grier.

//! Tests for the capture set.

use super::{CapturableAspect, CaptureSet};

#[test]
fn none_is_empty_and_contains_nothing() {
    assert!(CaptureSet::NONE.is_empty());
    assert_eq!(CaptureSet::NONE.aspects().count(), 0);
    for aspect in CapturableAspect::EVERY {
        assert!(!CaptureSet::NONE.contains(aspect.as_set()));
    }
}

#[test]
fn every_singleton_contains_exactly_itself() {
    for aspect in CapturableAspect::EVERY {
        let set = aspect.as_set();
        assert!(set.contains(set));
        assert_eq!(set.aspects().collect::<Vec<_>>(), vec![*aspect]);
    }
}

#[test]
fn all_contains_every_listed_aspect() {
    for aspect in CapturableAspect::EVERY {
        assert!(
            CaptureSet::ALL.contains(aspect.as_set()),
            "{aspect} is missing from ALL"
        );
    }
    assert_eq!(
        CaptureSet::ALL.aspects().count(),
        CapturableAspect::EVERY.len()
    );
}

#[test]
fn every_variant_is_listed_in_every() {
    // The exhaustive match is the point: adding a variant to `CapturableAspect`
    // without adding it to `EVERY` stops this test compiling, and `ALL` is
    // derived from `EVERY`, so the new aspect cannot be silently absent from it.
    for aspect in [
        CapturableAspect::Impersonation,
        CapturableAspect::ErrorMode,
        CapturableAspect::Transaction,
    ] {
        let listed = CapturableAspect::EVERY.contains(&aspect);
        match aspect {
            CapturableAspect::Impersonation => assert!(listed, "impersonation is unlisted"),
            CapturableAspect::ErrorMode => assert!(listed, "error mode is unlisted"),
            CapturableAspect::Transaction => assert!(listed, "transaction is unlisted"),
        }
    }
}

#[test]
fn the_default_is_impersonation_and_error_mode() {
    // Pinned deliberately: growing this set is a breaking change, so a change
    // here should fail a test rather than pass unnoticed.
    assert!(CaptureSet::DEFAULT.contains(CaptureSet::IMPERSONATION));
    assert!(CaptureSet::DEFAULT.contains(CaptureSet::ERROR_MODE));
    assert_eq!(CaptureSet::DEFAULT.aspects().count(), 2);
}

#[test]
fn the_default_excludes_the_transaction_aspect() {
    // TxF is deprecated, costs a lazy ntdll binding, and enlists remoted work in
    // a transaction the caller may commit or roll back underneath it. That is a
    // hazard to opt into, never one to acquire by taking a default.
    assert!(!CaptureSet::DEFAULT.contains(CaptureSet::TRANSACTION));
}

#[test]
fn the_default_is_a_subset_of_all() {
    assert!(CaptureSet::ALL.contains(CaptureSet::DEFAULT));
}

#[test]
fn union_accumulates_and_is_idempotent() {
    let one = CaptureSet::IMPERSONATION;
    assert_eq!(one.union(one), one);

    let two = one.union(CaptureSet::TRANSACTION);
    assert!(two.contains(CaptureSet::IMPERSONATION));
    assert!(two.contains(CaptureSet::TRANSACTION));
    assert!(!two.contains(CaptureSet::ERROR_MODE));
}

#[test]
fn union_is_commutative() {
    let left = CaptureSet::IMPERSONATION.union(CaptureSet::ERROR_MODE);
    let right = CaptureSet::ERROR_MODE.union(CaptureSet::IMPERSONATION);
    assert_eq!(left, right);
}

#[test]
fn without_removes_only_what_was_named() {
    let reduced = CaptureSet::ALL.without(CaptureSet::TRANSACTION);
    assert!(!reduced.contains(CaptureSet::TRANSACTION));
    assert!(reduced.contains(CaptureSet::IMPERSONATION));
    assert!(reduced.contains(CaptureSet::ERROR_MODE));
}

#[test]
fn without_an_absent_aspect_changes_nothing() {
    let set = CaptureSet::IMPERSONATION;
    assert_eq!(set.without(CaptureSet::TRANSACTION), set);
}

#[test]
fn removing_everything_yields_the_empty_set() {
    assert_eq!(CaptureSet::ALL.without(CaptureSet::ALL), CaptureSet::NONE);
    assert!(CaptureSet::ALL.without(CaptureSet::ALL).is_empty());
}

#[test]
fn every_set_contains_the_empty_set() {
    for set in [
        CaptureSet::NONE,
        CaptureSet::IMPERSONATION,
        CaptureSet::DEFAULT,
        CaptureSet::ALL,
    ] {
        assert!(set.contains(CaptureSet::NONE));
    }
}

#[test]
fn contains_requires_every_named_aspect_not_merely_one() {
    let single = CaptureSet::IMPERSONATION;
    let pair = CaptureSet::IMPERSONATION.union(CaptureSet::ERROR_MODE);
    assert!(pair.contains(single));
    assert!(
        !single.contains(pair),
        "containment must not succeed on a partial overlap"
    );
}

#[test]
fn aspects_are_yielded_in_a_stable_order() {
    let forwards = CaptureSet::ALL.aspects().collect::<Vec<_>>();
    let again = CaptureSet::ALL.aspects().collect::<Vec<_>>();
    assert_eq!(forwards, again);
    assert_eq!(forwards, CapturableAspect::EVERY.to_vec());
}

#[test]
fn a_set_built_in_either_order_yields_the_same_sequence() {
    let left = CaptureSet::TRANSACTION.union(CaptureSet::IMPERSONATION);
    let right = CaptureSet::IMPERSONATION.union(CaptureSet::TRANSACTION);
    assert_eq!(
        left.aspects().collect::<Vec<_>>(),
        right.aspects().collect::<Vec<_>>(),
        "iteration order should follow EVERY, not construction order"
    );
}

#[test]
fn debug_names_the_aspects_rather_than_a_bit_pattern() {
    // A caller inspecting a set in a log should see what it means, since the
    // whole reason the default is named is so its contents can be read.
    let rendered = format!("{:?}", CaptureSet::DEFAULT);
    assert!(rendered.contains("impersonation"), "got {rendered}");
    assert!(rendered.contains("error mode"), "got {rendered}");
    assert!(!rendered.contains("transaction"), "got {rendered}");

    assert_eq!(format!("{:?}", CaptureSet::NONE), "CaptureSet(none)");
}

#[test]
fn debug_delimits_single_and_multiple_aspects_exactly() {
    assert_eq!(
        format!("{:?}", CaptureSet::IMPERSONATION),
        "CaptureSet(impersonation)"
    );
    assert_eq!(
        format!("{:?}", CaptureSet::DEFAULT),
        "CaptureSet(impersonation, error mode)"
    );
    assert_eq!(
        format!("{:?}", CaptureSet::ALL),
        "CaptureSet(impersonation, error mode, transaction)"
    );
}

#[test]
fn an_aspect_converts_into_its_singleton_set() {
    for aspect in CapturableAspect::EVERY {
        assert_eq!(CaptureSet::from(*aspect), aspect.as_set());
    }
}

#[test]
fn a_capture_set_is_copy_and_send_so_it_can_reach_a_worker() {
    fn assert_send_copy<T: Send + Copy>() {}
    assert_send_copy::<CaptureSet>();
    assert_send_copy::<CapturableAspect>();
}
