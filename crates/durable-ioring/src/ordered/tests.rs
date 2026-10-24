// Copyright (c) 2026 Mike Grier
//! The ordered table: its order, its refusals, and that its storage never grows.

use std::collections::BTreeMap;

use super::{Full, Ordered};

fn table(limit: usize) -> Ordered<u64, &'static str> {
    Ordered::with_capacity(limit).expect("a small table")
}

fn keys(table: &Ordered<u64, &'static str>) -> Vec<u64> {
    table.keys().copied().collect()
}

#[test]
fn entries_inserted_in_rising_order_come_back_in_that_order() {
    let mut t = table(4);
    for (key, value) in [(1, "a"), (2, "b"), (5, "c")] {
        assert!(matches!(t.insert(key, value), Ok(None)));
    }
    assert_eq!(keys(&t), [1, 2, 5]);
    assert_eq!(t.len(), 3);
}

#[test]
fn entries_inserted_out_of_order_are_kept_in_key_order() {
    let mut t = table(5);
    for key in [7, 3, 9, 1, 5] {
        t.insert(key, "x").expect("room");
    }
    assert_eq!(keys(&t), [1, 3, 5, 7, 9]);
    assert_eq!(t.iter().next(), Some((&1, &"x")));
    assert_eq!(t.iter().next_back(), Some((&9, &"x")));
}

#[test]
fn an_insert_under_a_present_key_replaces_and_hands_back_the_old_value() {
    let mut t = table(2);
    t.insert(4, "old").expect("room");
    assert!(matches!(t.insert(4, "new"), Ok(Some("old"))));
    assert_eq!(t.get(&4), Some(&"new"));
    assert_eq!(t.len(), 1);
}

#[test]
fn a_full_table_refuses_a_new_key_changing_nothing_and_still_replaces() {
    let mut t = table(2);
    t.insert(1, "a").expect("room");
    t.insert(2, "b").expect("room");
    assert_eq!(t.insert(3, "c"), Err(Full));
    assert_eq!(keys(&t), [1, 2]);
    assert_eq!(t.get(&3), None);
    assert!(
        matches!(t.insert(2, "B"), Ok(Some("b"))),
        "a present key is no growth"
    );
    assert_eq!(keys(&t), [1, 2]);
}

#[test]
fn a_table_made_for_nothing_refuses_everything() {
    let mut t = table(0);
    assert_eq!(t.insert(1, "a"), Err(Full));
    assert_eq!(t.len(), 0);
    assert_eq!(t.iter().next(), None);
}

#[test]
fn a_removal_takes_the_value_and_an_absent_key_takes_nothing() {
    let mut t = table(3);
    t.insert(1, "a").expect("room");
    t.insert(2, "b").expect("room");
    assert_eq!(t.remove(&1), Some("a"));
    assert_eq!(t.remove(&1), None, "removed once");
    assert_eq!(t.remove(&9), None, "never there");
    assert_eq!(t.get(&1), None);
    assert_eq!(t.get_mut(&1), None);
    assert_eq!(keys(&t), [2]);
    assert_eq!(t.len(), 1);
}

#[test]
fn a_removed_key_inserted_again_takes_its_old_slot() {
    let mut t = table(3);
    for key in [1, 2, 3] {
        t.insert(key, "x").expect("room");
    }
    t.remove(&2);
    assert_eq!(t.entries.len(), 3, "left in place");
    t.insert(2, "again").expect("its slot");
    assert_eq!(t.get(&2), Some(&"again"));
    assert_eq!(t.entries.len(), 3);
}

#[test]
fn removing_the_last_entries_drops_their_slots() {
    let mut t = table(4);
    for key in [1, 2, 3] {
        t.insert(key, "x").expect("room");
    }
    t.remove(&2);
    t.remove(&3);
    assert_eq!(t.entries.len(), 1, "the trailing empty slots go");
    assert_eq!(keys(&t), [1]);
}

#[test]
fn a_full_table_with_removed_slots_compacts_to_take_an_entry_between_present_ones() {
    let mut t = table(4);
    let reserved = t.entries.capacity();
    for key in [10, 20, 30, 40] {
        t.insert(key, "x").expect("room");
    }
    t.remove(&10);
    t.remove(&20);
    // 35 goes between 30 and 40, both present: no empty slot beside it, and the array is full.
    t.insert(35, "y").expect("room after compacting");
    assert_eq!(keys(&t), [30, 35, 40]);
    assert_eq!(t.entries.capacity(), reserved, "never grown");
    t.insert(5, "z").expect("room");
    assert_eq!(keys(&t), [5, 30, 35, 40]);
    assert!(t.insert(50, "w").is_err());
}

#[test]
fn an_entry_takes_an_empty_slot_beside_its_place() {
    let mut t = table(4);
    for key in [10, 20, 30] {
        t.insert(key, "x").expect("room");
    }
    t.remove(&20);
    t.insert(25, "after").expect("room");
    assert_eq!(t.entries.len(), 3, "the empty slot before it was taken");
    t.remove(&25);
    t.insert(15, "before").expect("room");
    assert_eq!(t.entries.len(), 3, "the empty slot after it was taken");
    assert_eq!(keys(&t), [10, 15, 30]);
}

