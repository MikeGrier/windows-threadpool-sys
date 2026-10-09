// Copyright (c) 2026 Mike Grier
//! The clock the core stamps failures and markings with (DI-D-37, DI-D-38, DI-D-50): any reading
//! that never decreases. A driver supplies one -- interrupt time for Windows' -- and a test, a
//! mock it advances by hand.

use std::fmt::Debug;

/// What a stamp is: a reading of a [`StampClock`], ordered, and cheap to copy and share.
pub trait Instant: Copy + Ord + Debug + Send + Sync + 'static {}

impl<T: Copy + Ord + Debug + Send + Sync + 'static> Instant for T {}

/// A clock whose readings never decrease, read once per stamped transition.
pub trait StampClock: Send + 'static {
    /// What a reading is.
    type Instant: Instant;

    /// A reading: never before one taken earlier.
    fn now(&self) -> Self::Instant;
}
