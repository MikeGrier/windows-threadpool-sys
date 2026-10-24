// Copyright (c) 2026 Mike Grier

use super::super::tests::{Collide, LAST_OF_SIXTEEN};
use crate::{HeapMap, InlineMap};
use std::vec::Vec;

fn sorted<T: Ord>(mut items: Vec<T>) -> Vec<T> {
    items.sort();
    items
}

fn ten() -> InlineMap<u32, u32, 16> {
    let mut map = InlineMap::new();
    for key in 0..10 {
        map.insert(key, key * 10).expect("room");
    }
    map
}

#[test]
fn iter_yields_every_entry_once_and_knows_how_many_are_left() {
    let map = ten();
    let mut iter = map.iter();
    assert_eq!(iter.len(), 10);
    iter.next().expect("one");
    assert_eq!(iter.len(), 9);
    let seen = sorted(map.iter().map(|(k, v)| (*k, *v)).collect());
    assert_eq!(seen, (0..10).map(|k| (k, k * 10)).collect::<Vec<_>>());
    let mut done = map.iter();
    for _ in 0..10 {
        done.next();
    }
    assert_eq!((done.next(), done.next()), (None, None), "fused");
}

#[test]
fn an_empty_map_iterates_nothing() {
    let map: InlineMap<u32, u32, 8> = InlineMap::new();
    assert_eq!(map.iter().count(), 0);
    assert_eq!(map.keys().len(), 0);
}

#[test]
fn iter_mut_and_values_mut_change_values_in_place() {
    let mut map = ten();
    for (_, value) in map.iter_mut() {
        *value += 1;
    }
    for value in map.values_mut() {
        *value *= 2;
    }
    for key in 0..10 {
        assert_eq!(map.get(&key), Some(&((key * 10 + 1) * 2)));
    }
}

#[test]
fn keys_and_values_are_the_entries_halves() {
    let map = ten();
    assert_eq!(
        sorted(map.keys().copied().collect()),
        (0..10).collect::<Vec<_>>()
    );
    assert_eq!(
        sorted(map.values().copied().collect()),
        (0..10).map(|k| k * 10).collect::<Vec<_>>()
    );
}

#[test]
fn a_map_is_iterated_by_reference_and_by_value() {
    let mut map = ten();
    let mut total = 0;
    for (_, value) in &map {
        total += value;
    }
    for (_, value) in &mut map {
        *value = 1;
    }
    assert_eq!(total, 450);
    let mut owned: Vec<(u32, u32)> = map.into_iter().collect();
    owned.sort_unstable();
    assert_eq!(owned, (0..10).map(|k| (k, 1)).collect::<Vec<_>>());

    let mut heap: HeapMap<u32, u32> = HeapMap::try_with_capacity(10).expect("allocated");
    heap.insert(3, 30).expect("room");
    let mut entries = heap.into_iter();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries.next(), Some((3, 30)));
    assert_eq!(entries.next(), None);
}

#[test]
fn drain_takes_every_entry_and_leaves_the_map_empty_and_usable() {
    let mut map = ten();
    let drained = sorted(map.drain().collect());
    assert_eq!(drained.len(), 10);
    assert!(map.is_empty());
    assert_eq!(map.get(&3), None);
    map.insert(3, 3).expect("room again");
    assert_eq!(map.len(), 1);
}

#[test]
fn a_drain_dropped_part_way_still_empties_the_map() {
    let mut map = ten();
    {
        let mut drain = map.drain();
        assert_eq!(drain.len(), 10);
        drain.next().expect("one");
        assert_eq!(drain.len(), 9);
    }
    assert!(map.is_empty());
    assert!(map.slots.iter().all(Option::is_none));
    for key in 0..14 {
        map.insert(key, key).expect("all the room back");
    }
}

#[test]
fn retain_keeps_what_the_test_answers_true_for() {
    let mut map = ten();
    map.retain(|k, v| {
        *v += 1;
        k % 2 == 0
    });
    assert_eq!(
        sorted(map.keys().copied().collect()),
        std::vec![0, 2, 4, 6, 8]
    );
    assert_eq!(map.get(&4), Some(&41));
    map.retain(|_, _| true);
    assert_eq!(map.len(), 5);
    map.retain(|_, _| false);
    assert!(map.is_empty());
}

#[test]
fn retain_visits_each_entry_once_even_as_removals_shift_a_run_that_wraps() {
    for drop_when in [0, 1, 2] {
        let mut map: InlineMap<u32, u32, 16, Collide> =
            InlineMap::with_hasher(Collide(LAST_OF_SIXTEEN));
        for key in 0..14 {
            map.insert(key, 0).expect("room");
        }
        map.retain(|key, visits| {
            *visits += 1;
            key % 3 != drop_when
        });
        for (key, visits) in &map {
            assert_eq!(
                *visits, 1,
                "{key} visited {visits} times, dropping {drop_when}"
            );
        }
        for key in 0..14 {
            assert_eq!(map.contains_key(&key), key % 3 != drop_when, "{key}");
        }
    }
}
