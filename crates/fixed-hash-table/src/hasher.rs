// Copyright (c) 2026 Mike Grier
//! The default hasher (FHT-D-5): zero-sized, so a table pays nothing to hold it, and seedless,
//! since `no_std` has nothing to seed one from.

use core::hash::{BuildHasher, Hasher};

#[cfg(test)]
mod tests;

/// The default [`BuildHasher`]: zero-sized and seedless, so every table built with it hashes a key
/// the same way and holds no state for it.
///
/// Seedless means an adversary who chooses the keys can choose collisions. A table whose keys are
/// the caller's own -- sequence numbers, handles -- loses nothing by it; one whose keys come from
/// outside should be given a keyed hasher instead, `std`'s `RandomState` among them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FixedState;

impl BuildHasher for FixedState {
    type Hasher = FixedHasher;

    fn build_hasher(&self) -> FixedHasher {
        FixedHasher::default()
    }
}

/// The hasher [`FixedState`] builds: each word is rotated into the state and multiplied by the
/// golden ratio's 64-bit fraction, which spreads its bits into the state's high bits, the ones a
/// table indexes by.
#[derive(Clone, Copy, Debug, Default)]
pub struct FixedHasher {
    state: u64,
}

/// `2^64` divided by the golden ratio, odd, so multiplying by it is a bijection.
const MULTIPLIER: u64 = 0x9E37_79B9_7F4A_7C15;

impl FixedHasher {
    fn add(&mut self, word: u64) {
        self.state = (self.state.rotate_left(5) ^ word).wrapping_mul(MULTIPLIER);
    }
}

impl Hasher for FixedHasher {
    fn write(&mut self, bytes: &[u8]) {
        let (words, rest) = bytes.as_chunks::<8>();
        for word in words {
            self.add(u64::from_le_bytes(*word));
        }
        if !rest.is_empty() {
            let mut word = [0u8; 8];
            word[..rest.len()].copy_from_slice(rest);
            self.add(u64::from_le_bytes(word));
        }
    }

    fn write_u8(&mut self, value: u8) {
        self.add(u64::from(value));
    }

    fn write_u16(&mut self, value: u16) {
        self.add(u64::from(value));
    }

    fn write_u32(&mut self, value: u32) {
        self.add(u64::from(value));
    }

    fn write_u64(&mut self, value: u64) {
        self.add(value);
    }

    fn write_usize(&mut self, value: usize) {
        // A `usize` is at most 64 bits on every target Rust supports.
        self.add(value as u64);
    }

    fn finish(&self) -> u64 {
        self.state
    }
}
