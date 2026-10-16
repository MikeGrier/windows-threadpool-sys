// Copyright (c) 2026 Mike Grier
// Split from error_code.rs at 4fbcb343.
//! An error's code, read without the ring crate (DI-D-39), and the faults the Windows path
//! reports with it (DI-D-50).

use std::io;
use std::sync::Arc;

use durable_ioring::{ErrorCode, IoFault};
use win_error_codes::{HResult, Win32Error};
use windows_ioring_sys::IoRingErrorExt;

#[cfg(test)]
mod tests;

/// The source every fault the Windows path reports names: the I/O ring beneath it.
pub(crate) const WINDOWS_IORING: &str = "windows-ioring";

/// A fault the Windows path reports for `error`: the I/O ring's, with the code it carries.
pub(crate) fn windows_fault(error: io::Error) -> IoFault {
    IoFault {
        source: WINDOWS_IORING,
        code: error_code_of(&error),
        error: Arc::new(error),
    }
}

/// The code `error` carries, if it carries one.
///
/// Every failure the I/O ring reports answers `Some`, because the ring reports each as an
/// `HRESULT`; the Windows path reads it so into each [`IoFault`] it relays. An error carrying a
/// Win32 code, as `io::Error::from_raw_os_error` makes, answers it wrapped as
/// `HRESULT_FROM_WIN32` wraps it ([`error_code_from_win32`]). An error with neither, such as one a
/// consumer's provider made from a message, answers `None`. A code read from a
/// `FailureCause::Provider` error is the provider's own, relayed, not one dioring captured
/// (DI-D-47).
///
/// ```
/// use win_durable_ioring::{error_code_of, win32_code_of};
///
/// // ERROR_IO_DEVICE, as std reports a Win32 error.
/// let code = error_code_of(&std::io::Error::from_raw_os_error(1117)).unwrap();
/// assert_eq!(win32_code_of(code), Some(1117));
/// assert_eq!(code.hresult().cast_unsigned(), 0x8007_045D);
/// ```
#[must_use]
pub fn error_code_of(error: &io::Error) -> Option<ErrorCode> {
    if let Some(ring) = error.as_ioring_error() {
        return Some(ErrorCode::from_hresult(ring.code()));
    }
    error
        .raw_os_error()
        .map(|code| error_code_from_win32(code.cast_unsigned()))
}

/// The code a Win32 error names, wrapped as `HRESULT_FROM_WIN32` wraps it: a value that is already
/// an `HRESULT` -- zero, or one with the severity bit set -- is taken as it is, and any other
/// keeps only its low sixteen bits. The rule is `win-error-codes`'s ([`HResult::from_win32`]); the
/// number space is this crate's, since the codes this driver reports are `HRESULT`s.
///
/// ```
/// use win_durable_ioring::{error_code_from_win32, win32_code_of};
///
/// // ERROR_IO_DEVICE.
/// let code = error_code_from_win32(1117);
/// assert_eq!(code.hresult().cast_unsigned(), 0x8007_045D);
/// assert_eq!(win32_code_of(code), Some(1117));
/// ```
#[must_use]
pub fn error_code_from_win32(code: u32) -> ErrorCode {
    ErrorCode::from_hresult(HResult::from_win32(Win32Error::from_code(code)).raw())
}

/// The Win32 code `code` wraps, if it is one wrapped by `HRESULT_FROM_WIN32`: severity set,
/// `FACILITY_WIN32`, and no other bit above the code. `None` for `S_OK`, a success wrapping no
/// error, and for an `HRESULT` of any other facility. The inverse of [`error_code_from_win32`]
/// for the codes from 1 to 65535.
#[must_use]
pub fn win32_code_of(code: ErrorCode) -> Option<u32> {
    HResult::from_raw(code.hresult())
        .win32()
        .map(Win32Error::code)
}
