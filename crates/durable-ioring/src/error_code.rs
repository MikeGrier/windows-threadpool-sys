// Copyright (c) 2026 Mike Grier
//! A fault's code, as the layer that reported it numbers it (DI-D-39, DI-D-50). Reading one out of
//! an `io::Error` is the reporting layer's: `win-durable-ioring` reads a Windows backend's.

use std::fmt;

#[cfg(test)]
mod tests;

/// A fault's code, as the layer that reported it numbers it: its meaning is the [`IoFault`](crate::IoFault)'s
/// source's (DI-D-50). The Windows backend's are `HRESULT`s, the form the I/O ring reports every
/// failure in. A Win32 code is wrapped into one, and read back out of one, by `win-durable-ioring`,
/// which owns that rule; this type carries the number and does not interpret it.
///
/// Carried by every [`IoFault`](crate::IoFault) a layer reports with one, so a consumer reads a
/// code without that layer's own error types. The Windows backend's are read from an `io::Error`
/// by `win-durable-ioring`'s `error_code_of`.
///
/// ```
/// use durable_ioring::ErrorCode;
///
/// // ERROR_IO_DEVICE, wrapped as an HRESULT.
/// let code = ErrorCode::from_hresult(0x8007_045D_u32.cast_signed());
/// assert_eq!(code.hresult().cast_unsigned(), 0x8007_045D);
/// assert_eq!(code.to_string(), "0x8007045D");
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ErrorCode(i32);

impl ErrorCode {
    /// The code an `HRESULT` names.
    #[must_use]
    pub const fn from_hresult(hresult: i32) -> Self {
        Self(hresult)
    }

    /// The `HRESULT`.
    #[must_use]
    pub const fn hresult(self) -> i32 {
        self.0
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
