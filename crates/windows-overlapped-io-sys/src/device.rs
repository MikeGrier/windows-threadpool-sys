// Copyright (c) 2026 Mike Grier
//! Safe device-control operation adapters, gated behind the `device` feature.
//!
//! These wrappers own the input and output buffers and issue the single native
//! `DeviceIoControl` internally, so a caller performs an overlapped device
//! control without touching `OVERLAPPED`, the submission seam, or `unsafe`. A
//! device is a `HANDLE`, so the adapters extend the existing handle endpoints
//! rather than introducing a device-specific type.

use std::ffi::c_void;
use std::io;
use std::os::windows::io::AsRawHandle;

use windows_sys::Win32::Foundation::ERROR_IO_PENDING;
use windows_sys::Win32::System::IO::DeviceIoControl;

use crate::{BlockingEndpoint, Operation};

impl BlockingEndpoint {
    /// Issue an overlapped `DeviceIoControl` with control code `code`, blocking
    /// until it completes.
    ///
    /// `input` is the input buffer (empty for control codes that take none) and
    /// `output_len` sizes the output buffer. Returns the output truncated to the
    /// bytes returned and that count.
    ///
    /// # Errors
    ///
    /// Returns any error from issuing or completing the control operation.
    pub fn ioctl(
        &self,
        code: u32,
        input: &[u8],
        output_len: usize,
    ) -> io::Result<(Vec<u8>, usize)> {
        let mut output = vec![0_u8; output_len];
        let (in_ptr, in_len) = in_buf(input);
        let (out_ptr, out_len) = out_buf(&mut output);

        let mut operation = Operation::new(());
        // SAFETY: issues exactly one DeviceIoControl reading `input` and writing
        // `output`, both valid for the whole blocking call; no other operation is
        // outstanding.
        let returned = unsafe {
            self.run(&mut operation, |handle, overlapped| {
                let ok = DeviceIoControl(
                    handle.as_raw_handle(),
                    code,
                    in_ptr,
                    in_len,
                    out_ptr,
                    out_len,
                    std::ptr::null_mut(),
                    overlapped,
                );
                classify(ok)
            })
        }?;

        output.truncate(returned);
        Ok((output, returned))
    }
}

/// The input buffer pointer and size, `NULL`/0 for an empty buffer.
fn in_buf(input: &[u8]) -> (*const c_void, u32) {
    if input.is_empty() {
        (std::ptr::null(), 0)
    } else {
        (input.as_ptr().cast(), clamp_u32(input.len()))
    }
}

/// The output buffer pointer and size, `NULL`/0 for an empty buffer.
fn out_buf(output: &mut [u8]) -> (*mut c_void, u32) {
    if output.is_empty() {
        (std::ptr::null_mut(), 0)
    } else {
        (output.as_mut_ptr().cast(), clamp_u32(output.len()))
    }
}

/// Map a native `BOOL` into the submission-seam contract: native success or
/// `ERROR_IO_PENDING` is accepted, any other error is an immediate failure.
fn classify(ok: i32) -> io::Result<()> {
    if ok != 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(ERROR_IO_PENDING as i32) {
        Ok(())
    } else {
        Err(error)
    }
}

/// Clamp a length to the `u32` byte-count parameter the Win32 call takes.
fn clamp_u32(len: usize) -> u32 {
    u32::try_from(len).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests;
