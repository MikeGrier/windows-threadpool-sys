// Copyright (c) Mike Grier.

//! Errors shared by every queue shape.
//!
//! They live at the crate root rather than inside a shape's module because the
//! shapes must agree on them: a trait cannot unify `push` across shapes if each
//! returns a differently-named error meaning the same thing.

use core::fmt;

/// Why a capacity was rejected at construction.
///
/// Constructing a queue is the one place a caller can get this wrong, so it is
/// reported rather than rounded away. Silently rounding 100 up to 128 would
/// hand back a bound the caller cannot see they got, and a bound is exactly the
/// number a caller chose deliberately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapacityError {
    requested: usize,
    kind: CapacityErrorKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CapacityErrorKind {
    Zero,
    NotPowerOfTwo,
    TooLarge,
}

impl CapacityError {
    pub(crate) fn zero() -> Self {
        Self {
            requested: 0,
            kind: CapacityErrorKind::Zero,
        }
    }

    pub(crate) fn not_power_of_two(requested: usize) -> Self {
        Self {
            requested,
            kind: CapacityErrorKind::NotPowerOfTwo,
        }
    }

    pub(crate) fn too_large(requested: usize) -> Self {
        Self {
            requested,
            kind: CapacityErrorKind::TooLarge,
        }
    }

    /// The capacity that was asked for.
    #[must_use]
    pub fn requested(&self) -> usize {
        self.requested
    }

    /// The largest valid capacity not greater than the request, if there is
    /// one.
    ///
    /// Offered so a caller can correct the call without working out the
    /// arithmetic: a rejected 100 reports 64 here and 128 from
    /// [`Self::next_valid`].
    #[must_use]
    pub fn previous_valid(&self) -> Option<usize> {
        match self.kind {
            CapacityErrorKind::Zero => None,
            CapacityErrorKind::NotPowerOfTwo | CapacityErrorKind::TooLarge => {
                Some(1_usize << (usize::BITS - 1 - self.requested.leading_zeros()))
            }
        }
    }

    /// The smallest valid capacity not less than the request, if there is one.
    #[must_use]
    pub fn next_valid(&self) -> Option<usize> {
        match self.kind {
            CapacityErrorKind::Zero => Some(1),
            CapacityErrorKind::NotPowerOfTwo => self.requested.checked_next_power_of_two(),
            CapacityErrorKind::TooLarge => None,
        }
    }
}

impl fmt::Display for CapacityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            CapacityErrorKind::Zero => {
                write!(f, "a queue capacity of zero can never accept an item")
            }
            CapacityErrorKind::NotPowerOfTwo => {
                let (lo, hi) = (self.previous_valid(), self.next_valid());
                write!(
                    f,
                    "capacity {} is not a power of two; the nearest valid capacities are {:?} and {:?}",
                    self.requested, lo, hi
                )
            }
            CapacityErrorKind::TooLarge => write!(
                f,
                "capacity {} is too large; it must not exceed half of usize::MAX, so that the \
                 difference between the producer and consumer positions stays unambiguous across \
                 wraparound",
                self.requested
            ),
        }
    }
}

impl core::error::Error for CapacityError {}

/// Why a push did not happen, carrying the item back.
///
/// The item is returned rather than dropped, because a queue that swallows what
/// it refuses gives a caller no way to retry, redirect, or account for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushError<T> {
    /// The queue is at capacity.
    ///
    /// **This is the backpressure signal**, not a malfunction. A bounded queue
    /// exists so that a producer outrunning its consumer is told so, rather
    /// than allowed to consume memory until something worse happens.
    Full(T),

    /// Every consumer is gone, so nothing will ever take this item.
    ///
    /// Distinguished from [`Self::Full`] because the responses differ: a full
    /// queue may drain, and a disconnected one never will, so retrying the
    /// first is sensible and retrying the second is a spin.
    Disconnected(T),
}

impl<T> PushError<T> {
    /// Takes the item back out.
    #[must_use]
    pub fn into_inner(self) -> T {
        match self {
            Self::Full(item) | Self::Disconnected(item) => item,
        }
    }

    /// Whether a later attempt could plausibly succeed.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Full(_))
    }
}

impl<T> fmt::Display for PushError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Full(_) => write!(f, "the queue is at capacity"),
            Self::Disconnected(_) => write!(f, "every consumer is gone"),
        }
    }
}

impl<T: fmt::Debug> core::error::Error for PushError<T> {}
