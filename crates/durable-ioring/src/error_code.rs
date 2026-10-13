// Copyright (c) 2026 Mike Grier
//! A fault's code, as the layer that reported it numbers it (DI-D-39, DI-D-50). Reading one out of
//! an `io::Error` is the reporting layer's: `win-durable-ioring` reads a Windows backend's.

use std::fmt;

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

/// A fault's code, as the layer that reported it numbers it: its meaning is the [`IoFault`](crate::IoFault)'s
/// source's (DI-D-50). The Windows backend's are `HRESULT`s, the form the I/O ring reports every
/// failure in, a Win32 code being one wrapped by `HRESULT_FROM_WIN32`; the helpers below read
/// and build that form.
///
/// Carried by every [`IoFault`](crate::IoFault) a layer reports with one, so a consumer reads a
/// code without that layer's own error types. The Windows backend's are read from an `io::Error`
/// by `win-durable-ioring`'s `error_code_of`.
///
/// ```
/// use durable_ioring::ErrorCode;
///
/// // ERROR_IO_DEVICE, as a Win32 error is numbered.
/// let code = ErrorCode::from_win32(1117);
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
