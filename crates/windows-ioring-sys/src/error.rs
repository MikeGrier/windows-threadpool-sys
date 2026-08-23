// Copyright (c) 2026 Mike Grier
//! IoRing errors: `HRESULT`, not the `GetLastError` convention.

use std::fmt;
use std::io;

use windows_sys::Win32::Foundation::{
    IORING_E_COMPLETION_QUEUE_TOO_BIG, IORING_E_COMPLETION_QUEUE_TOO_FULL, IORING_E_CORRUPT,
    IORING_E_REQUIRED_FLAG_NOT_SUPPORTED, IORING_E_SUBMISSION_QUEUE_FULL,
    IORING_E_SUBMISSION_QUEUE_TOO_BIG, IORING_E_SUBMIT_IN_PROGRESS, IORING_E_VERSION_NOT_SUPPORTED,
};
use windows_sys::core::HRESULT;

/// An error from an `IoRing` API call.
///
/// Every `IoRing` entry point reports failure as an `HRESULT`, not through
/// `GetLastError` the way most of Win32 does, so this crate cannot reuse
/// `io::Error::last_os_error` the way `windows-overlapped-io-sys` does.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct IoRingError {
    code: HRESULT,
}

impl IoRingError {
    pub(crate) fn new(code: HRESULT) -> Self {
        Self { code }
    }

    /// The raw `HRESULT`.
    #[must_use]
    pub fn code(&self) -> HRESULT {
        self.code
    }

    /// The named `IORING_E_*` constant this value matches, if any.
    #[must_use]
    pub fn name(&self) -> Option<&'static str> {
        match self.code {
            IORING_E_REQUIRED_FLAG_NOT_SUPPORTED => Some("IORING_E_REQUIRED_FLAG_NOT_SUPPORTED"),
            IORING_E_VERSION_NOT_SUPPORTED => Some("IORING_E_VERSION_NOT_SUPPORTED"),
            IORING_E_SUBMISSION_QUEUE_FULL => Some("IORING_E_SUBMISSION_QUEUE_FULL"),
            IORING_E_SUBMISSION_QUEUE_TOO_BIG => Some("IORING_E_SUBMISSION_QUEUE_TOO_BIG"),
            IORING_E_COMPLETION_QUEUE_TOO_BIG => Some("IORING_E_COMPLETION_QUEUE_TOO_BIG"),
            IORING_E_CORRUPT => Some("IORING_E_CORRUPT"),
            IORING_E_SUBMIT_IN_PROGRESS => Some("IORING_E_SUBMIT_IN_PROGRESS"),
            IORING_E_COMPLETION_QUEUE_TOO_FULL => Some("IORING_E_COMPLETION_QUEUE_TOO_FULL"),
            _ => None,
        }
    }
}

impl fmt::Debug for IoRingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IoRingError")
            .field("code", &format_args!("0x{:08X}", self.code as u32))
            .finish()
    }
}

impl fmt::Display for IoRingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            Some(name) => write!(f, "{name} (HRESULT 0x{:08X})", self.code as u32),
            None => write!(f, "HRESULT 0x{:08X}", self.code as u32),
        }
    }
}

impl std::error::Error for IoRingError {}

/// Convert a native call's `HRESULT` into `Ok(())` or a wrapped
/// [`IoRingError`], following the `FAILED(hr)` convention (a negative
/// `HRESULT` is a failure; `S_OK` and `S_FALSE` are both non-negative
/// successes).
pub(crate) fn check(hr: HRESULT) -> io::Result<()> {
    if hr < 0 {
        Err(io::Error::other(IoRingError::new(hr)))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
