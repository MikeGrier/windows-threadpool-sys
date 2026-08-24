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
