// Copyright (c) 2026 Mike Grier
//! Tests for [`Witness`].
//!
//! The two properties that matter pull against each other: the witness must
//! catch a byte nothing accounted for, and it must **not** accuse a byte that
//! was legitimately written. Over-reporting is the same failure as missing
//! one, and a witness that flags every run is a witness nobody reads.

use super::Witness;
use crate::poison;

/// A poisoned region of `len` bytes for `ordinal`.
fn poisoned(seed: u64, ordinal: u64, len: usize) -> Vec<u8> {
    let mut bytes = vec![0_u8; len];
    // SAFETY: `bytes` is valid for writes of exactly `len` bytes.
    unsafe { poison::fill(bytes.as_mut_ptr(), len, seed, ordinal) };
    bytes
}

const SEED: u64 = 0x1234_5678_9ABC_DEF0;
const ORDINAL: u64 = 7;

#[test]
fn an_untouched_region_with_nothing_permitted_verifies_clean() {
    let bytes = poisoned(SEED, ORDINAL, 256);
    let witness = Witness::new(ORDINAL, 256);
    assert_eq!(witness.verify(SEED, &bytes), Ok(()));
}

#[test]
fn a_write_inside_a_permitted_range_is_not_accused() {
    // The false-positive direction. A witness that flags legitimate writes
    // would be worse than none, because it would train a reader to ignore it.
    let mut bytes = poisoned(SEED, ORDINAL, 256);
    bytes[64..128].fill(0xAA);

    let mut witness = Witness::new(ORDINAL, 256);
    witness.permit(64, 64);
    assert_eq!(witness.verify(SEED, &bytes), Ok(()));
}

#[test]
fn a_write_outside_every_permitted_range_is_caught() {
    let mut bytes = poisoned(SEED, ORDINAL, 256);
    bytes[64..128].fill(0xAA);
    // One byte in the gap after the permitted range, which no operation
    // accounted for.
    bytes[200] ^= 0xFF;

    let mut witness = Witness::new(ORDINAL, 256);
    witness.permit(64, 64);

    let breach = witness
        .verify(SEED, &bytes)
        .expect_err("an unaccounted-for byte must be reported");
    assert_eq!(breach.at, 200);
    assert_eq!(breach.found, bytes[200]);
    assert_eq!(breach.expected, poison::byte_at(SEED, ORDINAL, 200));
}

#[test]
fn a_write_just_before_a_permitted_range_is_caught() {
    // Off-by-one at the low edge: the byte immediately below a permitted span
    // is exactly where an offset computed one too small would land.
    let mut bytes = poisoned(SEED, ORDINAL, 256);
    bytes[63] ^= 0xFF;

    let mut witness = Witness::new(ORDINAL, 256);
    witness.permit(64, 64);

    let breach = witness.verify(SEED, &bytes).expect_err("must be reported");
    assert_eq!(breach.at, 63);
}

#[test]
fn a_write_just_after_a_permitted_range_is_caught() {
    // Off-by-one at the high edge, which is where a length one too large
    // lands -- the shape of a short read whose full span was permitted.
    let mut bytes = poisoned(SEED, ORDINAL, 256);
    bytes[128] ^= 0xFF;

    let mut witness = Witness::new(ORDINAL, 256);
    witness.permit(64, 64);

    let breach = witness.verify(SEED, &bytes).expect_err("must be reported");
    assert_eq!(breach.at, 128);
}

#[test]
fn overlapping_permissions_do_not_invent_a_gap_between_themselves() {
    // A slot written twice by overlapping operations must not be accused of a
    // breach in the overlap.
    let mut bytes = poisoned(SEED, ORDINAL, 256);
    bytes[10..100].fill(0xBB);

    let mut witness = Witness::new(ORDINAL, 256);
    witness.permit(10, 60); // 10..70
    witness.permit(50, 50); // 50..100, overlapping
    assert_eq!(witness.verify(SEED, &bytes), Ok(()));
}

#[test]
fn abutting_permissions_leave_no_gap_between_them() {
    // `0..4` and `4..8` touch without overlapping. Treating them as separate
    // would invent a zero-length gap; treating the boundary as unpermitted
    // would accuse byte 4.
    let mut bytes = poisoned(SEED, ORDINAL, 64);
    bytes[0..8].fill(0xCC);

    let mut witness = Witness::new(ORDINAL, 64);
    witness.permit(0, 4);
    witness.permit(4, 4);
    assert_eq!(witness.verify(SEED, &bytes), Ok(()));
    assert_eq!(witness.permitted_bytes(), 8);
}

#[test]
fn permissions_given_out_of_order_still_merge() {
    // Completions arrive in whatever order the ring reports them, so a caller
    // cannot be required to permit in ascending offset order.
    let mut bytes = poisoned(SEED, ORDINAL, 256);
    bytes[0..32].fill(0xDD);
    bytes[128..160].fill(0xEE);

    let mut witness = Witness::new(ORDINAL, 256);
    witness.permit(128, 32);
    witness.permit(0, 32);
    assert_eq!(witness.verify(SEED, &bytes), Ok(()));
    assert_eq!(witness.permitted_bytes(), 64);
}

#[test]
fn a_short_transfer_leaves_the_rest_of_its_span_accountable() {
    // The case the checklist calls out: permitting the *requested* length
    // rather than the transferred one would forgive a write that should never
    // have happened. Permitting only what arrived keeps the remainder guarded.
    let mut bytes = poisoned(SEED, ORDINAL, 256);
    bytes[64..96].fill(0xAA); // 32 bytes actually transferred
    bytes[100] ^= 0xFF; // a stray write inside the *requested* span

    let mut witness = Witness::new(ORDINAL, 256);
    witness.permit(64, 32); // what arrived, not the 128 requested

    let breach = witness
        .verify(SEED, &bytes)
        .expect_err("a write past the transferred count must still be caught");
    assert_eq!(breach.at, 100);
}

#[test]
fn permitting_the_whole_region_accepts_anything() {
    // The degenerate case, verified so that "permitted everything" cannot
    // accidentally still report a breach.
    let mut bytes = poisoned(SEED, ORDINAL, 128);
    bytes.fill(0x5A);

    let mut witness = Witness::new(ORDINAL, 128);
    witness.permit(0, 128);
    assert_eq!(witness.verify(SEED, &bytes), Ok(()));
}

#[test]
fn a_permission_beyond_the_region_is_clamped_rather_than_forgiving_more() {
    // A caller reporting a completion honestly must not be able to panic this,
    // but nor should an over-long permission excuse bytes outside the region.
    let mut witness = Witness::new(ORDINAL, 64);
    witness.permit(32, usize::MAX);
    assert_eq!(
        witness.permitted_bytes(),
        32,
        "a permission must not extend past the region it belongs to"
    );

    let mut bytes = poisoned(SEED, ORDINAL, 64);
    bytes[32..].fill(0x11);
    assert_eq!(witness.verify(SEED, &bytes), Ok(()));
    bytes[0] ^= 0xFF;
    assert!(witness.verify(SEED, &bytes).is_err());
}

#[test]
fn the_wrong_seed_does_not_verify_clean() {
    // Two runs must not read each other's poison as pristine, or a buffer left
    // over from a previous process could pass as untouched.
    let bytes = poisoned(SEED, ORDINAL, 128);
    let witness = Witness::new(ORDINAL, 128);
    assert!(
        witness.verify(SEED ^ 0xFFFF, &bytes).is_err(),
        "poison from another seed must not verify clean"
    );
}
