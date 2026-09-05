// Copyright (c) 2026 Mike Grier
use super::ProcessorSet;

#[test]
fn an_empty_set_is_empty() {
    let set = ProcessorSet::empty();
    assert!(set.is_empty());
    assert_eq!(set.len(), 0);
    assert!(!set.contains(0, 0));
}

#[test]
fn insert_and_contains_agree() {
    let mut set = ProcessorSet::empty();
    set.insert(0, 3);
    set.insert(0, 5);
    assert!(set.contains(0, 3));
    assert!(set.contains(0, 5));
    assert!(!set.contains(0, 4));
    assert!(
        !set.contains(1, 3),
        "a different group must not see group 0's members"
    );
    assert_eq!(set.len(), 2);
}

#[test]
#[should_panic(expected = "at most 64 processors")]
fn insert_rejects_a_processor_number_at_the_word_boundary() {
    let mut set = ProcessorSet::empty();
    set.insert(0, 64);
}

#[test]
fn contains_returns_false_rather_than_panicking_for_an_out_of_range_number() {
    let set = ProcessorSet::from_group_mask(0, 1);
    assert!(!set.contains(0, 64));
    assert!(!set.contains(0, 200));
}

#[test]
fn a_multi_group_set_keeps_groups_separate() {
    let mut set = ProcessorSet::empty();
    set.insert(0, 0);
    set.insert(1, 0);
    set.insert(1, 1);
    assert_eq!(set.len(), 3);
    assert_eq!(set.group_mask(0), 0b1);
    assert_eq!(set.group_mask(1), 0b11);
    assert_eq!(
        set.group_mask(2),
        0,
        "a group with no members reports an empty mask"
    );

    let mut masks: Vec<(u16, usize)> = set.group_masks().collect();
    masks.sort_unstable();
    assert_eq!(masks, vec![(0, 0b1), (1, 0b11)]);

    let mut members: Vec<(u16, u8)> = set.iter().collect();
    members.sort_unstable();
    assert_eq!(members, vec![(0, 0), (1, 0), (1, 1)]);
}

#[test]
fn from_group_mask_of_zero_is_empty() {
    // A group entry with no bits set carries no information, so it must not
    // be recorded -- otherwise `group_masks()` would report a phantom group.
    let set = ProcessorSet::from_group_mask(7, 0);
    assert!(set.is_empty());
    assert_eq!(set.group_masks().count(), 0);
}

#[test]
fn union_combines_both_sets_across_groups() {
    let a = ProcessorSet::from_group_mask(0, 0b0011);
    let b = ProcessorSet::from_group_mask(0, 0b1100).union(&ProcessorSet::from_group_mask(1, 0b1));
    let combined = a.union(&b);
    assert_eq!(combined.group_mask(0), 0b1111);
    assert_eq!(combined.group_mask(1), 0b1);
    assert_eq!(combined.len(), 5);
}

#[test]
fn intersection_keeps_only_shared_members() {
    let a = ProcessorSet::from_group_mask(0, 0b0111);
    let b = ProcessorSet::from_group_mask(0, 0b0110).union(&ProcessorSet::from_group_mask(1, 0b1));
    let shared = a.intersection(&b);
    assert_eq!(shared.group_mask(0), 0b0110);
    assert_eq!(
        shared.group_mask(1),
        0,
        "group 1 is only in `b`, so it cannot be in the intersection"
    );
    assert_eq!(shared.len(), 2);
}

#[test]
fn is_disjoint_is_true_only_when_nothing_is_shared() {
    let a = ProcessorSet::from_group_mask(0, 0b01);
    let b = ProcessorSet::from_group_mask(0, 0b10);
    assert!(a.is_disjoint(&b));

    let c = ProcessorSet::from_group_mask(0, 0b11);
    assert!(!a.is_disjoint(&c));
}

#[test]
fn is_disjoint_across_groups_with_no_overlap_in_group_ids() {
    let a = ProcessorSet::from_group_mask(0, 0b1);
    let b = ProcessorSet::from_group_mask(1, 0b1);
    assert!(a.is_disjoint(&b));
}

#[test]
fn is_subset_is_true_only_when_every_processor_is_covered() {
    let pair = ProcessorSet::from_group_mask(0, 0b011);
    let triple = ProcessorSet::from_group_mask(0, 0b111);

    assert!(pair.is_subset(&triple));
    assert!(!triple.is_subset(&pair));
}

#[test]
fn is_subset_is_reflexive_but_that_is_not_strictness() {
    // The order's comparison is built from this plus an inequality check, so
    // this method deliberately answers `true` for equal sets.
    let set = ProcessorSet::from_group_mask(0, 0b101);
    assert!(set.is_subset(&set.clone()));
}

#[test]
fn is_subset_is_false_when_a_group_is_missing_entirely() {
    // The case that separates "every group I name is covered" from "every
    // group I name is *present*": group 1 does not appear in `other` at all,
    // so the answer must be false rather than vacuously true.
    let spans_two_groups = {
        let mut set = ProcessorSet::from_group_mask(0, 0b1);
        set.insert(1, 0);
        set
    };
    let one_group = ProcessorSet::from_group_mask(0, 0b1);

    assert!(!spans_two_groups.is_subset(&one_group));
    assert!(one_group.is_subset(&spans_two_groups));
}

