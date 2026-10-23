// Copyright (c) 2026 Mike Grier
//! The static error (RCE-D-6): a message, made in a `const`, for a report that needs no storage.

use core::error::Error;
use core::fmt;

#[cfg(test)]
mod tests;

/// An error that is only a message, made in a `const` or a `static`, so reporting it allocates
/// nothing.
///
/// It holds no code: a caller that numbers its errors carries the number beside the report
/// (RCE-D-6). Its own equality compares messages; an [`ErrorReport`](crate::ErrorReport) made from
/// it compares by the static's address.
///
/// ```
/// use rc_errors::StaticError;
///
/// static AT_CAPACITY: StaticError = StaticError::new("the ring is at capacity");
///
/// assert_eq!(AT_CAPACITY.message(), "the ring is at capacity");
/// assert_eq!(AT_CAPACITY.to_string(), "the ring is at capacity");
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct StaticError {
    message: &'static str,
}

impl StaticError {
    /// An error whose message is `message`.
    #[must_use]
    pub const fn new(message: &'static str) -> Self {
        Self { message }
    }

    /// The message.
    #[must_use]
    pub const fn message(&self) -> &'static str {
        self.message
    }
}

impl fmt::Display for StaticError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message)
    }
}

impl Error for StaticError {}
