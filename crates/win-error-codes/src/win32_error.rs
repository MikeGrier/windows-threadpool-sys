// Copyright (c) Mike Grier.

use std::fmt;
#[cfg(windows)]
use std::io;

#[cfg(test)]
mod tests;

/// `ERROR_SUCCESS`: the code that means the call succeeded. A Win32 error code of zero is not
/// the absence of a code.
const ERROR_SUCCESS: u32 = 0;

/// A Win32 error code, exactly as Windows produced it.
///
/// The `DWORD` that `GetLastError` returns, and that a call such as `GetStorageDependencyInformation`
/// returns directly. Deliberately not an enum: the type carries whatever Windows said, including a
/// code this crate has never heard of. **It is not assumed to fit in 16 bits**: the virtdisk errors
/// are `WIN32_ERROR`s above 2^31, and `u32` holds them.
///
/// `ERROR_SUCCESS` is an ordinary value of this type, zero, and [`is_success`](Self::is_success)
/// and [`into_result`](Self::into_result) are how a call's return is turned into a `Result`.
///
/// # Example
///
/// ```
/// use win_error_codes::Win32Error;
///
/// // ERROR_FILE_NOT_FOUND.
/// let error = Win32Error::from_code(2);
/// assert_eq!(error.code(), 2);
/// assert!(!error.is_success());
///
/// // A call that returns its code directly: zero is success.
/// assert_eq!(Win32Error::from_code(0).into_result(), Ok(()));
/// assert_eq!(Win32Error::from_code(2).into_result(), Err(error));
///
/// // A code above 2^31 is carried as it is.
/// assert_eq!(Win32Error::from_code(0xC03A_0015).code(), 0xC03A_0015);
/// ```
///
/// On Windows the `io::Error` form keeps the code exactly, however large it is:
///
/// ```
/// # #[cfg(windows)] {
/// use win_error_codes::Win32Error;
///
/// // ERROR_VIRTDISK_NOT_VIRTUAL_DISK.
/// let error = Win32Error::from_code(0xC03A_0015);
/// let io_error = error.to_io_error();
///
/// assert_eq!(Win32Error::from_io(&io_error), Some(error));
/// # }
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Win32Error(u32);

impl Win32Error {
    /// Wraps a raw code. Any value is accepted, zero included.
    #[must_use]
    pub const fn from_code(code: u32) -> Self {
        Self(code)
    }

    /// The raw code, unaltered.
    #[must_use]
    pub const fn code(self) -> u32 {
        self.0
    }

    /// Whether this is `ERROR_SUCCESS`, which is zero.
    #[must_use]
    pub const fn is_success(self) -> bool {
        self.0 == ERROR_SUCCESS
    }

    /// `Ok` for `ERROR_SUCCESS` and `Err` carrying this code for anything else, which is how a call
    /// that returns its code directly becomes a `Result`.
    ///
    /// # Errors
    ///
    /// This code, when it is not `ERROR_SUCCESS`.
    pub const fn into_result(self) -> Result<(), Self> {
        if self.is_success() { Ok(()) } else { Err(self) }
    }
}

#[cfg(windows)]
impl Win32Error {
    /// The code in an [`io::Error`], or `None` for one that carries no OS error at all (one made
    /// with `io::Error::other`, or from an [`io::ErrorKind`] alone).
    ///
    /// `None` and not zero: zero is `ERROR_SUCCESS`, so an error with no code that came back as a
    /// code would read as a success.
    ///
    /// Std's `io::Error` holds an OS error as an `i32`, so the cast back to the `u32` a Win32 code
    /// is happens here and is bit-preserving: a code above 2^31, which std holds as a negative
    /// number, comes back as it went in.
    #[must_use]
    pub fn from_io(error: &io::Error) -> Option<Self> {
        error.raw_os_error().map(|raw| Self(raw.cast_unsigned()))
    }

    /// The same failure as a standard [`io::Error`], for callers that funnel everything through
    /// `std::io`.
    ///
    /// A re-presentation and not a reclassification: [`io::Error::raw_os_error`] on the result is
    /// this code, cast at std's `i32` boundary and bit-preserved, **for every code**, and
    /// [`from_io`](Self::from_io) reverses it exactly.
    #[must_use]
    pub fn to_io_error(self) -> io::Error {
        io::Error::from_raw_os_error(self.0.cast_signed())
    }

    /// The calling thread's last error, as `GetLastError` holds it.
    ///
    /// Call this immediately after the failing call: the slot is volatile thread state, and almost
    /// any other Win32 call overwrites it, a `CloseHandle` in a `Drop` included.
    #[must_use]
    pub fn last() -> Self {
        // `last_os_error` is always an OS error, so the zero is never taken.
        Self(
            io::Error::last_os_error()
                .raw_os_error()
                .map_or(ERROR_SUCCESS, i32::cast_unsigned),
        )
    }
}

#[cfg(windows)]
impl From<Win32Error> for io::Error {
    fn from(error: Win32Error) -> Self {
        error.to_io_error()
    }
}

/// The code in decimal and in hex, and on Windows the system's message for it after a colon.
///
/// The message is [`io::Error`]'s own rendering, which this crate does not edit: it ends with std's
/// `(os error N)`, where `N` is the code as std holds it, so a code above 2^31 appears there as a
/// negative number as well, and a code the system has no message for says so.
impl fmt::Display for Win32Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Win32 error {} (0x{:08X})", self.0, self.0)?;
        #[cfg(windows)]
        write!(f, ": {}", self.to_io_error())?;
        Ok(())
    }
}

impl std::error::Error for Win32Error {}