#[test]
fn is_subset_holds_across_several_groups() {
    let smaller = {
        let mut set = ProcessorSet::empty();
        set.insert(0, 1);
        set.insert(3, 2);
        set
    };
    let larger = {
        let mut set = ProcessorSet::empty();
        set.insert(0, 1);
        set.insert(0, 5);
        set.insert(3, 2);
        set.insert(3, 7);
        set
    };

    assert!(smaller.is_subset(&larger));
    assert!(!larger.is_subset(&smaller));
}

#[test]
fn is_subset_is_false_when_one_group_of_several_is_not_covered() {
    // Covered in group 0, not covered in group 1. A per-group check that
    // stopped at the first match would wrongly answer true.
    let left = {
        let mut set = ProcessorSet::empty();
        set.insert(0, 1);
        set.insert(1, 4);
        set
    };
    let right = {
        let mut set = ProcessorSet::empty();
        set.insert(0, 1);
        set.insert(1, 5);
        set
    };

    assert!(!left.is_subset(&right));
}

#[test]
fn the_empty_set_is_a_subset_of_everything_including_itself() {
    let empty = ProcessorSet::empty();
    let populated = ProcessorSet::from_group_mask(0, 0b1);

    assert!(empty.is_subset(&populated));
    assert!(empty.is_subset(&ProcessorSet::empty()));
    assert!(!populated.is_subset(&empty));
}

#[test]
fn is_subset_and_is_disjoint_agree_where_they_must() {
    // Two non-empty sets cannot be both, and a set that is a subset of a
    // non-empty set is never disjoint from it. Checked because the two
    // methods are the order's only set predicates and a sign error in either
    // is invisible from the other's tests.
    let small = ProcessorSet::from_group_mask(0, 0b001);
    let big = ProcessorSet::from_group_mask(0, 0b111);
    let apart = ProcessorSet::from_group_mask(0, 0b110);

    assert!(small.is_subset(&big) && !small.is_disjoint(&big));
    assert!(!small.is_subset(&apart) && small.is_disjoint(&apart));
}

#[test]
fn from_iter_builds_the_same_set_as_repeated_insert() {
    let via_insert = {
        let mut set = ProcessorSet::empty();
        set.insert(0, 1);
        set.insert(2, 3);
        set
    };
    let via_from_iter: ProcessorSet = [(0_u16, 1_u8), (2, 3)].into_iter().collect();
    assert_eq!(via_insert, via_from_iter);
}

#[test]
fn clone_and_equality_agree() {
    let set = ProcessorSet::from_group_mask(0, 0b101);
    assert_eq!(set, set.clone());
}

// --- serde (M3) ---

#[cfg(feature = "serde")]
#[test]
fn a_multi_group_set_round_trips_through_json() {
    let mut set = ProcessorSet::empty();
    set.insert(0, 0);
    set.insert(0, 5);
    set.insert(2, 1);
    let json = serde_json::to_string(&set).expect("serialize");
    let back: ProcessorSet = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(set, back);
}

#[cfg(feature = "serde")]
#[test]
fn an_empty_set_serializes_as_an_empty_array() {
    let json = serde_json::to_string(&ProcessorSet::empty()).expect("serialize");
    assert_eq!(json, "[]");
}

#[cfg(feature = "serde")]
#[test]
fn deserializing_a_processor_number_at_the_word_boundary_errors_rather_than_panics() {
    // The Rust-level insert() panics on an out-of-range number; the
    // deserialize boundary must not inherit that panic for untrusted input
    // (D-10 in DESIGN-NOTES.md).
    let json = r#"[{"group":0,"number":64}]"#;
    let error = serde_json::from_str::<ProcessorSet>(json).expect_err("64 is out of range");
    assert!(
        error.to_string().contains("64"),
        "error should name the offending number: {error}"
    );
}

#[cfg(feature = "serde")]
#[test]
fn deserializing_a_well_formed_description_produces_the_expected_set() {
    let json = r#"[{"group":0,"number":0},{"group":0,"number":3},{"group":1,"number":0}]"#;
    let set: ProcessorSet = serde_json::from_str(json).expect("deserialize");
    assert!(set.contains(0, 0));
    assert!(set.contains(0, 3));
    assert!(set.contains(1, 0));
    assert_eq!(set.len(), 3);
}

#[test]
fn a_populated_set_is_not_empty() {
    // A `cargo mutants` run replaced `is_empty` with `true` and the suite
    // passed: every existing test asserted only that an *empty* set reports
    // empty, so a predicate that always said "yes" satisfied all of them.
    //
    // One-sided assertions on a boolean accessor are the classic shape of this
    // gap -- the tests were correct and jointly proved nothing.
    let mut set = ProcessorSet::empty();
    set.insert(0, 3);
    assert!(!set.is_empty(), "a set with a member is not empty");
    assert_eq!(set.len(), 1);
}

#[test]
fn a_set_built_from_a_nonzero_mask_is_not_empty() {
    let set = ProcessorSet::from_group_mask(0, 0b1011);
    assert!(!set.is_empty());
    assert_eq!(set.len(), 3);
}

#[test]
fn emptiness_tracks_the_contents_across_groups() {
    // A member in any group is enough, which is what makes `is_empty` a
    // statement about the whole set rather than about group 0.
    let mut set = ProcessorSet::empty();
    assert!(set.is_empty());
    set.insert(5, 0);
    assert!(!set.is_empty(), "a member in a non-zero group still counts");
}
