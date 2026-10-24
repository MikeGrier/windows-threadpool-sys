// Copyright (c) 2026 Mike Grier

use crate::map::tests::{Collide, LAST_OF_SIXTEEN, Seeded};
use crate::{Full, HeapSet, InlineSet};
use core::hash::{BuildHasher, Hash, Hasher};
use std::format;
use std::string::String;
use std::vec::Vec;

/// Equal, and hashed alike, by `key` alone, so a replace or a `get` can be told apart by `tag`.
#[derive(Clone, Copy, Debug)]
struct Tagged {
    key: u32,
    tag: u32,
}

impl PartialEq for Tagged {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl Eq for Tagged {}

impl Hash for Tagged {
    fn hash<S: Hasher>(&self, state: &mut S) {
        self.key.hash(state);
    }
}

fn tagged(key: u32, tag: u32) -> Tagged {
    Tagged { key, tag }
}

fn sorted<'a, T: Ord + Copy + 'a>(values: impl IntoIterator<Item = &'a T>) -> Vec<T> {
    let mut values: Vec<T> = values.into_iter().copied().collect();
    values.sort_unstable();
    values
}

fn set_of<const N: usize, H: BuildHasher>(hasher: H, values: &[u32]) -> InlineSet<u32, N, H> {
    let mut set = InlineSet::with_hasher(hasher);
    set.try_extend(values.iter().copied()).expect("room");
    set
}

fn plain<const N: usize>(values: &[u32]) -> InlineSet<u32, N> {
    set_of(crate::FixedState, values)
}

#[test]
fn insert_reports_whether_a_value_was_new() {
    let mut set: InlineSet<u32, 8> = InlineSet::new();
    assert!(set.is_empty());
    assert_eq!(set.insert(3), Ok(true));
    assert_eq!(set.insert(3), Ok(false));
    assert_eq!(set.len(), 1);
    assert!(set.contains(&3));
    assert!(!set.contains(&4));
}

#[test]
fn a_full_set_refuses_a_new_value_and_hands_it_back_but_accepts_a_present_one() {
    let mut set: InlineSet<u32, 8> = plain(&[0, 1, 2, 3, 4, 5, 6]);
    assert_eq!(set.capacity(), 7);
    assert_eq!(set.insert(7), Err(Full { key: 7, value: () }));
    assert_eq!(set.insert(6), Ok(false));
    assert_eq!(set.len(), 7);
    assert!(set.remove(&0));
    assert_eq!(set.insert(7), Ok(true));
}

#[test]
fn insert_keeps_the_value_already_held() {
    let mut set: InlineSet<Tagged, 8> = InlineSet::new();
    set.insert(tagged(1, 10)).expect("room");
    assert_eq!(set.insert(tagged(1, 20)), Ok(false));
    assert_eq!(set.get(&tagged(1, 0)).map(|held| held.tag), Some(10));
    assert_eq!(set.get(&tagged(2, 0)).map(|held| held.tag), None);
}

#[test]
fn replace_swaps_in_the_new_value_and_returns_the_old() {
    let mut set: InlineSet<Tagged, 8> = InlineSet::new();
    assert_eq!(
        set.replace(tagged(1, 10)).map(|old| old.map(|t| t.tag)),
        Ok(None)
    );
    assert_eq!(
        set.replace(tagged(1, 20)).map(|old| old.map(|t| t.tag)),
        Ok(Some(10))
    );
    assert_eq!(set.get(&tagged(1, 0)).map(|held| held.tag), Some(20));
    assert_eq!(set.len(), 1);
}

#[test]
fn replace_on_a_full_set_refuses_only_a_new_value() {
    let mut set: InlineSet<Tagged, 8> = InlineSet::new();
    for key in 0..7 {
        set.insert(tagged(key, key)).expect("room");
    }
    let present = set.replace(tagged(3, 99)).expect("present needs no room");
    assert_eq!(present.map(|old| old.tag), Some(3));
    assert_eq!(set.get(&tagged(3, 0)).map(|held| held.tag), Some(99));
    let refused = set.replace(tagged(7, 7)).expect_err("full");
    assert_eq!(refused.key.key, 7);
    assert_eq!(set.len(), 7);
}

#[test]
fn take_returns_the_held_value_and_remove_reports_presence() {
    let mut set: InlineSet<Tagged, 8> = InlineSet::new();
    set.insert(tagged(1, 10)).expect("room");
    set.insert(tagged(2, 20)).expect("room");
    assert_eq!(set.take(&tagged(1, 0)).map(|held| held.tag), Some(10));
    assert_eq!(set.take(&tagged(1, 0)).map(|held| held.tag), None);
    assert!(set.remove(&tagged(2, 0)));
    assert!(!set.remove(&tagged(2, 0)));
    assert!(set.is_empty());
}

#[test]
fn lookups_take_a_borrowed_form() {
    let mut set: InlineSet<String, 8> = InlineSet::new();
    set.insert(String::from("ring")).expect("room");
    assert!(set.contains("ring"));
    assert_eq!(set.get("ring").map(String::as_str), Some("ring"));
    assert!(set.remove("ring"));
    assert!(!set.contains("ring"));
}

#[test]
fn removal_under_collision_keeps_the_rest_reachable() {
    let mut set: InlineSet<u32, 16, Collide> = InlineSet::with_hasher(Collide(LAST_OF_SIXTEEN));
    set.try_extend(0..14).expect("room");
    for value in [5, 0, 13] {
        assert!(set.remove(&value));
    }
    for value in 0..14 {
        assert_eq!(
            set.contains(&value),
            ![5, 0, 13].contains(&value),
            "{value}"
        );
    }
}

#[test]
fn clear_empties_and_keeps_the_room() {
    let mut set: InlineSet<u32, 8> = plain(&[1, 2, 3]);
    set.clear();
    assert!(set.is_empty());
    set.try_extend(10..17).expect("all the room back");
}

#[test]
fn retain_keeps_what_the_test_answers_true_for_visiting_each_once() {
    let mut set: InlineSet<u32, 16, Collide> = InlineSet::with_hasher(Collide(LAST_OF_SIXTEEN));
    set.try_extend(0..14).expect("room");
    let mut visits = [0u32; 14];
    set.retain(|value| {
        visits[*value as usize] += 1;
        value % 2 == 0
    });
    assert_eq!(visits, [1; 14]);
    assert_eq!(sorted(&set), std::vec![0, 2, 4, 6, 8, 10, 12]);
}

#[test]
fn iteration_yields_each_value_once_with_an_exact_length() {
    let set: InlineSet<u32, 16> = plain(&[4, 8, 15, 16, 23, 42]);
    let mut values = set.iter();
    assert_eq!(values.len(), 6);
    values.next();
    assert_eq!(values.len(), 5);
    assert_eq!(sorted(&set), std::vec![4, 8, 15, 16, 23, 42]);
    let mut owned: Vec<u32> = set.into_iter().collect();
    owned.sort_unstable();
    assert_eq!(owned, std::vec![4, 8, 15, 16, 23, 42]);

    let mut heap: HeapSet<u32> = HeapSet::try_with_capacity(3).expect("allocated");
    heap.insert(9).expect("room");
    let mut values = heap.into_iter();
    assert_eq!(
        (values.len(), values.next(), values.next()),
        (1, Some(9), None)
    );
}

#[test]
fn a_drain_dropped_part_way_still_empties_the_set() {
    let mut set: InlineSet<u32, 8> = plain(&[1, 2, 3, 4]);
    {
        let mut drain = set.drain();
        assert_eq!(drain.len(), 4);
        drain.next().expect("one");
    }
    assert!(set.is_empty());
    assert_eq!(set.drain().count(), 0);
}

#[test]
fn the_set_operations_agree_across_storages_and_hashers() {
    let a: InlineSet<u32, 16> = plain(&[1, 2, 3, 4, 5]);
    let b: InlineSet<u32, 32, Seeded> = set_of(Seeded(7), &[4, 5, 6, 7]);
    let mut c: HeapSet<u32> = HeapSet::try_with_capacity(10).expect("allocated");
    c.try_extend([4, 5, 6, 7]).expect("room");
    for (name, other) in [("seeded", sorted(&b)), ("heap", sorted(&c))] {
        assert_eq!(other, std::vec![4, 5, 6, 7], "{name}");
    }
    assert_eq!(sorted(a.difference(&b)), std::vec![1, 2, 3]);
    assert_eq!(sorted(b.difference(&a)), std::vec![6, 7]);
    assert_eq!(sorted(a.intersection(&c)), std::vec![4, 5]);
    assert_eq!(sorted(c.intersection(&a)), std::vec![4, 5]);
    assert_eq!(sorted(a.symmetric_difference(&c)), std::vec![1, 2, 3, 6, 7]);
    assert_eq!(sorted(a.union(&b)), std::vec![1, 2, 3, 4, 5, 6, 7]);
    assert_eq!(a.union(&c).count(), 7, "each value once");
}

#[test]
fn the_set_operations_with_an_empty_set() {
    let a: InlineSet<u32, 8> = plain(&[1, 2]);
    let empty: InlineSet<u32, 8> = InlineSet::new();
    assert_eq!(sorted(a.difference(&empty)), std::vec![1, 2]);
    assert_eq!(empty.difference(&a).count(), 0);
    assert_eq!(a.intersection(&empty).count(), 0);
    assert_eq!(sorted(empty.symmetric_difference(&a)), std::vec![1, 2]);
    assert_eq!(sorted(empty.union(&a)), std::vec![1, 2]);
    assert!(empty.is_subset(&a) && a.is_superset(&empty) && a.is_disjoint(&empty));
}

#[test]
fn subset_superset_and_disjoint() {
    let small: InlineSet<u32, 8> = plain(&[2, 3]);
    let big: InlineSet<u32, 16, Seeded> = set_of(Seeded(1), &[1, 2, 3, 4]);
    let apart: InlineSet<u32, 8> = plain(&[8, 9]);
    assert!(small.is_subset(&big) && !big.is_subset(&small));
    assert!(big.is_superset(&small) && !small.is_superset(&big));
    assert!(small.is_subset(&small) && small.is_superset(&small));
    assert!(small.is_disjoint(&apart) && big.is_disjoint(&apart) && apart.is_disjoint(&big));
    assert!(!small.is_disjoint(&big) && !big.is_disjoint(&small));
}

#[test]
fn sets_with_the_same_values_are_equal_whatever_their_storage_and_hasher() {
    let a: InlineSet<u32, 8> = plain(&[1, 2, 3]);
    let b: InlineSet<u32, 32, Seeded> = set_of(Seeded(3), &[3, 2, 1]);
    let mut c: HeapSet<u32> = HeapSet::try_with_capacity(3).expect("allocated");
    c.try_extend([2, 3, 1]).expect("room");
    assert!(a == b && b == a && a == c && c == b);
    c.insert(4).expect_err("full at three");
    assert!(c.remove(&1));
    c.insert(4).expect("room");
    assert!(a != c && c != a);
}

#[test]
fn debug_shows_the_values() {
    let mut set: InlineSet<u32, 8> = InlineSet::new();
    assert_eq!(format!("{set:?}"), "{}");
    set.insert(5).expect("room");
    assert_eq!(format!("{set:?}"), "{5}");
}

#[test]
fn clones_are_independent() {
    let set: InlineSet<u32, 8, Seeded> = set_of(Seeded(2), &[1]);
    let mut copy = set.clone();
    copy.insert(2).expect("room");
    assert_eq!((set.len(), copy.len(), *copy.hasher()), (1, 2, Seeded(2)));

    let mut heap: HeapSet<u32> = HeapSet::try_with_capacity(5).expect("allocated");
    heap.try_extend(0..5).expect("room");
    let mut copy = heap.try_clone().expect("allocated");
    assert_eq!(copy, heap);
    assert!(copy.remove(&0));
    assert!(heap.contains(&0));
}

#[test]
fn try_extend_stops_at_the_first_refusal_and_leaves_the_rest_untaken() {
    let mut set: InlineSet<u32, 8> = InlineSet::new();
    let mut values = [1, 1, 2, 3, 4, 5, 6, 7, 8, 9].into_iter();
    let refused = set.try_extend(values.by_ref()).expect_err("full");
    assert_eq!(refused.key, 8);
    assert_eq!(set.len(), 7);
    assert_eq!(values.collect::<Vec<_>>(), std::vec![9]);
}

#[test]
fn a_heap_set_holds_at_least_what_it_was_asked_for() {
    for asked in [0, 1, 7, 8, 100] {
        let set: HeapSet<u32> = HeapSet::try_with_capacity(asked).expect("allocated");
        assert!(set.capacity() >= asked, "{asked}");
    }
    let default: InlineSet<u32, 8> = InlineSet::default();
    assert_eq!(default.capacity(), 7);
}

#[test]
fn a_set_is_iterated_by_reference() {
    let set: InlineSet<u32, 8> = plain(&[1, 2, 3]);
    let mut total = 0;
    for value in &set {
        total += value;
    }
    assert_eq!(total, 6);
}
