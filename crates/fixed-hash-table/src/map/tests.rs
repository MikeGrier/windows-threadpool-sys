// Copyright (c) 2026 Mike Grier

use super::{FixedHashMap, Full, HeapMap, InlineMap, ReserveError, capacity_of, slots_for};
use crate::FixedState;
use core::hash::{BuildHasher, Hash, Hasher};
use std::collections::HashMap;
use std::string::{String, ToString};
use std::vec::Vec;

/// Every key hashes to the one value it holds: total collision, from a stateful hasher.
#[derive(Clone, Copy, Debug)]
pub(super) struct Collide(pub(super) u64);

pub(super) struct Constant(u64);

impl Hasher for Constant {
    fn write(&mut self, _: &[u8]) {}

    fn finish(&self) -> u64 {
        self.0
    }
}

impl BuildHasher for Collide {
    type Hasher = Constant;

    fn build_hasher(&self) -> Constant {
        Constant(self.0)
    }
}

/// A keyed hasher: the seed is the per-table state FHT-D-5 allows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Seeded(pub(super) u64);

pub(super) struct SeededHasher(u64);

impl Hasher for SeededHasher {
    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 = (self.0 ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01B3);
        }
    }

    fn finish(&self) -> u64 {
        self.0.wrapping_mul(0x9E37_79B9_7F4A_7C15)
    }
}

impl BuildHasher for Seeded {
    type Hasher = SeededHasher;

    fn build_hasher(&self) -> SeededHasher {
        SeededHasher(self.0)
    }
}

/// A key equal to another of the same id whatever its tag, so a test can see which key is held.
#[derive(Clone, Debug)]
struct Tagged {
    id: u32,
    tag: &'static str,
}