#[test]
fn between_covers_the_keys_its_predicates_bound_and_skips_removed_entries() {
    let mut t = table(8);
    for key in 1..=8 {
        t.insert(key, "x").expect("room");
    }
    t.remove(&4);
    let of = |low: u64, high: u64| -> Vec<u64> {
        t.between(|key| *key < low, |key| *key <= high)
            .map(|(key, _)| *key)
            .collect()
    };
    assert_eq!(of(2, 5), [2, 3, 5]);
    assert_eq!(of(0, 2), [1, 2]);
    assert_eq!(of(7, 99), [7, 8]);
    assert!(of(4, 4).is_empty(), "only a removed entry");
    assert!(of(7, 3).is_empty(), "an inverted range is empty");
    assert_eq!(
        t.between(|key| *key < 2, |key| *key <= 3).next_back(),
        Some((&3, &"x"))
    );
}

#[test]
fn between_mut_changes_only_the_present_entries_its_predicates_bound() {
    let mut t: Ordered<u64, u32> = Ordered::with_capacity(8).expect("a small table");
    for key in 1..=6 {
        t.insert(key, 0).expect("room");
    }
    t.remove(&3);
    for (_, value) in t.between_mut(|key| *key < 2, |key| *key <= 4) {
        *value += 1;
    }
    let values: Vec<(u64, u32)> = t.iter().map(|(key, value)| (*key, *value)).collect();
    assert_eq!(values, [(1, 0), (2, 1), (4, 1), (5, 0), (6, 0)]);
    assert_eq!(
        t.between_mut(|key| *key < 2, |key| *key <= 5)
            .next_back()
            .map(|(key, _)| *key),
        Some(5)
    );
    assert!(
        t.between_mut(|key| *key < 3, |key| *key <= 3)
            .next()
            .is_none()
    );
    assert!(
        t.between_mut(|key| *key < 5, |key| *key <= 2)
            .next()
            .is_none(),
        "an inverted range is empty"
    );
}

#[test]
fn values_change_through_a_mutable_range_and_iteration() {
    let mut t: Ordered<u64, u32> = Ordered::with_capacity(4).expect("a small table");
    for key in [1, 2, 3, 4] {
        t.insert(key, 0).expect("room");
    }
    for (key, value) in t.iter_mut() {
        if (2..=3).contains(key) {
            *value += 1;
        }
    }
    for value in t.values_mut() {
        *value += 10;
    }
    for (key, value) in t.iter_mut() {
        if *key == 4 {
            *value += 100;
        }
    }
    *t.get_mut(&1).expect("present") += 1000;
    assert_eq!(t.values().copied().collect::<Vec<_>>(), [1010, 11, 11, 110]);
    assert_eq!(t[&2], 11);
}

#[test]
fn retain_keeps_what_it_is_told_to_in_order() {
    let mut t = table(6);
    for key in 1..=6 {
        t.insert(key, "x").expect("room");
    }
    let mut seen = Vec::new();
    t.retain(|key, _| {
        seen.push(*key);
        key % 2 == 1
    });
    assert_eq!(seen, [1, 2, 3, 4, 5, 6], "every entry asked, in order");
    assert_eq!(keys(&t), [1, 3, 5]);
    assert_eq!(t.len(), 3);
    assert_eq!(
        t.entries.len(),
        5,
        "the trailing empty slot goes, the others stay"
    );
}

#[test]
fn take_where_gives_each_taken_entry_in_order_and_keeps_the_rest() {
    let mut t = table(5);
    for (key, value) in [(1, "keep"), (2, "take"), (3, "keep"), (4, "take")] {
        t.insert(key, value).expect("room");
    }
    let mut taken = Vec::new();
    t.take_where(
        |_, value| *value == "take",
        |key, value| taken.push((*key, value)),
    );
    assert_eq!(taken, [(2, "take"), (4, "take")]);
    assert_eq!(keys(&t), [1, 3]);
    assert_eq!(t.len(), 2);
}

#[test]
fn into_values_gives_the_present_values_in_key_order() {
    let mut t = table(4);
    for (key, value) in [(3, "c"), (1, "a"), (2, "b")] {
        t.insert(key, value).expect("room");
    }
    t.remove(&2);
    assert_eq!(t.into_values().collect::<Vec<_>>(), ["a", "c"]);
}

#[test]
#[should_panic(expected = "an entry under the key")]
fn indexing_an_absent_key_panics() {
    let t = table(1);
    let _ = t[&1];
}

/// A fixed sequence of inserts and removals against a `BTreeMap`: the same entries in the same
/// order after every step, the same refusals, and the storage never grown. The sequence comes from
/// a fixed linear congruential generator, so the run is the same every time.
#[test]
fn a_fixed_sequence_of_changes_matches_a_btree_map_and_never_grows_the_storage() {
    const LIMIT: usize = 16;
    let mut t: Ordered<u64, u64> = Ordered::with_capacity(LIMIT).expect("a small table");
    let reserved = t.entries.capacity();
    let mut model = BTreeMap::new();
    let mut state: u64 = 0x2545_F491_4F6C_DD1D;
    for step in 0..4000_u64 {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let key = (state >> 33) % 40;
        if (state >> 20).is_multiple_of(3) {
            assert_eq!(t.remove(&key), model.remove(&key), "step {step}");
        } else {
            let fits = model.len() < LIMIT || model.contains_key(&key);
            match t.insert(key, step) {
                Ok(old) => {
                    assert!(fits, "step {step}: accepted past the limit");
                    assert_eq!(old, model.insert(key, step), "step {step}");
                }
                Err(Full) => assert!(!fits, "step {step}: refused with room"),
            }
        }
        assert!(
            t.iter()
                .map(|(k, v)| (*k, *v))
                .eq(model.iter().map(|(k, v)| (*k, *v)))
        );
        assert_eq!(t.len(), model.len());
        assert_eq!(t.entries.capacity(), reserved, "step {step}: grown");
    }
}
