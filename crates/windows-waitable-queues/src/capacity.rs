// Copyright (c) Mike Grier.

//! The capacity rule, stated once for every bounded shape.
//!
//! It lives here rather than inside a shape's module because every bounded
//! shape enforces the same rule for the same reason, and a second copy of a
//! rule is free to drift from the first. A test that wants to check a suggested
//! capacity asks [`validate_capacity`] rather than re-encoding the conditions,
//! which is the difference between checking the rule and checking a paraphrase
//! of it.
//!
//! # The bounds belong to the shape, not to the crate
//!
//! [`CapacityError`] carries both bounds rather than assuming crate-wide
//! constants, because both follow from how a shape represents its positions --
//! and the shipped shapes disagree about both.
//!
//! - **The minimum.** `spsc` accepts a capacity of one; `slotwise_mpsc` cannot, because
//!   its slot state machine encodes "published" as one past the claim position
//!   and "free again" as one lap past it, and with a single slot those are the
//!   same number.
//! - **The maximum.** Most shapes stop at [`WRAPPING_MAX_CAPACITY`], where a
//!   wrapping difference between two positions stops being unambiguous.
//!   `reserving_mpsc` stops far lower, because it packs its reservation count
//!   into the same word as its position so that the two can be claimed
//!   together.
//!
//! So a shape supplies its own [`Bounds`] and this module applies them, which
//! is the arrangement the error type was already shaped for.

use crate::error::CapacityError;

/// The largest capacity that keeps a wrapping position difference unambiguous.
///
/// Positions are monotonic and wrap with the integer, so a shape needs the
/// difference between two of them to be readable as a signed quantity:
///
/// - `spsc` computes the number of items held as `tail.wrapping_sub(head)`,
///   which is the true difference only while that difference cannot exceed half
///   the range.
/// - `slotwise_mpsc` compares a slot's sequence number against a position by
///   interpreting `sequence.wrapping_sub(position)` as an [`isize`], which is
///   the same requirement written a different way.
///
/// A shape whose positions are narrower than a [`usize`] has a correspondingly
/// smaller bound, and says so in its own [`Bounds`]; this is the widest any
/// shape may be.
pub(crate) const WRAPPING_MAX_CAPACITY: usize = usize::MAX / 2;

/// What one shape will accept as a capacity.
///
/// A named pair rather than two loose arguments, so neither a call site nor a
/// test can silently transpose them, and so a shape's answer to "how small" and
/// "how large" is written in one place with the reasoning beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Bounds {
    /// The smallest capacity this shape can represent.
    pub(crate) min: usize,
    /// The largest capacity this shape can represent.
    pub(crate) max: usize,
}

/// Whether a bounded shape will accept a capacity, and why not if it will not.
///
/// A power of two is required so a position can be reduced to a slot index with
/// a mask rather than a division, and the requested number is the exact number
/// of items the queue holds -- not a hint, and not rounded. See
/// [`CapacityError`] for why a rejection is preferred to silently rounding.
///
/// Separated from each shape's constructor so the rule can be *asked* rather
/// than restated. A test that wants to check a suggested capacity is acceptable
/// would otherwise have to either re-encode these conditions -- a second copy
/// of a rule, free to drift from this one -- or call the constructor, which for
/// a capacity near the bound means trying to allocate half the address space.
pub(crate) fn validate_capacity(capacity: usize, bounds: Bounds) -> Result<(), CapacityError> {
    debug_assert!(
        bounds.min.is_power_of_two(),
        "a shape's minimum is suggested to callers verbatim, so it must itself be valid"
    );
    debug_assert!(
        bounds.max <= WRAPPING_MAX_CAPACITY,
        "no shape may exceed the width at which a wrapping position difference is unambiguous"
    );
    debug_assert!(
        bounds.min <= bounds.max,
        "a shape that accepts nothing at all would reject every capacity with a suggestion it \
         would also reject"
    );

    if capacity == 0 {
        return Err(CapacityError::zero(bounds));
    }
    if !capacity.is_power_of_two() {
        return Err(CapacityError::not_power_of_two(capacity, bounds));
    }
    if capacity < bounds.min {
        return Err(CapacityError::too_small(capacity, bounds));
    }
    if capacity > bounds.max {
        return Err(CapacityError::too_large(capacity, bounds));
    }
    Ok(())
}