impl PartialEq for Tagged {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for Tagged {}

impl Hash for Tagged {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

/// The hash whose home is the last of sixteen slots.
pub(super) const LAST_OF_SIXTEEN: u64 = 15 << 60;

#[test]
fn a_new_map_is_empty_and_holds_seven_eighths_of_its_slots() {
    assert_eq!(InlineMap::<u32, u32, 1>::new().capacity(), 0);
    assert_eq!(InlineMap::<u32, u32, 2>::new().capacity(), 1);
    assert_eq!(InlineMap::<u32, u32, 4>::new().capacity(), 3);
    assert_eq!(InlineMap::<u32, u32, 8>::new().capacity(), 7);
    assert_eq!(InlineMap::<u32, u32, 16>::new().capacity(), 14);
    assert_eq!(InlineMap::<u32, u32, 64>::new().capacity(), 56);
    let map: InlineMap<u32, u32, 16> = InlineMap::default();
    assert!(map.is_empty());
    assert_eq!(map.len(), 0);
}

#[test]
fn every_slot_count_keeps_at_least_one_slot_empty() {
    for power in 0..20 {
        let slots = 1usize << power;
        assert!(capacity_of(slots) < slots, "{slots} slots");
        assert!(capacity_of(slots) >= slots - slots / 8 - 1, "{slots} slots");
    }
}

#[test]
fn inserted_keys_are_found_and_others_are_not() {
    let mut map: InlineMap<u32, String, 32> = InlineMap::new();
    for key in 0..20 {
        assert_eq!(map.insert(key, key.to_string()), Ok(None));
    }
    assert_eq!(map.len(), 20);
    for key in 0..20 {
        assert_eq!(
            map.get(&key).map(String::as_str),
            Some(key.to_string().as_str())
        );
        assert!(map.contains_key(&key));
    }
    for key in 20..40 {
        assert_eq!(map.get(&key), None);
        assert!(!map.contains_key(&key));
    }
}

#[test]
fn an_insert_past_capacity_is_refused_and_handed_back() {
    let mut map: InlineMap<u32, u32, 16> = InlineMap::new();
    for key in 0..14 {
        map.insert(key, key * 10).expect("room");
    }
    assert_eq!(map.len(), map.capacity());
    let refused = map.insert(99, 990).expect_err("full");
    assert_eq!(
        refused,
        Full {
            key: 99,
            value: 990
        }
    );
    assert_eq!(map.len(), 14);
    assert!(!map.contains_key(&99));
}

#[test]
fn replacing_a_present_key_at_capacity_is_never_refused() {
    let mut map: InlineMap<u32, u32, 8> = InlineMap::new();
    for key in 0..7 {
        map.insert(key, 0).expect("room");
    }
    assert_eq!(map.insert(3, 33), Ok(Some(0)));
    assert_eq!(map.get(&3), Some(&33));
    assert_eq!(map.len(), 7);
}

#[test]
fn replacing_keeps_the_key_already_held() {
    let mut map: InlineMap<Tagged, u32, 8> = InlineMap::new();
    map.insert(
        Tagged {
            id: 1,
            tag: "first",
        },
        1,
    )
    .expect("room");
    map.insert(
        Tagged {
            id: 1,
            tag: "second",
        },
        2,
    )
    .expect("room");
    let (key, value) = map
        .get_key_value(&Tagged { id: 1, tag: "any" })
        .expect("present");
    assert_eq!((key.tag, *value), ("first", 2));
}

#[test]
fn a_removal_frees_room_for_another_key() {
    let mut map: InlineMap<u32, u32, 8> = InlineMap::new();
    for key in 0..7 {
        map.insert(key, key).expect("room");
    }
    map.insert(7, 7).expect_err("full");
    assert_eq!(map.remove(&2), Some(2));
    assert_eq!(map.len(), 6);
    assert_eq!(map.insert(7, 7), Ok(None));
    assert_eq!(map.get(&7), Some(&7));
}

#[test]
fn removing_an_absent_key_changes_nothing() {
    let mut map: InlineMap<u32, u32, 8> = InlineMap::new();
    map.insert(1, 1).expect("room");
    assert_eq!(map.remove(&2), None);
    assert_eq!(map.remove_entry(&2), None);
    assert_eq!(map.len(), 1);
    assert_eq!(map.remove_entry(&1), Some((1, 1)));
    assert!(map.is_empty());
}

#[test]
fn under_total_collision_every_removal_keeps_the_other_keys_reachable() {
    for order in [
        [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13],
        [13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0],
        [6, 0, 13, 3, 9, 1, 12, 4, 7, 10, 2, 11, 5, 8],
    ] {
        let mut map: InlineMap<u32, u32, 16, Collide> = InlineMap::with_hasher(Collide(0));
        for key in 0..14 {
            map.insert(key, key + 100).expect("room");
        }
        let mut left: Vec<u32> = (0..14).collect();
        for key in order {
            assert_eq!(map.remove(&key), Some(key + 100), "{key} in {order:?}");
            left.retain(|k| *k != key);
            for k in &left {
                assert_eq!(map.get(k), Some(&(k + 100)), "{k} after {key} in {order:?}");
            }
        }
        assert!(map.is_empty());
    }
}

#[test]
fn a_run_that_wraps_past_the_last_slot_survives_removals() {
    let mut map: InlineMap<u32, u32, 16, Collide> =
        InlineMap::with_hasher(Collide(LAST_OF_SIXTEEN));
    for key in 0..14 {
        map.insert(key, key).expect("room");
    }
    // The run starts in the last slot and wraps to the front; removing from either side of the
    // wrap must shift the rest back across it.
    let mut removed = Vec::new();
    for key in [0, 1, 7, 13] {
        assert_eq!(map.remove(&key), Some(key));
        removed.push(key);
        for k in (0..14).filter(|k| !removed.contains(k)) {
            assert_eq!(map.get(&k), Some(&k), "{k} after removing {removed:?}");
        }
    }
    for key in [0, 1, 7, 13] {
        assert_eq!(map.insert(key, key), Ok(None));
    }
    assert_eq!(map.len(), 14);
}

#[test]
fn a_removal_leaves_no_tombstone_to_slow_the_next_probe() {
    // After filling and emptying a colliding table, every slot is empty again, so a key absent
    // from it is answered by its home slot alone.
    let mut map: InlineMap<u32, u32, 16, Collide> = InlineMap::with_hasher(Collide(0));
    for round in 0..50 {
        for key in 0..14 {
            map.insert(key + round, key).expect("room");
        }
        for key in 0..14 {
            map.remove(&(key + round)).expect("present");
        }
    }
    assert!(map.slots.iter().all(Option::is_none));
}

#[test]
fn a_long_sequence_of_inserts_and_removals_agrees_with_std() {
    // A fixed-seed linear congruential sequence: deterministic, so the run is reproducible.
    let mut state: u64 = 0x2545_F491_4F6C_DD1D;
    let mut next = move || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        state >> 33
    };
    let mut map: InlineMap<u64, u64, 64> = InlineMap::new();
    let mut model: HashMap<u64, u64> = HashMap::new();
    for step in 0..10_000 {
        let key = next() % 96;
        let value = next();
        if next() % 3 == 0 {
            assert_eq!(map.remove(&key), model.remove(&key), "step {step}");
        } else if model.contains_key(&key) || model.len() < map.capacity() {
            assert_eq!(
                map.insert(key, value).ok(),
                Some(model.insert(key, value)),
                "step {step}"
            );
        } else {
            assert_eq!(
                map.insert(key, value),
                Err(Full { key, value }),
                "step {step}"
            );
        }
        assert_eq!(map.len(), model.len(), "step {step}");
    }
    for (key, value) in &model {
        assert_eq!(map.get(key), Some(value));
    }
}

#[test]
fn a_value_is_changed_in_place_through_get_mut() {
    let mut map: InlineMap<u32, u32, 8> = InlineMap::new();
    map.insert(5, 50).expect("room");
    *map.get_mut(&5).expect("present") += 1;
    assert_eq!(map.get(&5), Some(&51));
    assert_eq!(map.get_mut(&6), None);
}

#[test]
fn clearing_empties_the_map_and_keeps_its_capacity() {
    let mut map: InlineMap<u32, u32, 8> = InlineMap::new();
    for key in 0..7 {
        map.insert(key, key).expect("room");
    }
    map.clear();
    assert!(map.is_empty());
    assert_eq!(map.capacity(), 7);
    assert_eq!(map.get(&3), None);
    for key in 10..17 {
        map.insert(key, key).expect("room again");
    }
}

#[test]
fn a_string_key_is_looked_up_by_str() {
    let mut map: InlineMap<String, u32, 8> = InlineMap::new();
    map.insert("log".to_string(), 1).expect("room");
    assert_eq!(map.get("log"), Some(&1));
    assert!(map.contains_key("log"));
    assert_eq!(map.remove("log"), Some(1));
}

#[test]
fn a_heap_map_holds_what_it_was_asked_to_and_refuses_past_it() {
    for asked in [0, 1, 7, 8, 100, 1000] {
        let mut map: HeapMap<usize, usize> = HeapMap::try_with_capacity(asked).expect("allocated");
        assert!(map.capacity() >= asked, "{asked}");
        assert!(map.slots.len().is_power_of_two());
        let capacity = map.capacity();
        for key in 0..capacity {
            map.insert(key, key).expect("room");
        }
        map.insert(capacity, 0).expect_err("full");
    }
}

#[test]
fn a_heap_map_too_large_for_a_usize_is_refused() {
    assert_eq!(
        HeapMap::<u8, u8>::try_with_capacity(usize::MAX).map(|_| ()),
        Err(ReserveError::TooLarge)
    );
    assert_eq!(slots_for(usize::MAX), None);
    assert_eq!(slots_for(0), Some(1));
    assert_eq!(slots_for(7), Some(8));
    assert_eq!(slots_for(8), Some(16));
}

#[test]
fn a_heap_map_the_allocator_cannot_provide_is_refused_without_panicking() {
    let refused = HeapMap::<[u8; 1024], u8>::try_with_capacity(usize::MAX / 4096)
        .map(|_| ())
        .expect_err("too much to allocate");
    assert!(matches!(
        refused,
        ReserveError::Alloc(_) | ReserveError::TooLarge
    ));
}

#[test]
fn a_zero_sized_hasher_costs_the_map_nothing_and_a_stateful_one_costs_its_state() {
    let plain = size_of::<InlineMap<u32, u32, 8, FixedState>>();
    let seeded = size_of::<InlineMap<u32, u32, 8, Seeded>>();
    assert_eq!(seeded - plain, size_of::<Seeded>());
}

#[test]
fn each_map_may_hash_with_its_own_state() {
    let mut a: InlineMap<u32, u32, 16, Seeded> = InlineMap::with_hasher(Seeded(1));
    let mut b: InlineMap<u32, u32, 16, Seeded> = InlineMap::with_hasher(Seeded(2));
    for key in 0..14 {
        a.insert(key, key).expect("room");
        b.insert(key, key * 2).expect("room");
    }
    for key in 0..14 {
        assert_eq!((a.get(&key), b.get(&key)), (Some(&key), Some(&(key * 2))));
    }
    assert_eq!((*a.hasher(), *b.hasher()), (Seeded(1), Seeded(2)));
    let heap: HeapMap<u32, u32, Seeded> =
        FixedHashMap::try_with_capacity_and_hasher(4, Seeded(3)).expect("allocated");
    assert_eq!(*heap.hasher(), Seeded(3));
}

#[test]
fn a_map_with_one_slot_holds_nothing() {
    let mut map: InlineMap<u32, u32, 1> = InlineMap::new();
    assert_eq!(map.insert(1, 1), Err(Full { key: 1, value: 1 }));
    assert_eq!(map.get(&1), None);
}

#[test]
fn full_and_reserve_errors_say_what_happened() {
    use core::error::Error;
    let full = Full { key: 1, value: 2 };
    assert_eq!(full.to_string(), "the table is full");
    assert!(full.source().is_none());
    assert_eq!(
        ReserveError::TooLarge.to_string(),
        "no table that large can be made"
    );
    assert!(ReserveError::TooLarge.source().is_none());
}
