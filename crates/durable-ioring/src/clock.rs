// Copyright (c) 2026 Mike Grier
//! The clock the core stamps failures and markings with (DI-D-37, DI-D-38, DI-D-50): any reading
//! that never decreases. A driver supplies one -- interrupt time for Windows' -- and a test, a
//! mock it advances by hand.

use core::fmt::Debug;

/// What a stamp is: a reading of a [`StampClock`], ordered, and cheap to copy and share.
pub trait Instant: Copy + Ord + Debug + Send + Sync + 'static {}

impl<T: Copy + Ord + Debug + Send + Sync + 'static> Instant for T {}

/// A clock whose readings never decrease, read once per stamped transition.
///
/// Implementing it is the implementor's statement of that promise; the core does not check it. A
/// clock that goes backwards yields stamps out of order, which the conformance oracle reports when
/// a test gives it the entries.
pub trait StampClock: Send + 'static {
    /// What a reading is.
    type Instant: Instant;

    /// A reading: never before one taken earlier.
    fn now(&self) -> Self::Instant;
}
