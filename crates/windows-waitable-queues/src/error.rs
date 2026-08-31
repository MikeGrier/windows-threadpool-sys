// Copyright (c) Mike Grier.

//! Errors shared by every queue shape.
//!
//! They live at the crate root rather than inside a shape's module because the
//! shapes must agree on them: a trait cannot unify `push` across shapes if each
//! returns a differently-named error meaning the same thing.

use core::fmt;
use std::io;

use crate::capacity::Bounds;

/// Why a capacity was rejected at construction.
///
/// Constructing a queue is the one place a caller can get this wrong, so it is
/// reported rather than rounded away. Silently rounding 100 up to 128 would
/// hand back a bound the caller cannot see they got, and a bound is exactly the
/// number a caller chose deliberately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapacityError {
    requested: usize,
    /// The smallest capacity the rejecting shape accepts.
    ///
    /// Carried for the same reason as [`Self::max_valid`], and it is not always
    /// one: `mpsc` cannot represent a capacity below two, because its slot
    /// state machine reuses a sequence number one lap later and a one-slot ring
    /// would make "published" and "free again" the same value.
    min_valid: usize,
    /// The largest capacity the rejecting shape accepts.
    ///
    /// Carried on the error rather than assumed to be a crate-wide constant:
    /// the bound follows from how a shape represents its positions, and the
    /// shapes differ. Most stop where a wrapping difference between positions
    /// stops being unambiguous; `reserving_mpsc` stops far lower, because it
    /// packs its reservation count into the same word as its position so the
    /// two can be claimed together. A suggestion computed against the wrong
    /// bound is worse than no suggestion, because a caller will act on it.
    max_valid: usize,
    kind: CapacityErrorKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CapacityErrorKind {
    Zero,
    NotPowerOfTwo,
    TooSmall,
    TooLarge,
}

impl CapacityError {
    fn new(requested: usize, bounds: Bounds, kind: CapacityErrorKind) -> Self {
        Self {
            requested,
            min_valid: bounds.min,
            max_valid: bounds.max,
            kind,
        }
    }

    pub(crate) fn zero(bounds: Bounds) -> Self {
        Self::new(0, bounds, CapacityErrorKind::Zero)
    }

    pub(crate) fn not_power_of_two(requested: usize, bounds: Bounds) -> Self {
        Self::new(requested, bounds, CapacityErrorKind::NotPowerOfTwo)
    }

    pub(crate) fn too_small(requested: usize, bounds: Bounds) -> Self {
        Self::new(requested, bounds, CapacityErrorKind::TooSmall)
    }

    pub(crate) fn too_large(requested: usize, bounds: Bounds) -> Self {
        Self::new(requested, bounds, CapacityErrorKind::TooLarge)
    }

    /// The smallest capacity the shape that rejected this request will accept.
    #[must_use]
    pub fn min_valid(&self) -> usize {
        self.min_valid
    }

    /// The largest capacity the shape that rejected this request will accept.
    #[must_use]
    pub fn max_valid(&self) -> usize {
        self.max_valid
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
    ///
    /// Never returns a value the shape would itself reject. Rounding a request
    /// down to the nearest power of two is not sufficient on its own: the
    /// nearest power of two below `usize::MAX` is 2^63, which exceeds the
    /// largest representable capacity, so the answer is clamped to
    /// [`Self::max_valid`], and a result below [`Self::min_valid`] is reported
    /// as no suggestion at all. A suggestion that is itself refused would be
    /// worse than none, because a caller acts on it and gets a second error.
    #[must_use]
    pub fn previous_valid(&self) -> Option<usize> {
        match self.kind {
            // Nothing valid lies below either of these: a request that was
            // already too small has only larger answers, and zero has none.
            CapacityErrorKind::Zero | CapacityErrorKind::TooSmall => None,
            CapacityErrorKind::NotPowerOfTwo | CapacityErrorKind::TooLarge => {
                let rounded = 1_usize << (usize::BITS - 1 - self.requested.leading_zeros());
                let clamped = rounded.min(self.largest_power_of_two_within_bound());
                (clamped >= self.min_valid).then_some(clamped)
            }
        }
    }

    /// The largest power of two that does not exceed [`Self::max_valid`].
    ///
    /// The clamp target for [`Self::previous_valid`]: `max_valid` need not be a
    /// power of two -- for a ring of monotonic wrapping positions it is
    /// `usize::MAX / 2`, which is `2^63 - 1` -- so clamping to it
    /// directly would hand back a capacity that fails the power-of-two test
    /// instead of the size test.
    fn largest_power_of_two_within_bound(&self) -> usize {
        if self.max_valid == 0 {
            return 0;
        }
        1_usize << (usize::BITS - 1 - self.max_valid.leading_zeros())
    }

    /// The smallest valid capacity not less than the request, if there is one.
    ///
    /// `None` when rounding up would leave the shape's bound behind, which is
    /// not only the case for a request that was already too large: one that is
    /// merely *not a power of two* can still sit between the largest valid
    /// power of two and the bound, and rounding it up then overshoots. There is
    /// genuinely no valid capacity at or above such a request, so saying so is
    /// the honest answer -- [`Self::previous_valid`] is the one that can still
    /// help.
    #[must_use]
    pub fn next_valid(&self) -> Option<usize> {
        let rounded = match self.kind {
            // The shape's own minimum, not one: a shape whose slot state
            // machine needs two slots would reject a suggestion of one, and a
            // suggestion that is itself refused is worse than none.
            CapacityErrorKind::Zero | CapacityErrorKind::TooSmall => Some(self.min_valid),
            CapacityErrorKind::NotPowerOfTwo => self.requested.checked_next_power_of_two(),
            CapacityErrorKind::TooLarge => None,
        }?;
        (rounded >= self.min_valid && rounded <= self.max_valid).then_some(rounded)
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
            CapacityErrorKind::TooSmall => write!(
                f,
                "capacity {} is below the smallest this queue shape can represent, which is {}",
                self.requested, self.min_valid
            ),
            CapacityErrorKind::TooLarge => write!(
                f,
                "capacity {} is above the largest this queue shape can represent, which is {}",
                self.requested, self.max_valid
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

/// The only way delivering into a reserved slot can fail: nobody is left to
/// take it.
///
/// **There is deliberately no `Full` here, and the absence is the contract.** A
/// reservation's whole purpose is that the room is already the holder's, so a
/// full queue cannot refuse it. Returning [`PushError`] instead would name a
/// case that cannot occur and oblige every caller to handle it, which is how a
/// guarantee decays back into a thing you hope is true.
///
/// The item comes back for the same reason it does from a refused push: a queue
/// that swallows what it cannot deliver leaves the caller no way to account for
/// it. That matters more here than elsewhere -- an item important enough to
/// reserve a slot for is exactly the kind whose disposal must not be silent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Disconnected<T>(pub T);

impl<T> Disconnected<T> {
    /// Takes the item back out.
    #[must_use]
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> fmt::Display for Disconnected<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("every consumer is gone")
    }
}

impl<T: fmt::Debug> core::error::Error for Disconnected<T> {}

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
