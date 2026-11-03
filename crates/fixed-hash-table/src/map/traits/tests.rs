// Copyright (c) 2026 Mike Grier

use super::super::tests::Seeded;
use crate::{Full, HeapMap, InlineMap};
use std::format;
use std::vec::Vec;

#[test]
fn debug_shows_the_entries() {
    let mut map: InlineMap<u32, &str, 8> = InlineMap::new();
    assert_eq!(format!("{map:?}"), "{}");
    map.insert(1, "one").expect("room");
    assert_eq!(format!("{map:?}"), "{1: \"one\"}");
}

#[test]
fn maps_with_the_same_entries_are_equal_whatever_their_storage_and_hasher() {
    let mut inline: InlineMap<u32, u32, 16> = InlineMap::new();
    let mut seeded: InlineMap<u32, u32, 32, Seeded> = InlineMap::with_hasher(Seeded(9));
    let mut heap: HeapMap<u32, u32> = HeapMap::try_with_capacity(40).expect("allocated");
    for key in 0..10 {
        inline.insert(key, key).expect("room");
        seeded.insert(key, key).expect("room");
        heap.insert(key, key).expect("room");
    }
    assert!(inline == seeded && inline == heap && heap == inline);
    heap.insert(3, 99).expect("replace");
    assert!(inline != heap, "a value differs");
    heap.insert(3, 3).expect("replace");
    heap.insert(10, 10).expect("room");
    assert!(inline != heap && heap != inline, "an extra key");
}

#[test]
fn a_map_equals_itself() {
    let mut map: InlineMap<u32, u32, 8> = InlineMap::new();
    map.insert(1, 1).expect("room");
    assert_eq!(map, map.clone());
}

#[test]
fn index_reads_a_present_key() {
    let mut map: InlineMap<u32, u32, 8> = InlineMap::new();
    map.insert(4, 40).expect("room");
    assert_eq!(map[&4], 40);
}

#[test]
#[should_panic(expected = "no entry for the key")]
fn index_panics_on_an_absent_key() {
    let map: InlineMap<u32, u32, 8> = InlineMap::new();
    let _ = map[&4];
}

#[test]
fn a_clone_of_an_inline_map_is_independent() {
    let mut map: InlineMap<u32, u32, 8, Seeded> = InlineMap::with_hasher(Seeded(3));
    map.insert(1, 1).expect("room");
    let mut copy = map.clone();
    copy.insert(2, 2).expect("room");
    assert_eq!((map.len(), copy.len()), (1, 2));
    assert_eq!((copy.capacity(), *copy.hasher()), (7, Seeded(3)));
}

#[test]
fn try_clone_copies_a_heap_map_into_slots_of_its_own() {
    let mut map: HeapMap<u32, u32, Seeded> =
        HeapMap::try_with_capacity_and_hasher(20, Seeded(5)).expect("allocated");
    for key in 0..20 {
        map.insert(key, key).expect("room");
    }
    let mut copy = map.try_clone().expect("allocated");
    assert_eq!(copy, map);
    copy.remove(&3);
    assert_eq!((map.len(), copy.len()), (20, 19));
    assert_eq!(map.get(&3), Some(&3));
    assert_eq!(*copy.hasher(), Seeded(5));
}

#[test]
fn try_extend_inserts_every_entry_while_there_is_room() {
    let mut map: InlineMap<u32, u32, 16> = InlineMap::new();
    map.try_extend((0..10).map(|k| (k, k))).expect("room");
    assert_eq!(map.len(), 10);
    map.try_extend([(3, 33), (20, 20)])
        .expect("a replace and one more");
    assert_eq!((map.get(&3), map.len()), (Some(&33), 11));
}

#[test]
fn try_extend_stops_at_the_first_refusal_and_leaves_the_rest_untaken() {
    let mut map: InlineMap<u32, u32, 8> = InlineMap::new();
    let mut entries = (0..10).map(|k| (k, k * 2));
    let refused = map.try_extend(entries.by_ref()).expect_err("full");
    assert_eq!(refused, Full { key: 7, value: 14 });
    assert_eq!(map.len(), 7);
    assert_eq!(entries.collect::<Vec<_>>(), std::vec![(8, 16), (9, 18)]);
}
