// Copyright (c) Mike Grier.

//! The switches a queue is built with.
//!
//! # Why a builder rather than more constructors
//!
//! There are two independent choices at construction -- what becomes of
//! undrained items, and whether peak depth is tracked -- across three shapes.
//! Spelled as constructors that is four functions per shape and twelve in the
//! crate, and every future switch doubles it again. Spelled as one value passed
//! to one `bounded_with`, a new switch is a new method and nothing else moves.
//!
//! The plain [`bounded`](crate::spsc::bounded) constructor stays, because the
//! default is the overwhelmingly common case and it should not have to say so.
//!
//! # Both switches are off by default, for different reasons
//!
//! **Disposal** is off because destroying an item that owns nothing, where it
//! lies, is exactly right -- a queue of `u32` should not have to think about
//! teardown at all. See [`Disposal`].
//!
//! **High-water tracking** is off because it is the one metric that cannot be
//! made free. Refusals and doorbell rings sit on paths that were already paying
//! for themselves, but a peak has to observe every change, and on
//! [`slotwise_mpsc`](crate::slotwise_mpsc) observing the depth means the producer reading the
//! consumer's position -- the single shared line that shape's push is built to
//! avoid touching. So it is a switch, and the cost lands only on queues that
//! asked for the answer.

use core::fmt;

use crate::disposal::Disposal;

/// What a queue is built with, beyond its capacity.
///
/// # Examples
///
/// ```
/// use windows_waitable_queues::{Options, spsc};
///
/// let (tx, rx) = spsc::bounded_with::<u32>(4, Options::new().tracking_high_water())?;
///
/// tx.push(1).expect("a fresh queue has room");
/// tx.push(2).expect("a fresh queue has room");
/// assert_eq!(rx.pop(), Some(1));
///
/// // The peak, not the depth right now.
/// assert_eq!(rx.len(), 1);
/// assert_eq!(rx.high_water(), Some(2));
/// # Ok::<(), windows_waitable_queues::CapacityError>(())
/// ```
pub struct Options<T> {
    pub(crate) disposal: Option<Disposal<T>>,
    pub(crate) track_high_water: bool,
}

impl<T> Options<T> {
    /// The defaults: undrained items destroyed in place, peak depth untracked.
    #[must_use]
    pub fn new() -> Self {
        Self {
            disposal: None,
            track_high_water: false,
        }
    }

    /// Hand undrained items to `disposal` at teardown instead of destroying
    /// them where they lie.
    ///
    /// See [`Disposal`] for why this has to be decided here rather than asked
    /// for at teardown.
    #[must_use]
    pub fn disposal(mut self, disposal: Disposal<T>) -> Self {
        self.disposal = Some(disposal);
        self
    }

    /// Track the deepest the queue gets, readable from
    /// [`Observable::high_water`](crate::Observable::high_water).
    ///
    /// **This is the one option that costs the push path something**, which is
    /// why it is off by default. A peak has to observe every change, so on
    /// `slotwise_mpsc` it makes the producer read the consumer's position -- the shared
    /// line that shape's push exists to avoid. On `spsc` and `reserving_mpsc`
    /// the producer already knows the depth, so it costs those two almost
    /// nothing.
    ///
    /// Untracked, `high_water` reports `None` rather than `0`, so a caller
    /// cannot mistake "nobody was counting" for "it never filled".
    #[must_use]
    pub fn tracking_high_water(mut self) -> Self {
        self.track_high_water = true;
        self
    }
}

impl<T> Default for Options<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> fmt::Debug for Options<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Options")
            .field("disposal", &self.disposal.is_some())
            .field("track_high_water", &self.track_high_water)
            .finish()
    }
}
