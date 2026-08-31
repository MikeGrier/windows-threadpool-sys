// Copyright (c) Mike Grier.

//! Errors shared by every queue shape.
//!
//! They live at the crate root rather than inside a shape's module because the
//! shapes must agree on them: a trait cannot unify `push` across shapes if each
//! returns a differently-named error meaning the same thing.

use core::fmt;
use std::io;

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

/// Why a blocking receive gave up.
///
/// There is no `Empty` variant, because a blocking receive does not return on
/// an empty queue -- it waits. Emptiness is only ever terminal when the
/// producer is gone as well, and that is [`RecvError::Disconnected`].
#[derive(Debug)]
#[non_exhaustive]
pub enum RecvError {
    /// Every producer has been dropped and the queue has been drained.
    ///
    /// Reported only after the queue is genuinely empty, never merely because
    /// the producer went away: a producer may push and then drop, and those
    /// items are still owed to the consumer.
    Disconnected,
    /// A Windows call failed while creating or waiting on the doorbell.
    ///
    /// Kept distinct from [`RecvError::Disconnected`] because the two demand
    /// opposite reactions: disconnection is the orderly end of a stream, while
    /// this means the wait itself is broken and retrying will not help.
    Io(io::Error),
}

impl From<io::Error> for RecvError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl fmt::Display for RecvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disconnected => {
                f.write_str("the queue is empty and every producer has been dropped")
            }
            Self::Io(error) => write!(f, "waiting on the queue's doorbell failed: {error}"),
        }
    }
}

impl core::error::Error for RecvError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Disconnected => None,
            Self::Io(error) => Some(error),
        }
    }
}

/// Why a blocking receive with a deadline gave up.
///
/// Distinct from [`RecvError`] rather than a variant of it, so a caller that
/// cannot time out is not obliged to handle a case that cannot happen.
#[derive(Debug)]
#[non_exhaustive]
pub enum RecvTimeoutError {
    /// The deadline passed with the queue still empty.
    ///
    /// The queue is still live, and this is not a malfunction: a caller polling
    /// with a short deadline will see it constantly and should simply ask
    /// again.
    Timeout,
    /// Every producer has been dropped and the queue has been drained.
    Disconnected,
    /// A Windows call failed while creating or waiting on the doorbell.
    Io(io::Error),
}

impl RecvTimeoutError {
    /// Whether asking again could succeed.
    ///
    /// True only for [`RecvTimeoutError::Timeout`]. Both other variants are
    /// terminal -- no further item will ever arrive, so retrying is a spin.
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        matches!(self, Self::Timeout)
    }
}

impl From<io::Error> for RecvTimeoutError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl fmt::Display for RecvTimeoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout => f.write_str("the queue was still empty when the deadline passed"),
            Self::Disconnected => {
                f.write_str("the queue is empty and every producer has been dropped")
            }
            Self::Io(error) => write!(f, "waiting on the queue's doorbell failed: {error}"),
        }
    }
}

impl core::error::Error for RecvTimeoutError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Timeout | Self::Disconnected => None,
            Self::Io(error) => Some(error),
        }
    }
}
