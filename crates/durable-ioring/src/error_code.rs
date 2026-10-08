// Copyright (c) 2026 Mike Grier
//! An error's code, read without the ring crate (DI-D-39).

use std::fmt;
use std::io;

use windows_ioring_sys::IoRingErrorExt;

#[cfg(test)]
mod tests;

/// The severity and facility bits of an `HRESULT` that wraps a Win32 code: severity 1 and
/// `FACILITY_WIN32` (7), as `HRESULT_FROM_WIN32` sets them. Changing it changes every code this
/// type reports.
const FROM_WIN32: u32 = 0x8007_0000;
/// The severity and facility bits of any `HRESULT`.
const SEVERITY_AND_FACILITY: u32 = 0xFFFF_0000;
/// The Win32 code within an `HRESULT` that wraps one.
const WIN32_CODE: u32 = 0x0000_FFFF;

/// What failed, as Windows numbers it: an `HRESULT`, the form the I/O ring reports every failure
/// in, a Win32 code being one wrapped by `HRESULT_FROM_WIN32`.
///
/// Read from any error dioring reports with [`ErrorCode::of`], and carried by every marking
/// recording that a suspect write failed, so a consumer reads a code without the ring crate's
/// `IoRingErrorExt`.
///
/// ```
/// use durable_ioring::ErrorCode;
///
/// // ERROR_IO_DEVICE, as std reports a Win32 error.
/// let code = ErrorCode::of(&std::io::Error::from_raw_os_error(1117)).unwrap();
/// assert_eq!(code.win32(), Some(1117));
/// assert_eq!(code.hresult().cast_unsigned(), 0x8007_045D);
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ErrorCode(i32);

impl ErrorCode {
    /// The code an `HRESULT` names.
    #[must_use]
    pub const fn from_hresult(hresult: i32) -> Self {
        Self(hresult)
    }

    /// The code a Win32 error names, wrapped as `HRESULT_FROM_WIN32` wraps it: a value that is
    /// already an `HRESULT` -- zero, or one with the severity bit set -- is taken as it is.
    #[must_use]
    pub const fn from_win32(code: u32) -> Self {
        if code.cast_signed() <= 0 {
            Self(code.cast_signed())
        } else {
            Self(((code & WIN32_CODE) | FROM_WIN32).cast_signed())
        }
    }

    /// The `HRESULT`.
    #[must_use]
    pub const fn hresult(self) -> i32 {
        self.0
    }

    /// The Win32 code, if this is one wrapped by `HRESULT_FROM_WIN32`.
    #[must_use]
    pub const fn win32(self) -> Option<u32> {
        let bits = self.0.cast_unsigned();
        if bits & SEVERITY_AND_FACILITY == FROM_WIN32 {
            Some(bits & WIN32_CODE)
        } else {
            None
        }
    }

    /// The code `error` carries, if it carries one.
    ///
    /// Every failure dioring reports from the kernel answers `Some` -- a failed completion's
    /// `Outcome::Failed` error and a failed flush's `Cause::Flush` error alike -- because the ring
    /// reports each as an `HRESULT`. An error carrying a Win32 code, as
    /// `io::Error::from_raw_os_error` makes, answers it wrapped as `HRESULT_FROM_WIN32` wraps it.
    /// An error with neither, such as one a consumer's provider made from a message, answers
    /// `None`.
    #[must_use]
    pub fn of(error: &io::Error) -> Option<Self> {
        if let Some(ring) = error.as_ioring_error() {
            return Some(Self(ring.code()));
        }
        error
            .raw_os_error()
            .map(|code| Self::from_win32(code.cast_unsigned()))
    }
}

impl fmt::Debug for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ErrorCode({self})")
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#010X}", self.0.cast_unsigned())
    }
}
