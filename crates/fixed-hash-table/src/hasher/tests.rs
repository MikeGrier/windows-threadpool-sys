// Copyright (c) 2026 Mike Grier

use super::{FixedHasher, FixedState};
use core::hash::{BuildHasher, Hasher};

/// A value made through `Default`, as a table's `new` makes its hasher.
fn made<T: Default>() -> T {
    T::default()
}

fn hash_of<T: core::hash::Hash>(value: &T) -> u64 {
    FixedState.hash_one(value)
}

#[test]
fn it_is_zero_sized() {
    assert_eq!(size_of::<FixedState>(), 0);
}

#[test]
fn it_hashes_a_key_the_same_way_every_time_and_from_every_instance() {
    for key in [0u64, 1, 42, u64::MAX] {
        assert_eq!(hash_of(&key), hash_of(&key));
        assert_eq!(FixedState.hash_one(key), made::<FixedState>().hash_one(key));
    }
}

#[test]
fn distinct_small_integers_have_distinct_high_bits() {
    // A table indexes by the high bits, so sequential keys must spread there.
    let mut tops: std::vec::Vec<u64> = (0u64..256).map(|key| hash_of(&key) >> 56).collect();
    tops.sort_unstable();
    tops.dedup();
    assert!(tops.len() > 128, "only {} distinct top bytes", tops.len());
}

#[test]
fn a_usize_hashes_as_the_u64_of_the_same_value() {
    let mut a = FixedHasher::default();
    a.write_usize(77);
    let mut b = FixedHasher::default();
    b.write_u64(77);
    assert_eq!(a.finish(), b.finish());
}

#[test]
fn bytes_are_taken_eight_at_a_time_with_a_padded_tail() {
    let mut whole = FixedHasher::default();
    whole.write(&[1, 2, 3, 4, 5, 6, 7, 8, 9]);
    let mut parts = FixedHasher::default();
    parts.write_u64(u64::from_le_bytes([1, 2, 3, 4, 5, 6, 7, 8]));
    parts.write_u64(9);
    assert_eq!(whole.finish(), parts.finish());
}

#[test]
fn different_strings_hash_differently() {
    let words = ["", "a", "b", "ab", "ba", "durable", "ioring"];
    for (i, a) in words.iter().enumerate() {
        for b in &words[i + 1..] {
            assert_ne!(hash_of(a), hash_of(b), "{a:?} and {b:?}");
        }
    }
}

#[test]
fn every_integer_width_reaches_the_state() {
    let mut hasher = FixedHasher::default();
    let start = hasher.finish();
    hasher.write_u8(1);
    let after_u8 = hasher.finish();
    hasher.write_u16(1);
    let after_u16 = hasher.finish();
    hasher.write_u32(1);
    assert_ne!(start, after_u8);
    assert_ne!(after_u8, after_u16);
    assert_ne!(after_u16, hasher.finish());
}
