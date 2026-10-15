// Copyright (c) Mike Grier.

//! The one `DeviceIoControl` call every control entry goes through (D-20).
//!
//! # Safe on any handle, because the crate cannot see how a handle was opened
//!
//! A request owns a duplicate of a handle its caller opened, so whether that handle was opened
//! with `FILE_FLAG_OVERLAPPED` is not known here, and `DeviceIoControl` documents that on an
//! overlapped handle a call with no valid `OVERLAPPED` "fails in unpredictable ways". So every
//! control is made with an `OVERLAPPED` and an event of its own, and is waited for: it is the
//! same call on either kind of handle, and it returns only when the device has finished with the
//! output buffer.
//!
//! # No packet for a handle that has a completion port
//!
//! A handle that belongs to a completion port gets a packet when an overlapped call on it
//! completes, which would reach a consumer's loop as a completion it never asked for. The event
//! handle is passed with its low-order bit set, which `GetQueuedCompletionStatus` documents as
//! preventing exactly that, and which does not stop the event from being signalled.

use std::mem;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::ptr;

use windows_sys::Win32::Foundation::{ERROR_IO_PENDING, FALSE, HANDLE, TRUE, WAIT_FAILED};
use windows_sys::Win32::System::IO::{DeviceIoControl, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Threading::{CreateEventW, INFINITE, WaitForSingleObject};

use crate::outcome::{Outcome, perform, perform_bool, perform_nonnull_handle};

#[cfg(test)]
mod tests;

/// Sends `code`, which takes no input, to `device`, and reads what it answers into `output`: the
/// number of bytes the device wrote.
///
/// The call returns only when the device has finished with `output`. The raw Win32 code is
/// returned unaltered on failure, including `ERROR_MORE_DATA` and `ERROR_INSUFFICIENT_BUFFER`,
/// which a control that reports a size first is sized by.
///
/// # Panics
///
/// If `output` is longer than a `u32` can count, which no control this crate carries has.
///
/// # Aborts
///
/// If the wait for a call that is still running fails. That wait is on an event this function
/// created and owns, so it cannot fail in practice; but the device may still be about to write
/// into memory this function would otherwise let go of, and unwinding past that would be
/// undefined behaviour, where stopping the process is not.
pub(crate) fn device_control(device: HANDLE, code: u32, output: &mut [u8]) -> Outcome<u32> {
    let output_len = u32::try_from(output.len()).expect("a control's output buffer fits a u32");

    // SAFETY: no security attributes, an unnamed manual-reset event, unsignalled to start.
    let event =
        perform_nonnull_handle(|| unsafe { CreateEventW(ptr::null(), TRUE, FALSE, ptr::null()) })?;
    // SAFETY: a successful `CreateEventW` returns a handle this call owns and nothing else does.
    let event = unsafe { OwnedHandle::from_raw_handle(event) };

    // SAFETY: all-zero is a valid `OVERLAPPED`.
    let mut overlapped: OVERLAPPED = unsafe { mem::zeroed() };
    overlapped.hEvent = event.as_raw_handle().map_addr(|address| address | 1);

    let mut written = 0_u32;
    let started = perform_bool(|| {
        // SAFETY: `device` is a handle the caller holds open for this call. There is no input.
        // `output` is writable for `output_len` bytes, and `overlapped` and `written` live until
        // this function returns, which it does only after the device has finished: the call
        // completed here, or the wait below has.
        unsafe {
            DeviceIoControl(
                device,
                code,
                ptr::null(),
                0,
                output.as_mut_ptr().cast(),
                output_len,
                &raw mut written,
                &raw mut overlapped,
            )
        }
    });

    match started {
        Ok(()) => Ok(written),
        Err(error) if error.code() == ERROR_IO_PENDING => {
            // SAFETY: the event is owned and open. Its low bit was set only in the copy the
            // `OVERLAPPED` carries.
            let waited = perform(
                || unsafe { WaitForSingleObject(event.as_raw_handle(), INFINITE) },
                |result| *result == WAIT_FAILED,
            );
            if waited.is_err() {
                std::process::abort();
            }
            perform_bool(|| {
                // SAFETY: the same device and `OVERLAPPED` the call took, now complete, so this
                // does not wait.
                unsafe {
                    GetOverlappedResult(device, &raw const overlapped, &raw mut written, FALSE)
                }
            })?;
            Ok(written)
        }
        Err(error) => Err(error),
    }
}
