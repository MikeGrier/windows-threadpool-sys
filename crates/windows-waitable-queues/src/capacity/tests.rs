// Copyright (c) Mike Grier.

//! Tests for the capacity bounds themselves.
//!
//! These exist because the ceiling was **documented wrongly in four places at
//! once**: the crate docs, the README and two design-note sections all said the
//! widest shape reaches `2^63` slots. It does not, and `error.rs` said so
//! correctly in the same crate the whole time -- "the nearest power of two
//! below `usize::MAX` is 2^63, which exceeds the largest representable
//! capacity". Prose cannot notice that it disagrees with prose; a test can.
//!
//! No queue is constructed here. Validation is a pure function of the request
//! and the bounds, so the ceiling can be checked without asking an allocator
//! for exabytes.

use super::{Bounds, WRAPPING_MAX_CAPACITY, validate_capacity};

/// The bounds of a shape that stops where wrapping positions stop being
/// unambiguous, which is the widest any shape in this crate goes.
const WIDEST: Bounds = Bounds {
    min: 1,
    max: WRAPPING_MAX_CAPACITY,
};

/// The largest power-of-two capacity the wrapping bound admits, as a shift.
///
/// **Derived from `usize::BITS`, not written as 62.** The bound is
/// `usize::MAX / 2`, which is `2^(BITS-1) - 1`, so the largest power of two
/// under it is `2^(BITS-2)`. Hard-coding the 64-bit answer made these tests
/// unbuildable on a 32-bit target -- `1_usize << 63` does not fit in a 32-bit
/// `usize` -- a strange way for a test *about* `usize` bounds to fail.
const LARGEST_ACCEPTED_SHIFT: u32 = usize::BITS - 2;

/// One past it: the smallest power of two the bound refuses.
const SMALLEST_REFUSED_SHIFT: u32 = usize::BITS - 1;

#[test]
fn the_wrapping_ceiling_is_one_below_a_power_of_two() {
    // The fact every other assertion here rests on, stated so a reader does not
    // have to do the arithmetic: `usize::MAX / 2` is odd, so it is not itself a
    // capacity any shape accepts.
    assert_eq!(
        WRAPPING_MAX_CAPACITY,
        (1_usize << SMALLEST_REFUSED_SHIFT) - 1
    );
    assert!(!WRAPPING_MAX_CAPACITY.is_power_of_two());
}

#[test]
fn the_largest_accepted_capacity_is_two_below_the_word_size() {
    // On 64-bit that is 2^62 accepted and 2^63 refused, which is what four
    // documents used to claim was the other way round. Expressed as shifts so
    // the same assertion holds on a narrower word.
    validate_capacity(1_usize << LARGEST_ACCEPTED_SHIFT, WIDEST)
        .expect("the largest power of two under the bound is within it");

    validate_capacity(1_usize << SMALLEST_REFUSED_SHIFT, WIDEST)
        .expect_err("one power of two past the bound must be refused");
}

#[test]
fn every_power_of_two_up_to_the_ceiling_is_accepted() {
    // A property rather than the two boundary samples above, so a bound that
    // moved for some other reason cannot pass by coincidence.
    for shift in 0..=LARGEST_ACCEPTED_SHIFT {
        let capacity = 1_usize << shift;
        assert!(
            validate_capacity(capacity, WIDEST).is_ok(),
            "2^{shift} should be accepted"
        );
    }
    for shift in SMALLEST_REFUSED_SHIFT..usize::BITS {
        let capacity = 1_usize << shift;
        assert!(
            validate_capacity(capacity, WIDEST).is_err(),
            "2^{shift} should be refused"
        );
    }
}

#[test]
fn a_capacity_that_is_not_a_power_of_two_is_refused_whatever_its_size() {
    // Guards the other half of the rule, so a fix to the ceiling cannot be made
    // by loosening the shape of what is accepted.
    let largest = 1_usize << LARGEST_ACCEPTED_SHIFT;
    for capacity in [3_usize, 6, 100, largest - 1, largest + 1] {
        assert!(
            validate_capacity(capacity, WIDEST).is_err(),
            "{capacity} is not a power of two and must be refused"
        );
    }
}

#[test]
fn a_capacity_exactly_at_the_ceiling_is_accepted() {
    // **The boundary the other tests here cannot reach.** They all use
    // `WIDEST`, whose `max` is `usize::MAX / 2` -- not a power of two, so the
    // power-of-two rule refuses every capacity near it and the `>` in
    // `validate_capacity` is never asked about equality. Widening `>` to `>=`
    // therefore changed nothing observable, and a mutation run found that
    // comparison unguarded.
    //
    // With a ceiling that is itself a legal capacity, the two differ: the
    // largest capacity a shape offers must be constructible, and off by one
    // here would refuse it.
    let bounds = Bounds { min: 2, max: 8 };

    validate_capacity(8, bounds).expect("the ceiling itself must be accepted");
    validate_capacity(16, bounds).expect_err("one power of two above it must not be");

    // The same at the other end, so the floor is not off by one either.
    validate_capacity(2, bounds).expect("the floor itself must be accepted");
    validate_capacity(1, bounds).expect_err("below the floor must not be");
}
