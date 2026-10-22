// Copyright (c) 2026 Mike Grier
//! `OutputDebugStringW`: send a string to the debugger.
//!
//! Read 2026-10-20 from the call's page (`ms.date` 2024-02-02, `updated_at` 2025-08-27). The call takes one
//! argument, "The null-terminated string to be displayed", returns nothing, and has no error to report: the
//! page's return value is "None", and its remarks say that with no debugger and no active system debugger the
//! call "does nothing". So the call cannot fail, and this crate's one error is its own refusal, made before the
//! call ([`OutputDebugStringError::EmbeddedNul`]).

use std::error::Error;
use std::fmt;

use windows_sys::Win32::System::Diagnostics::Debug::OutputDebugStringW;

#[cfg(test)]
mod tests;

/// Why a string was not sent to the debugger.
///
/// The call was not made. This is the crate's refusal and not the call's: the call reports no failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum OutputDebugStringError {
    /// The string holds a NUL, and the call's string is "null-terminated", so the call would take the string
    /// to end there and display only what comes before it.
    ///
    /// Sending the part before the NUL, or leaving the rest out, would be this crate choosing what a caller
    /// meant, so the string is not sent.
    EmbeddedNul {
        /// The place of the first NUL, in UTF-16 code units from the start of the string, which for text
        /// given as a `str` is not its byte or character offset.
        index: usize,
    },
}

impl fmt::Display for OutputDebugStringError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmbeddedNul { index } => write!(
                f,
                "the string has a NUL at unit {index}, and OutputDebugStringW's string ends at its first NUL"
            ),
        }
    }
}

impl Error for OutputDebugStringError {}

/// Sends a string of UTF-16 code units to the debugger, by `OutputDebugStringW`.
///
/// The units are sent as they are: an empty slice sends the empty string, and an unpaired surrogate is a unit
/// like any other, which the call may display in whatever way the debugger does. The call takes a
/// NUL-terminated string, so a copy of the units with the terminator added is what it is given, and a slice
/// with a NUL in it is refused.
///
/// The call converts the string "based on the current system locale information and passes it to
/// **OutputDebugStringA** to be displayed", so by the page "some Unicode characters may not be displayed
/// correctly". Whether anything is displayed depends on a debugger being attached, or on the system debugger
/// being active and its filter allowing the string, and the call gives no way to tell.
///
/// # Errors
///
/// [`OutputDebugStringError::EmbeddedNul`] if `text` holds a NUL. The call is not made.
pub fn output_debug_string(text: &[u16]) -> Result<(), OutputDebugStringError> {
    let mut units = Vec::with_capacity(text.len() + 1);
    units.extend_from_slice(text);
    send(units)
}

/// Sends a `str` to the debugger, by `OutputDebugStringW`, as the UTF-16 units it encodes to.
///
/// As [`output_debug_string`], for text that is valid Unicode. A `str` cannot hold an unpaired surrogate, so
/// a caller that has one in a UTF-16 string uses that function.
///
/// # Errors
///
/// [`OutputDebugStringError::EmbeddedNul`] if `text` holds a NUL, with the index in UTF-16 code units. The
/// call is not made.
pub fn output_debug_str(text: &str) -> Result<(), OutputDebugStringError> {
    let mut units: Vec<u16> = Vec::with_capacity(text.len() + 1);
    units.extend(text.encode_utf16());
    send(units)
}

/// Refuses units with a NUL, and sends the rest with the terminator the call needs.
fn send(mut units: Vec<u16>) -> Result<(), OutputDebugStringError> {
    terminate(&mut units)?;
    // SAFETY: `units` is a buffer of `u16` that ends in a NUL and has none before it, so the pointer is valid
    // for reads up to and including that NUL, which is as far as the call reads. It is not used after the call.
    unsafe { OutputDebugStringW(units.as_ptr()) };
    Ok(())
}

/// Adds the terminator the call needs to `units`, or refuses units that have a NUL already.
fn terminate(units: &mut Vec<u16>) -> Result<(), OutputDebugStringError> {
    if let Some(index) = units.iter().position(|&unit| unit == NUL) {
        return Err(OutputDebugStringError::EmbeddedNul { index });
    }
    units.push(NUL);
    Ok(())
}

/// The unit that ends the string the call takes.
const NUL: u16 = 0;
