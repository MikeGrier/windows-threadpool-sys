// Copyright (c) Mike Grier.

//! The faithful-execution contract every entry follows.
//!
//! An entry reports what Windows reported. It does not normalise a code, map it
//! onto a friendlier taxonomy, or decide that one failure "really means"
//! something else.
//!
//! # Why preservation is a constraint, not a preference
//!
//! `ERROR_FILE_NOT_FOUND` means three different things depending on which call
//! produced it and when: a missing directory from an open, an **empty**
//! directory from a first query, and a genuine failure from a later one. Only a
//! consumer holding that context can tell them apart. Any reclassification here
//! destroys information no layer above can reconstruct.
//!
//! # Why the code is snapshotted rather than read later
//!
//! `GetLastError` is thread state, and it is *volatile* thread state: almost
//! any subsequent Win32 call overwrites it, including cleanup a caller does not
//! think of as a call at all -- a `CloseHandle` in a `Drop`, a buffer being
//! released, a restoration guard unwinding. Reading it a few statements after
//! the failure is a race against the entry's own tidying up.
//!
//! So the read is not left to the caller's discipline. [`perform`] and its
//! convention-specific forms take the call as a closure and snapshot the code
//! **in the statement after it returns**, before anything else can run. Binding
//! to these functions is what makes the guarantee structural rather than a rule
//! each entry has to remember.
//!
//! # Scope: entries, not capture
//!
//! This governs the Win32 call an entry *performs*. It does not govern capture
//! failures -- [`crate::handle`], [`crate::security`], and [`crate::path`]
//! report a named stage plus a code, because there the useful question is which
//! part of building the request went wrong. Those happen on the calling thread,
//! before any entry runs.

use std::fmt;
use std::io;

use windows_sys::Win32::Foundation::{
    FALSE, GetLastError, HANDLE, INVALID_HANDLE_VALUE, WIN32_ERROR,
};

/// A raw Win32 error code, exactly as Windows produced it.
///
/// Deliberately not an enum: the point of this type is that it carries whatever
/// Windows said, including codes this crate has never heard of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Win32Error(WIN32_ERROR);

impl Win32Error {
    /// Wraps a raw `WIN32_ERROR`.
    #[must_use]
    pub fn from_code(code: WIN32_ERROR) -> Self {
        Self(code)
    }

    /// Snapshots the calling thread's last error.
    ///
    /// Call this only immediately after the failing Win32 call. Prefer
    /// [`perform`], which makes that ordering structural.
    fn last() -> Self {
        // SAFETY: GetLastError only reads the calling thread's own error slot.
        Self(unsafe { GetLastError() })
    }

    /// The raw code, unaltered.
    #[must_use]
    pub fn code(self) -> WIN32_ERROR {
        self.0
    }

    /// The same failure as a standard [`io::Error`], for callers that funnel
    /// everything through `std::io`.
    ///
    /// This is a re-presentation, not a reclassification: the raw code survives
    /// as [`io::Error::raw_os_error`].
    #[must_use]
    pub fn to_io_error(self) -> io::Error {
        i32::try_from(self.0).map_or_else(
            |_| io::Error::other(format!("Win32 error {}", self.0)),
            io::Error::from_raw_os_error,
        )
    }
}

impl fmt::Display for Win32Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Win32 error {}: {}", self.0, self.to_io_error())
    }
}

impl std::error::Error for Win32Error {}

/// What an entry's Win32 call produced: its result, or the raw code.
pub type Outcome<T> = Result<T, Win32Error>;

/// Performs `call` and, when `failed` says the result is a failure, snapshots
/// the thread's last error before anything else can run.
///
/// `failed` decides using only the returned value, because Win32's failure
/// conventions differ per call and none of them is inferable from the type. The
/// three the catalogue actually meets have named forms:
/// [`perform_bool`], [`perform_handle`], and [`perform_nonzero`]. Use this
/// general form for a call whose convention is none of those.
pub fn perform<T>(call: impl FnOnce() -> T, failed: impl FnOnce(&T) -> bool) -> Outcome<T> {
    let result = call();

    // Nothing may go between these two statements. No drop runs here, no
    // cleanup, no second Win32 call -- which is the entire reason this function
    // exists rather than each entry reading GetLastError for itself.
    if failed(&result) {
        return Err(Win32Error::last());
    }

    Ok(result)
}

/// Performs a call whose `BOOL` return is `FALSE` on failure.
///
/// The successful value carries no information beyond "it worked", so it is
/// discarded rather than handed back as a bare integer.
///
/// # Errors
///
/// Returns the raw Win32 code when the call returns `FALSE`.
pub fn perform_bool(call: impl FnOnce() -> i32) -> Outcome<()> {
    perform(call, |result| *result == FALSE).map(|_| ())
}

/// Performs a call whose `HANDLE` return is `INVALID_HANDLE_VALUE` on failure.
///
/// This is the convention of `CreateFileW`, `OpenFileById`, and
/// `FindFirstChangeNotificationW`.
///
/// # Errors
///
/// Returns the raw Win32 code when the call returns `INVALID_HANDLE_VALUE`.
pub fn perform_handle(call: impl FnOnce() -> HANDLE) -> Outcome<HANDLE> {
    perform(call, |result| *result == INVALID_HANDLE_VALUE)
}

/// Performs a call whose `HANDLE` return is **null** on failure.
///
/// A distinct convention from [`perform_handle`], and getting the two the wrong
/// way round turns a failure into a plausible-looking handle. Both are provided
/// because Windows uses both.
///
/// # Errors
///
/// Returns the raw Win32 code when the call returns a null handle.
pub fn perform_nonnull_handle(call: impl FnOnce() -> HANDLE) -> Outcome<HANDLE> {
    perform(call, |result| result.is_null())
}

/// Performs a call whose numeric return is `0` on failure.
///
/// This is the convention of the sizing and length calls, such as
/// `GetFullPathNameW` and `GetFinalPathNameByHandleW`.
///
/// # Errors
///
/// Returns the raw Win32 code when the call returns `0`.
pub fn perform_nonzero(call: impl FnOnce() -> u32) -> Outcome<u32> {
    perform(call, |result| *result == 0)
}

#[cfg(test)]
mod tests;
