// Copyright (c) Mike Grier.

use std::fmt;

use crate::Win32Error;

#[cfg(test)]
mod tests;

/// A Win32 error code in a signed carrier: the `LONG` that `LSTATUS` is, which the registry calls
/// (`RegOpenKeyExW` and the rest) return.
///
/// It is the same number space as [`Win32Error`], and the registry's own documentation calls the
/// value an error code: zero is `ERROR_SUCCESS` and anything else is the code. It is a type of its
/// own because it is the signed carrier, so a `LONG` is never read as a `DWORD` by accident, and
/// the cast between the two is made once, here, and is a bit cast that loses nothing in either
/// direction. A value is not assumed to be non-negative: a code above 2^31 is a negative `LONG`.
///
/// # Example
///
/// ```
/// use win_error_codes::{LStatus, Win32Error};
///
/// // What a registry call returns for success, and for ERROR_FILE_NOT_FOUND.
/// assert_eq!(LStatus::from_raw(0).into_result(), Ok(()));
/// assert_eq!(
///     LStatus::from_raw(2).into_result(),
///     Err(Win32Error::from_code(2))
/// );
///
/// // The same number space, so the two convert both ways without loss, including a code that
/// // needs the sign bit.
/// let status = LStatus::from_raw(-1_069_940_715);
/// assert_eq!(status.to_win32(), Win32Error::from_code(0xC03A_0015));
/// assert_eq!(Win32Error::from_code(0xC03A_0015).to_lstatus(), status);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LStatus(i32);

impl LStatus {
    /// Wraps the raw `LONG` a call returned. Any value is accepted, zero included.
    #[must_use]
    pub const fn from_raw(raw: i32) -> Self {
        Self(raw)
    }

    /// The raw `LONG`, unaltered.
    #[must_use]
    pub const fn raw(self) -> i32 {
        self.0
    }

    /// Whether this is `ERROR_SUCCESS`, which is zero.
    #[must_use]
    pub const fn is_success(self) -> bool {
        self.to_win32().is_success()
    }

    /// `Ok` for `ERROR_SUCCESS` and `Err` carrying the code for anything else, which is how a call
    /// that returns an `LSTATUS` becomes a `Result`.
    ///
    /// # Errors
    ///
    /// The code, as a [`Win32Error`], when it is not `ERROR_SUCCESS`.
    pub const fn into_result(self) -> Result<(), Win32Error> {
        self.to_win32().into_result()
    }

    /// The same code as a [`Win32Error`]: a bit cast from `i32` to `u32`, so a negative value is a
    /// code above 2^31 and not its magnitude.
    #[must_use]
    pub const fn to_win32(self) -> Win32Error {
        Win32Error::from_code(self.0.cast_unsigned())
    }
}

impl Win32Error {
    /// The same code as an [`LStatus`]: a bit cast from `u32` to `i32`, so a code above 2^31 is a
    /// negative value, which is how a `LONG` carries it.
    #[must_use]
    pub const fn to_lstatus(self) -> LStatus {
        LStatus::from_raw(self.code().cast_signed())
    }
}

impl From<LStatus> for Win32Error {
    fn from(status: LStatus) -> Self {
        status.to_win32()
    }
}

impl From<Win32Error> for LStatus {
    fn from(error: Win32Error) -> Self {
        error.to_lstatus()
    }
}

/// The code as [`Win32Error`] shows it: there is no separate rendering for the signed carrier,
/// since the two are the same number space.
impl fmt::Display for LStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.to_win32(), f)
    }
}
