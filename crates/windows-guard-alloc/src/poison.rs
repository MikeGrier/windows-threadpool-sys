// Copyright (c) 2026 Mike Grier
//! The tracked poison pattern: what fills a fresh allocation, and how to tell
//! later whether anything overwrote it.
//!
//! # Why the poison is tracked rather than a constant
//!
//! A fixed byte like `0xDD` answers "are these bytes real data?" and nothing
//! else. It collides with real payloads -- a buffer legitimately full of
//! `0xDD` is indistinguishable from an untouched one -- and when a stray write
//! *is* found it cannot say where the surviving bytes came from.
//!
//! The pattern here is derived from a per-run **seed** and a per-allocation
//! **ordinal**, so the bytes name their own allocation. [`identify`] recovers
//! the ordinal from any 8 poison bytes, which is what lets a test ask "is this
//! region still the poison it started as?" without having snapshotted it
//! first.
//!
//! # Why it is reproducible
//!
//! Randomised test data would ordinarily be forbidden here: this component
//! requires tests to be reproducible. The seed is what reconciles the two. It
//! is printed by [`crate::GuardAlloc::announce_seed`] and read back from
//! `WINDOWS_GUARD_ALLOC_SEED`, so a failure replays byte-for-byte rather than
//! being a one-off that never recurs. A run with a fixed seed is as
//! deterministic as a constant would be; what varies is *which* deterministic
//! pattern, which is what stops code from accidentally depending on the
//! pattern's value.
//!
//! # Why the mixing function has to be a bijection
//!
//! [`identify`] inverts it. `splitmix64` is a bijection on `u64`, so every
//! ordinal maps to a distinct word and the mapping can be run backwards
//! exactly -- no scanning, no guessing, and no possibility of two allocations
//! sharing a pattern.

/// `splitmix64`'s increment, the fractional part of the golden ratio.
const GOLDEN: u64 = 0x9E37_79B9_7F4A_7C15;
const MIX_A: u64 = 0xBF58_476D_1CE4_E5B9;
const MIX_B: u64 = 0x94D0_49BB_1331_11EB;

/// The environment variable that pins the seed for a reproducible run.
pub const SEED_VAR: &str = "WINDOWS_GUARD_ALLOC_SEED";

/// `splitmix64`'s finalising mix: a bijection on `u64`.
const fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(GOLDEN);
    z = (z ^ (z >> 30)).wrapping_mul(MIX_A);
    z = (z ^ (z >> 27)).wrapping_mul(MIX_B);
    z ^ (z >> 31)
}

/// The multiplicative inverse of an odd `x` modulo 2^64.
///
/// Newton-Raphson on `f(i) = 1/i - x`: each round doubles the number of
/// correct low bits, starting from three, so five rounds cover 64 bits.
///
/// Computed rather than written down. A hardcoded inverse is exactly the kind
/// of constant that looks authoritative and is wrong -- the first draft of
/// this file had two of them, both fabricated, and
/// `the_multiplicative_inverses_are_actually_inverses` is what caught it.
/// Deriving them makes that class of error unrepresentable.
const fn mul_inverse(x: u64) -> u64 {
    assert!(x % 2 == 1, "only odd values are invertible modulo 2^64");
    let mut inverse = x; // correct to three bits
    let mut round = 0;
    // Five rounds, and a mutation run reports `<= 5` as surviving because it is
    // an equivalent mutant rather than a gap. Newton's iteration doubles the
    // correct bits each round -- 3, 6, 12, 24, 48, 96 -- so five is the first
    // count that covers all 64, and the step is idempotent once exact:
    // `inverse * (2 - x * inverse)` is `inverse * (2 - 1)`. A sixth round
    // returns the same value, so no test can distinguish the two bounds.
    //
    // Four rounds would *not* be equivalent, and
    // `the_multiplicative_inverses_are_actually_inverses` is what catches that
    // direction.
    while round < 5 {
        inverse = inverse.wrapping_mul(2_u64.wrapping_sub(x.wrapping_mul(inverse)));
        round += 1;
    }
    inverse
}

