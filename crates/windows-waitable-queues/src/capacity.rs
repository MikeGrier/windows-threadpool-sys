// Copyright (c) Mike Grier.

//! The capacity rule, stated once for every bounded shape.
//!
//! It lives here rather than inside a shape's module because both bounded
//! shapes enforce the same rule for the same reason, and a second copy of a
//! rule is free to drift from the first. A test that wants to check a suggested
//! capacity asks [`validate_capacity`] rather than re-encoding the three
//! conditions, which is the difference between checking the rule and checking a
//! paraphrase of it.
//!
//! The bounds are carried on [`CapacityError`] rather than assumed to be
//! crate-wide constants, because they follow from how a shape represents its
//! positions -- and the two shapes shipped so far already disagree about the
//! lower one. `spsc` accepts a capacity of one; `mpsc` cannot, because its slot
//! state machine encodes "published" as one past the claim position and "free
//! again" as one lap past it, and with a single slot those are the same number.
//! So each shape supplies its own minimum and this module applies it, which is
//! the arrangement the error type was already shaped for.

use crate::error::CapacityError;

/// The largest capacity that keeps a wrapping position difference unambiguous.
///
/// Positions are monotonic and wrap with the integer, so both shapes need the
/// difference between two of them to be readable as a signed quantity:
///
/// - `spsc` computes the number of items held as `tail.wrapping_sub(head)`,
///   which is the true difference only while that difference cannot exceed half
///   the range.
/// - `mpsc` compares a slot's sequence number against a position by
///   interpreting `sequence.wrapping_sub(position)` as an [`isize`], which is
///   the same requirement written a different way.
pub(crate) const MAX_CAPACITY: usize = usize::MAX / 2;

/// Whether a bounded shape will accept a capacity, and why not if it will not.
///
/// A power of two is required so a position can be reduced to a slot index with
/// a mask rather than a division, and the requested number is the exact number
/// of items the queue holds -- not a hint, and not rounded. See
/// [`CapacityError`] for why a rejection is preferred to silently rounding.
///
/// `min_valid` is the calling shape's own smallest usable capacity. It is a
/// parameter rather than a constant because it is a property of the shape's
/// slot representation, and the two shapes do not agree on it. Each shape names
/// its own and says why, so the number is never a bare literal at a call site.
///
/// Separated from each shape's constructor so the rule can be *asked* rather
/// than restated. A test that wants to check a suggested capacity is acceptable
/// would otherwise have to either re-encode these conditions -- a second copy
/// of a rule, free to drift from this one -- or call the constructor, which for
/// a capacity near the bound means trying to allocate half the address space.
pub(crate) fn validate_capacity(capacity: usize, min_valid: usize) -> Result<(), CapacityError> {
    debug_assert!(
        min_valid.is_power_of_two(),
        "a shape's minimum is suggested to callers verbatim, so it must itself be valid"
    );
    if capacity == 0 {
        return Err(CapacityError::zero(min_valid, MAX_CAPACITY));
    }
    if !capacity.is_power_of_two() {
        return Err(CapacityError::not_power_of_two(
            capacity,
            min_valid,
            MAX_CAPACITY,
        ));
    }
    if capacity < min_valid {
        return Err(CapacityError::too_small(capacity, min_valid, MAX_CAPACITY));
    }
    if capacity > MAX_CAPACITY {
        return Err(CapacityError::too_large(capacity, min_valid, MAX_CAPACITY));
    }
    Ok(())
}
