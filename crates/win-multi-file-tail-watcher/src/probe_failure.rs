// Copyright (c) 2026 Mike Grier
//! What a failed probe means: transient or terminal, decided in one place (TW-D-13).

use std::io;

use windows_sys::Win32::Foundation::{ERROR_FILE_INVALID, ERROR_INVALID_HANDLE};

#[cfg(test)]
mod tests;

/// The Win32 codes that mean a handle can never answer again. The set is not complete: it grows
/// as the system produces codes nobody listed, so this is the one list to add to.
const TERMINAL_CODES: &[u32] = &[
    // The handle is closed or was never valid.
    ERROR_INVALID_HANDLE,
    // The volume under the file was externally altered, so the open file is no longer valid.
    ERROR_FILE_INVALID,
];

/// What a failed probe means for the registration it was made through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FailureClass {
    /// It may not recur: probe again on the next beat.
    Transient,
    /// The handle can never answer again: report it, and stop probing.
    Terminal,
}

impl FailureClass {
    /// Whether the registration stops being probed.
    pub(crate) fn is_terminal(self) -> bool {
        self == Self::Terminal
    }
}

/// The Win32 code `error` carries, if it carries one. Only a plain Win32 code is read: an error
/// made from a message, or from an `HRESULT`, carries none.
pub(crate) fn win32_code(error: &io::Error) -> Option<u32> {
    error.raw_os_error().map(i32::cast_unsigned)
}

/// Whether a probe that failed with `error` is worth repeating. A code that is not listed as
/// terminal, and an error with no code, are transient.
pub(crate) fn classify(error: &io::Error) -> FailureClass {
    classify_code(win32_code(error))
}

/// [`classify`] for a failure whose code is already known, which a read's is: the ring reports it
/// inside an `HRESULT`.
pub(crate) fn classify_code(code: Option<u32>) -> FailureClass {
    match code {
        Some(code) if TERMINAL_CODES.contains(&code) => FailureClass::Terminal,
        _ => FailureClass::Transient,
    }
}
