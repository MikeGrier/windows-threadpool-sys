// Copyright (c) 2026 Mike Grier
//! Tests for the poison pattern.
//!
//! The one that matters most is [`mix_and_unmix_are_exact_inverses`]: the
//! whole reason [`super::identify`] can recover an allocation's ordinal from
//! its bytes is that the mixing function is a bijection with a computable
//! inverse. Multiplicative inverses modulo 2^64 are easy to state and easy to
//! get subtly wrong, so they are checked against the operation rather than
//! trusted.

use super::{
    MIX_A, MIX_A_INV, MIX_B, MIX_B_INV, byte_at, first_mismatch, identify, mix, unmix, word,
};

#[test]
fn the_multiplicative_inverses_are_actually_inverses() {
    // If either constant were wrong, `unmix` would silently return garbage and
    // `identify` would report confident nonsense rather than failing.
    assert_eq!(MIX_A.wrapping_mul(MIX_A_INV), 1);
    assert_eq!(MIX_B.wrapping_mul(MIX_B_INV), 1);
}

#[test]
fn mix_and_unmix_are_exact_inverses() {
    // Spread across the whole range, including the edges where the xor-shift
    // recovery has the least to work with.
    let cases = [
        0_u64,
        1,
        2,
        u64::MAX,
        u64::MAX - 1,
        1 << 63,
        0x0123_4567_89AB_CDEF,
        0xFEDC_BA98_7654_3210,
        0xAAAA_AAAA_AAAA_AAAA,
        0x5555_5555_5555_5555,
    ];
    for value in cases {
        assert_eq!(unmix(mix(value)), value, "round trip failed for {value:#x}");
    }

    // Every single-bit value, which is where an off-by-one in the xor-shift
    // recovery shows up first.
    for bit in 0..64 {
        let value = 1_u64 << bit;
        assert_eq!(unmix(mix(value)), value, "round trip failed for bit {bit}");
    }
}

#[test]
fn mix_is_injective_over_a_dense_range() {
    // A bijection cannot collide. Checking a dense prefix is enough to catch a
    // mixing function that has degenerated into something non-invertible.
    let mut seen = std::collections::HashSet::with_capacity(4096);
    for ordinal in 0..4096_u64 {
        assert!(
            seen.insert(mix(ordinal)),
            "mix collided at ordinal {ordinal}"
        );
    }
}

#[test]
fn identify_recovers_the_ordinal_a_word_was_made_for() {
    // This is what lets a test ask "which allocation did these bytes come
    // from" without having snapshotted them, and is the whole payoff of a
    // tracked pattern over a constant.
    let seed = 0xDEAD_BEEF_1234_5678;
    let total = 10_000;
    for ordinal in [0_u64, 1, 2, 99, 5000, 9999] {
        let w = word(seed, ordinal);
        assert_eq!(
            identify(seed, w, total),
            Some(ordinal),
            "failed to identify ordinal {ordinal}"
        );
    }
}

#[test]
fn identify_rejects_a_word_from_a_different_run() {
    // Two runs with different seeds must not read each other's poison as
    // valid, or a stale buffer from a previous process could be mistaken for
    // pristine memory.
    let word_from_run_a = word(0x1111_1111_1111_1111, 5);
    let rejected = identify(0x2222_2222_2222_2222, word_from_run_a, 100);
    assert_eq!(
        rejected, None,
        "a word from another seed must not identify as a plausible ordinal"
    );
}

#[test]
fn identify_rejects_bytes_that_are_not_poison_at_all() {
    // Real data must not be mistaken for poison. Every u64 is the image of
    // *some* ordinal, so the bound on plausible ordinals is what makes this
    // distinguishable -- and it is the reason `identify` takes the allocation
    // count rather than answering from the word alone.
    let seed = 0xABCD_EF01_2345_6789;
    let not_poison = [
        0_u64,
        u64::from_le_bytes(*b"hello wo"),
        u64::from_le_bytes([0xAB; 8]),
        u64::from_le_bytes([0xDD; 8]),
    ];
    let mut rejected = 0;
    for candidate in not_poison {
        if identify(seed, candidate, 64).is_none() {
            rejected += 1;
        }
    }
    assert_eq!(
        rejected,
        not_poison.len(),
        "ordinary byte patterns must not pass as poison from a 64-allocation run"
    );
}

#[test]
fn different_ordinals_get_different_patterns() {
    // If two allocations shared a pattern, "these bytes are still poison"
    // could be true of the wrong allocation's poison -- which would make a
    // stray cross-buffer write invisible.
    let seed = 7;
    let a = word(seed, 1);
    let b = word(seed, 2);
    assert_ne!(a, b);
}

#[test]
fn the_pattern_repeats_from_the_start_of_the_allocation() {
    // `byte_at` and `fill` have to agree about phase, or a check at a non
    // multiple-of-eight offset would report a mismatch that is not there.
    let seed = 42;
    let ordinal = 3;
    let expected = word(seed, ordinal).to_le_bytes();
    for offset in 0..32_usize {
        assert_eq!(
            byte_at(seed, ordinal, offset),
            expected[offset % 8],
            "phase disagreement at offset {offset}"
        );
    }
}

#[test]
fn fill_writes_the_pattern_including_a_ragged_tail() {
    // A length that is not a multiple of eight is the common case for a real
    // allocation, and dropping the tail would leave uninstrumented bytes at
    // the end of every one of them.
    let seed = 0x5EED;
    let ordinal = 11;
    for len in [1_usize, 7, 8, 9, 15, 16, 100] {
        let mut buffer = vec![0_u8; len];
        // SAFETY: `buffer` is valid for writes of exactly `len` bytes.
        unsafe { super::fill(buffer.as_mut_ptr(), len, seed, ordinal) };
        assert_eq!(
            first_mismatch(seed, ordinal, 0, &buffer),
            None,
            "fill left a gap for len {len}"
        );
    }
}

#[test]
fn first_mismatch_finds_the_exact_byte_that_changed() {
    // The diagnostic value: not merely "something wrote here" but "the write
    // started at this offset", which is what makes an out-of-span kernel write
    // attributable.
    let seed = 0xC0FFEE;
    let ordinal = 4;
    let mut buffer = vec![0_u8; 64];
    // SAFETY: valid for writes of 64 bytes.
    unsafe { super::fill(buffer.as_mut_ptr(), 64, seed, ordinal) };
    assert_eq!(first_mismatch(seed, ordinal, 0, &buffer), None);

    buffer[37] ^= 0xFF;
    assert_eq!(first_mismatch(seed, ordinal, 0, &buffer), Some(37));
}

#[test]
fn first_mismatch_honours_a_non_zero_offset() {
    // Checking "the bytes after the span are untouched" means checking a slice
    // that starts partway into the allocation, so the offset has to shift the
    // expected phase rather than restarting it.
    let seed = 0xFACE;
    let ordinal = 6;
    let mut buffer = vec![0_u8; 64];
    // SAFETY: valid for writes of 64 bytes.
    unsafe { super::fill(buffer.as_mut_ptr(), 64, seed, ordinal) };

    let tail = &buffer[19..];
    assert_eq!(
        first_mismatch(seed, ordinal, 19, tail),
        None,
        "a correctly-phased tail must verify clean"
    );
    assert!(
        first_mismatch(seed, ordinal, 0, tail).is_some(),
        "the wrong phase must not verify clean, or the offset argument does nothing"
    );
}
