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

#[test]
fn the_wrapping_ceiling_is_one_below_a_power_of_two() {
    // The fact every other assertion here rests on, stated so a reader does not
    // have to do the arithmetic: `usize::MAX / 2` is odd, so it is not itself a
    // capacity any shape accepts.
    assert_eq!(WRAPPING_MAX_CAPACITY, (1_usize << 63) - 1);
    assert!(!WRAPPING_MAX_CAPACITY.is_power_of_two());
}

#[test]
fn the_largest_accepted_capacity_is_two_to_the_sixty_two() {
    // The documented number. `2^62` fits under `usize::MAX / 2`; `2^63` is one
    // larger than the bound and is refused, which is what four documents used
    // to claim was reachable.
    validate_capacity(1_usize << 62, WIDEST).expect("2^62 is within the wrapping bound");

    validate_capacity(1_usize << 63, WIDEST)
        .expect_err("2^63 exceeds the wrapping bound and must be refused");
}

#[test]
fn every_power_of_two_up_to_the_ceiling_is_accepted() {
    // A property rather than the two boundary samples above, so a bound that
    // moved for some other reason cannot pass by coincidence.
    for shift in 0..63_u32 {
        let capacity = 1_usize << shift;
        assert!(
            validate_capacity(capacity, WIDEST).is_ok(),
            "2^{shift} should be accepted"
        );
    }
    for shift in 63..usize::BITS {
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
    for capacity in [3_usize, 6, 100, (1 << 62) - 1, (1 << 62) + 1] {
        assert!(
            validate_capacity(capacity, WIDEST).is_err(),
            "{capacity} is not a power of two and must be refused"
        );
    }
}