/// The multiplicative inverse of [`MIX_A`] modulo 2^64.
const MIX_A_INV: u64 = mul_inverse(MIX_A);
/// The multiplicative inverse of [`MIX_B`] modulo 2^64.
const MIX_B_INV: u64 = mul_inverse(MIX_B);

/// Undo an `x ^= x >> shift` step.
///
/// The top `shift` bits survive the operation untouched, so they are already
/// correct in `y`. Each further round xors in the previous estimate shifted
/// down, recovering another `shift` bits from the top, and after `64 / shift`
/// rounds the whole word is exact.
const fn unxor_shift_right(y: u64, shift: u32) -> u64 {
    let mut recovered = y;
    let mut resolved = shift;
    // `<= 64` survives a mutation run for the same reason `mul_inverse`'s bound
    // does: the step is idempotent once exact. With `recovered == x`, another
    // round computes `y ^ (x >> shift)`, which is `x ^ (x >> shift) ^
    // (x >> shift)` -- that is, `x` again. An extra round cannot change the
    // answer, so the two bounds are indistinguishable.
    while resolved < 64 {
        recovered = y ^ (recovered >> shift);
        resolved += shift;
    }
    recovered
}

/// Invert [`mix`].
const fn unmix(mut z: u64) -> u64 {
    z = unxor_shift_right(z, 31);
    z = z.wrapping_mul(MIX_B_INV);
    z = unxor_shift_right(z, 27);
    z = z.wrapping_mul(MIX_A_INV);
    z = unxor_shift_right(z, 30);
    z.wrapping_sub(GOLDEN)
}

/// The 8-byte pattern that fills the `ordinal`-th allocation of the run
/// identified by `seed`.
#[must_use]
pub const fn word(seed: u64, ordinal: u64) -> u64 {
    mix(seed ^ ordinal)
}

/// Recover the allocation ordinal from a poison word.
///
/// Returns `None` only when `seed` is wrong for the run those bytes came from,
/// since every `u64` is the image of exactly one ordinal -- so a "successful"
/// identification of an implausibly large ordinal means the bytes were not
/// poison at all. The bound below is what makes that distinguishable.
#[must_use]
pub fn identify(seed: u64, word: u64, allocations_so_far: u64) -> Option<u64> {
    let ordinal = unmix(word) ^ seed;
    (ordinal < allocations_so_far.max(1)).then_some(ordinal)
}

/// The poison byte `offset` bytes into the `ordinal`-th allocation.
///
/// The pattern is written from the allocation's start, so the byte at an
/// arbitrary offset is the word's `offset % 8`-th byte rather than always its
/// first.
#[must_use]
pub const fn byte_at(seed: u64, ordinal: u64, offset: usize) -> u8 {
    word(seed, ordinal).to_le_bytes()[offset % 8]
}

/// Fill `len` bytes at `ptr` with the poison for `ordinal`.
///
/// # Safety
///
/// `ptr` must be valid for writes of `len` bytes.
pub unsafe fn fill(ptr: *mut u8, len: usize, seed: u64, ordinal: u64) {
    let pattern = word(seed, ordinal).to_le_bytes();
    let whole = len / 8;
    for i in 0..whole {
        // SAFETY: `i * 8 + 8 <= len`, so this stays inside the caller's region.
        unsafe { std::ptr::copy_nonoverlapping(pattern.as_ptr(), ptr.add(i * 8), 8) };
    }
    let tail = len % 8;
    if tail != 0 {
        // SAFETY: `whole * 8 + tail == len`, so this writes the remainder and
        // no more.
        unsafe { std::ptr::copy_nonoverlapping(pattern.as_ptr(), ptr.add(whole * 8), tail) };
    }
}

/// Where `bytes` first departs from the poison for `ordinal`, given that
/// `bytes` starts `offset` bytes into that allocation.
///
/// `None` means the whole region is still pristine.
#[must_use]
pub fn first_mismatch(seed: u64, ordinal: u64, offset: usize, bytes: &[u8]) -> Option<usize> {
    bytes
        .iter()
        .enumerate()
        .find(|(i, actual)| **actual != byte_at(seed, ordinal, offset + i))
        .map(|(i, _)| i)
}

#[cfg(test)]
mod tests;
