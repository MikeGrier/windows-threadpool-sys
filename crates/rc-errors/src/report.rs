// Copyright (c) 2026 Mike Grier
//! The report (RCE-D-7): a static error or a shared one, so code that reports both names one type.

use crate::shared::SharedError;
use core::error::Error;
use core::fmt;
use core::ops::Deref;
use core::ptr;

#[cfg(test)]
mod tests;

/// An error that is either a `static`, which needs no storage, or a [`SharedError`].
///
/// It dereferences to `dyn Error + Send + Sync` in both forms, and compares by identity: two
/// static reports are equal when they name the same static, two shared ones when they share one
/// block, and a static report never equals a shared one. A zero-sized static's address may be
/// shared with another zero-sized static, so give a static that is to be told apart a size;
/// [`StaticError`](crate::StaticError) has one. Like [`SharedError`], it is an `Error` itself, so
/// downcast through `Deref` to reach the error it holds.
///
/// ```
/// use rc_errors::{ErrorReport, StaticError};
///
/// static REFUSED: StaticError = StaticError::new("refused");
///
/// let report = ErrorReport::from_static(&REFUSED);
/// assert_eq!(report, report.clone());
/// assert_eq!(report.to_string(), "refused");
/// assert!(report.downcast_ref::<StaticError>().is_some());
/// ```
#[derive(Clone)]
pub enum ErrorReport {
    /// A static error: reporting it allocates nothing.
    Static(&'static (dyn Error + Send + Sync)),
    /// A shared error.
    Shared(SharedError),
}

impl ErrorReport {
    /// A report of a static error.
    #[must_use]
    pub const fn from_static(error: &'static (dyn Error + Send + Sync)) -> Self {
        Self::Static(error)
    }
}

impl From<SharedError> for ErrorReport {
    fn from(error: SharedError) -> Self {
        Self::Shared(error)
    }
}

impl From<&'static (dyn Error + Send + Sync)> for ErrorReport {
    fn from(error: &'static (dyn Error + Send + Sync)) -> Self {
        Self::Static(error)
    }
}

impl Deref for ErrorReport {
    type Target = dyn Error + Send + Sync;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Static(error) => *error,
            // Through the `SharedError` to the error it holds: `error` alone would coerce the
            // `SharedError` itself, which is also an `Error`, and downcasting would then see it.
            Self::Shared(error) => &**error,
        }
    }
}

impl fmt::Display for ErrorReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}

impl fmt::Debug for ErrorReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl Error for ErrorReport {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        (**self).source()
    }
}

impl PartialEq for ErrorReport {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Static(a), Self::Static(b)) => ptr::addr_eq(*a, *b),
            (Self::Shared(a), Self::Shared(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for ErrorReport {}
