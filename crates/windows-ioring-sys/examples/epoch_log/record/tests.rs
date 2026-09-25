// Copyright (c) 2026 Mike Grier
//! Tests for the record format's digest (M25.7).
//!
//! # Why this exists
//!
//! `compare_strategies` asserted that all three strategies write
//! **byte-identical** logs by keeping one strategy's whole log in memory and
//! comparing the next against it. `M25.7` replaced that with a digest, which
//! retains thirty-two bits instead of eight megabytes -- and which is a
//! **weaker** check, because two different logs can in principle share a
//! digest where two different byte arrays cannot share their bytes.
//!
//! So the weakening needs a guard. These pin that the digest separates the
//! differences the assertion exists to catch: a flipped byte anywhere, and a
//! log with a different number of records in it.
//!
//! # What they cannot establish
//!
//! **That no two logs collide.** FNV-1a over 32 bits has collisions and
//! finding one is not hard for someone trying. The digest's job here is to
//! compare files this program wrote itself moments earlier, where the
//! difference being looked for is a dropped record or a wrong offset rather
//! than an adversary's construction -- which is stated at the definition too.

use super::{RECORD_STRIDE, digest};

#[test]
fn identical_bytes_digest_identically() {
    let log = vec![0xAB_u8; RECORD_STRIDE * 3];
    let copy = log.clone();
    assert_eq!(
        digest(&log),
        digest(&copy),
        "the same bytes must always give the same digest, or the comparison \
         this backs would fail on logs that agree"
    );
}

#[test]
fn a_single_flipped_byte_changes_the_digest() {
    let log = vec![0xAB_u8; RECORD_STRIDE * 3];
    let baseline = digest(&log);

    // First, last, and a block boundary: the positions a walk over strides is
    // most likely to treat differently from the bytes around them.
    for victim in [0, 1, RECORD_STRIDE - 1, RECORD_STRIDE, log.len() - 1] {
        let mut damaged = log.clone();
        damaged[victim] ^= 0xFF;
        assert_ne!(
            digest(&damaged),
            baseline,
            "flipping byte {victim} must change the digest, or a log that \
             differs there would compare equal"
        );
    }
}

#[test]
fn a_log_with_fewer_records_digests_differently() {
    let log = vec![0xAB_u8; RECORD_STRIDE * 3];
    let short = vec![0xAB_u8; RECORD_STRIDE * 2];
    assert_ne!(
        digest(&log),
        digest(&short),
        "a dropped record must change the digest -- that is the failure the \
         cross-strategy assertion exists to catch"
    );
}

#[test]
fn a_trailing_zero_block_is_not_invisible() {
    // The case a length-oblivious hash would miss: one log ends after its
    // records, another has a zeroed block after them. Since M25.3 the log is
    // pre-allocated with slack, so a strategy that wrote one record fewer
    // leaves exactly this shape rather than a shorter file.
    let log = vec![0xAB_u8; RECORD_STRIDE * 2];
    let mut padded = log.clone();
    padded.extend(std::iter::repeat_n(0_u8, RECORD_STRIDE));
    assert_ne!(
        digest(&log),
        digest(&padded),
        "appending a zeroed block must change the digest"
    );
}
