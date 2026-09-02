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

use super::{Bounds, MAX_ADMISSIBLE_CAPACITY, WRAPPING_MAX_CAPACITY, validate_capacity};

/// The bounds of the widest shape in this crate.
///
/// **`MAX_ADMISSIBLE_CAPACITY`, not `WRAPPING_MAX_CAPACITY`, and that is a
/// correction.** This fixture used to carry the wrapping bound, which is
/// `usize::MAX / 2` -- odd, and so not a capacity `validate_capacity` accepts.
/// The tests below already knew that (see
/// `the_wrapping_ceiling_is_one_below_a_power_of_two`, which asserts exactly
/// it), so the fixture was encoding a `Bounds` no real shape should ever have
/// held -- and two shapes did hold it, reporting through
/// `CapacityError::max_valid` a ceiling they would themselves refuse.
const WIDEST: Bounds = Bounds {
    min: 1,
    max: MAX_ADMISSIBLE_CAPACITY,
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
    // **A small explicit ceiling, kept after `WIDEST` was corrected.** This was
    // originally the only test here that could reach the equality boundary at
    // all: `WIDEST` carried `usize::MAX / 2`, which is not a power of two, so
    // the power-of-two rule refused every capacity near it and the `>` in
    // `validate_capacity` was never asked about equality -- widening it to `>=`
    // changed nothing observable, and a mutation run found the comparison
    // unguarded.
    //
    // `WIDEST` now carries a ceiling that *is* a legal capacity, so it reaches
    // the boundary too. This stays because a bound of 8 states the property
    // without depending on the word size, and because the two tests fail for
    // different reasons if the rule breaks.
    let bounds = Bounds { min: 2, max: 8 };

    validate_capacity(8, bounds).expect("the ceiling itself must be accepted");
    validate_capacity(16, bounds).expect_err("one power of two above it must not be");

    // The same at the other end, so the floor is not off by one either.
    validate_capacity(2, bounds).expect("the floor itself must be accepted");
    validate_capacity(1, bounds).expect_err("below the floor must not be");
}

#[test]
fn the_ceiling_a_refusal_reports_is_itself_a_capacity_that_would_be_accepted() {
    // The contract `CapacityError::max_valid` states -- "the largest capacity
    // the shape that rejected this request will accept" -- was false for two
    // shapes, which reported `usize::MAX / 2`. A caller correcting a refusal by
    // using it would have been refused again, with the same suggestion.
    //
    // Asserted as a round trip rather than against a literal, so it holds for
    // whatever bound each shape declares: take the ceiling out of a real
    // refusal and feed it straight back.
    let too_large = validate_capacity(MAX_ADMISSIBLE_CAPACITY * 2, WIDEST)
        .expect_err("one past the ceiling must be refused");

    validate_capacity(too_large.max_valid(), WIDEST).expect(
        "the ceiling a refusal suggests must be one the same bounds accept, or the \
         suggestion sends a caller straight back into the error they just had",
    );
    assert_eq!(too_large.max_valid(), MAX_ADMISSIBLE_CAPACITY);

    // The same for the other end, which was already correct -- included so the
    // pair is stated together and a later edit cannot break one while the other
    // still passes.
    let too_small = validate_capacity(0, Bounds { min: 4, max: 64 })
        .expect_err("zero is refused whatever the bounds");
    validate_capacity(too_small.min_valid(), Bounds { min: 4, max: 64 })
        .expect("the floor a refusal suggests must also be acceptable");
}

#[test]
fn the_admissible_ceiling_is_the_largest_power_of_two_the_wrapping_bound_allows() {
    // Only the relationships that are *not* already compile-time facts. That
    // the ceiling is a power of two and sits inside the wrapping bound is
    // asserted in `capacity.rs`'s `const` block, which is the stronger place --
    // it fails the build rather than a run somebody chose to make -- and clippy
    // rightly rejects restating them here as constant-valued assertions.
    //
    // What is left is the tie between the ceiling and the shifts these tests
    // reason in, so a bound moved for some other reason cannot pass by
    // coincidence.
    assert_eq!(MAX_ADMISSIBLE_CAPACITY, 1_usize << LARGEST_ACCEPTED_SHIFT);
    assert_eq!(
        WRAPPING_MAX_CAPACITY,
        (1_usize << SMALLEST_REFUSED_SHIFT) - 1,
        "the next power of two up is one past the wrapping bound, which is what \
         makes the admissible ceiling the largest one that fits"
    );
}

#[test]
fn the_shapes_ceilings_are_what_the_public_documentation_claims() {
    // The crate docs and the README compare the shapes by capacity, and that
    // comparison is target-dependent -- which they did not say until a review
    // round pointed it out. Asserted here so the claim is checked on whatever
    // target the suite runs on rather than believed from a 64-bit reading.
    //
    // Written while correcting exactly that: a first draft of the corrected
    // prose said `reserving_mpsc` keeps its 2^31 packed ceiling on a 32-bit
    // target. It does not -- the clamp applies to it too -- and this assertion
    // is what caught it.
    // Asked through the public surface rather than by reaching for each
    // shape's private `BOUNDS`: what the documentation describes is what a
    // caller can observe, and a caller observes the ceiling by being refused.
    // A capacity of 3 is refused by every shape for a reason that has nothing
    // to do with the ceiling, so the error it carries reports the real one.
    let spsc_ceiling = crate::spsc::bounded::<u8>(3)
        .expect_err("3 is not a power of two")
        .max_valid();
    let slotwise_ceiling = crate::slotwise_mpsc::bounded::<u8>(3)
        .expect_err("3 is not a power of two")
        .max_valid();

    assert_eq!(
        spsc_ceiling, MAX_ADMISSIBLE_CAPACITY,
        "spsc's positions are full-width, so it goes as wide as any shape may"
    );
    assert_eq!(
        slotwise_ceiling, MAX_ADMISSIBLE_CAPACITY,
        "slotwise_mpsc is bounded by allocation rather than by its own positions"
    );

    // `reserving_mpsc` packs a 32-bit position beside a reservation count, so
    // its own ceiling is 2^31 -- but it is *also* clamped, and on a 32-bit
    // target the clamp is the binding constraint.
    let packed = 1_usize << 31;
    let expected = if packed <= MAX_ADMISSIBLE_CAPACITY {
        packed
    } else {
        MAX_ADMISSIBLE_CAPACITY
    };
    assert_eq!(crate::reserving_mpsc::BOUNDS_MAX, expected);

    #[cfg(target_pointer_width = "64")]
    {
        assert_eq!(MAX_ADMISSIBLE_CAPACITY, 1_usize << 62);
        assert_eq!(crate::reserving_mpsc::BOUNDS_MAX, 1_usize << 31);
    }
    #[cfg(target_pointer_width = "32")]
    {
        assert_eq!(MAX_ADMISSIBLE_CAPACITY, 1_usize << 30);
        assert_eq!(
            crate::reserving_mpsc::BOUNDS_MAX,
            1_usize << 30,
            "the clamp binds here, so both shapes land on the same ceiling"
        );
    }
}
