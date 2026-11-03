// Copyright (c) 2026 Mike Grier

use super::super::tests::Collide;
use super::Entry;
use crate::{Full, InlineMap};
use core::cell::Cell;
use std::vec::Vec;

fn full_of_sevens() -> InlineMap<u32, u32, 8> {
    let mut map = InlineMap::new();
    for key in 0..7 {
        map.insert(key, 7).expect("room");
    }
    map
}

#[test]
fn or_insert_counts_occurrences() {
    let mut counts: InlineMap<&str, u32, 8> = InlineMap::new();
    for word in ["a", "b", "a", "c", "a", "b"] {
        *counts.entry(word).or_insert(0).expect("room") += 1;
    }
    assert_eq!(
        (counts.get("a"), counts.get("b"), counts.get("c")),
        (Some(&3), Some(&2), Some(&1))
    );
}

#[test]
fn or_insert_on_a_full_map_refuses_an_absent_key_and_serves_a_present_one() {
    let mut map = full_of_sevens();
    assert_eq!(
        map.entry(99).or_insert(1).map(|v| *v),
        Err(Full { key: 99, value: 1 })
    );
    assert_eq!(map.entry(3).or_insert(1).map(|v| *v), Ok(7));
    assert_eq!(map.len(), 7);
}

#[test]
fn or_insert_with_does_not_make_a_value_it_cannot_place() {
    let mut map = full_of_sevens();
    let made = Cell::new(false);
    let refused = map
        .entry(99)
        .or_insert_with(|| {
            made.set(true);
            5
        })
        .map(|v| *v)
        .expect_err("full");
    assert!(!made.get(), "the closure is handed back uncalled");
    assert_eq!(refused.key, 99);
    assert_eq!((refused.value)(), 5);
    assert!(made.get());

    let mut room: InlineMap<u32, u32, 8> = InlineMap::new();
    assert_eq!(
        room.entry(1).or_insert_with(|| 10).map(|v| *v).ok(),
        Some(10)
    );
}

#[test]
fn or_insert_with_key_is_given_the_key() {
    let mut map: InlineMap<u32, u32, 8> = InlineMap::new();
    assert_eq!(
        map.entry(6).or_insert_with_key(|k| k * 2).map(|v| *v).ok(),
        Some(12)
    );
    assert_eq!(
        map.entry(6).or_insert_with_key(|_| 0).map(|v| *v).ok(),
        Some(12)
    );
    let mut full = full_of_sevens();
    let refused = full
        .entry(9)
        .or_insert_with_key(|k| k * 2)
        .map(|v| *v)
        .expect_err("full");
    assert_eq!((refused.value)(&refused.key), 18);
}

#[test]
fn or_default_inserts_a_default_or_hands_one_back() {
    let mut map: InlineMap<u32, Vec<u32>, 8> = InlineMap::new();
    map.entry(1).or_default().expect("room").push(4);
    assert_eq!(map.get(&1), Some(&std::vec![4]));
    let mut full = full_of_sevens();
    assert_eq!(
        full.entry(42).or_default().map(|v| *v),
        Err(Full { key: 42, value: 0 })
    );
}

#[test]
fn and_modify_changes_only_a_present_value() {
    let mut map: InlineMap<u32, u32, 8> = InlineMap::new();
    map.insert(1, 1).expect("room");
    let one = map
        .entry(1)
        .and_modify(|v| *v += 10)
        .or_insert(0)
        .map(|v| *v);
    let two = map
        .entry(2)
        .and_modify(|v| *v += 10)
        .or_insert(0)
        .map(|v| *v);
    assert_eq!((one, two), (Ok(11), Ok(0)));
}

#[test]
fn an_entry_names_its_key() {
    let mut map: InlineMap<u32, u32, 8> = InlineMap::new();
    map.insert(1, 1).expect("room");
    assert_eq!(*map.entry(1).key(), 1);
    assert_eq!(*map.entry(2).key(), 2);
}

#[test]
fn an_occupied_entry_reads_changes_and_removes() {
    let mut map: InlineMap<u32, u32, 8> = InlineMap::new();
    map.insert(1, 10).expect("room");
    let Entry::Occupied(mut entry) = map.entry(1) else {
        panic!("present");
    };
    assert_eq!((*entry.key(), *entry.get()), (1, 10));
    *entry.get_mut() += 1;
    assert_eq!(entry.insert(20), 11);
    assert_eq!(*entry.into_mut(), 20);
    let Entry::Occupied(entry) = map.entry(1) else {
        panic!("present");
    };
    assert_eq!(entry.remove_entry(), (1, 20));
    assert!(map.is_empty());
}

#[test]
fn removing_through_an_entry_keeps_colliding_keys_reachable() {
    let mut map: InlineMap<u32, u32, 16, Collide> = InlineMap::with_hasher(Collide(0));
    for key in 0..14 {
        map.insert(key, key).expect("room");
    }
    for key in [3, 0, 13, 7] {
        let Entry::Occupied(entry) = map.entry(key) else {
            panic!("{key} present");
        };
        assert_eq!(entry.remove(), key);
    }
    for key in (0..14).filter(|k| ![3, 0, 13, 7].contains(k)) {
        assert_eq!(map.get(&key), Some(&key));
    }
}

#[test]
fn a_vacant_entry_inserts_or_is_refused_when_full() {
    let mut map: InlineMap<u32, u32, 8> = InlineMap::new();
    let Entry::Vacant(entry) = map.entry(5) else {
        panic!("absent");
    };
    assert_eq!(*entry.key(), 5);
    *entry.insert(50).expect("room") += 1;
    assert_eq!(map.get(&5), Some(&51));

    let mut full = full_of_sevens();
    let Entry::Vacant(entry) = full.entry(8) else {
        panic!("absent");
    };
    assert_eq!(
        entry.insert(80).map(|v| *v),
        Err(Full { key: 8, value: 80 })
    );
    let Entry::Vacant(entry) = full.entry(9) else {
        panic!("absent");
    };
    assert_eq!(entry.into_key(), 9);
}
