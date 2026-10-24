// Copyright (c) 2026 Mike Grier
//! Making a fault's report (DI-D-105): a static one where the report needs no storage, and a
//! shared one, from the global allocator, where it does. A shared report whose allocation fails
//! falls back to a static one, so a fault is reported either way and nothing panics.

use core::error::Error;

use rc_errors::{Allocator, ErrorReport, Global, SharedError, StaticError};

#[cfg(test)]
mod tests;

/// `error` shared from the global allocator, or `fallback` when that allocation fails, in which
/// case `error` is dropped. A caller tags the call (DI-D-104).
pub(crate) fn shared_or<E>(error: E, fallback: &'static StaticError) -> ErrorReport
where
    E: Error + Send + Sync + 'static,
{
    shared_in_or(error, &Global, fallback)
}

/// As [`shared_or`], from `allocator`: the seam a test reaches the fallback through.
fn shared_in_or<E>(
    error: E,
    allocator: &'static (dyn Allocator + Sync),
    fallback: &'static StaticError,
) -> ErrorReport
where
    E: Error + Send + Sync + 'static,
{
    SharedError::try_new_in(error, allocator)
        .map_or_else(|_| ErrorReport::from_static(fallback), ErrorReport::Shared)
}

/// The report of an `io::Error` whose allocation failed.
#[cfg(feature = "std")]
static IO_ERROR_UNSHARED: StaticError =
    StaticError::new("an I/O error, which could not be reported: allocating its report failed");

/// An `io::Error` as a fault's report: shared from the global allocator, so a downcast through it
/// finds the `io::Error`. When that allocation fails, a static report saying so, and the
/// `io::Error` is dropped: the fault is still reported, with its source and code.
///
/// ```
/// use durable_ioring::report_io_error;
///
/// let report = report_io_error(std::io::Error::other("the device was removed"));
/// assert_eq!(report.to_string(), "the device was removed");
/// assert!(report.downcast_ref::<std::io::Error>().is_some());
/// ```
#[cfg(feature = "std")]
#[must_use]
pub fn report_io_error(error: std::io::Error) -> ErrorReport {
    shared_or(error, &IO_ERROR_UNSHARED)
}

/// A fault as an `io::Error`, for a caller whose own errors are `io::Error`s, with the fault as its
/// inner error, so `get_ref` and a downcast find it again. Its kind is the kind of the `io::Error`
/// the fault's report holds, where it holds one, so a caller matching on kind sees what the layer
/// below reported; otherwise `Other`.
#[cfg(feature = "std")]
impl From<crate::IoFault> for std::io::Error {
    fn from(fault: crate::IoFault) -> Self {
        let kind = fault
            .error
            .downcast_ref::<std::io::Error>()
            .map_or(std::io::ErrorKind::Other, std::io::Error::kind);
        std::io::Error::new(kind, fault)
    }
}
