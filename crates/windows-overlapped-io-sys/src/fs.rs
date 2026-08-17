// Copyright (c) 2026 Mike Grier
//! Safe file-family operation adapters, gated behind the `fs` feature.
//!
//! These wrappers own the I/O buffer and issue the single native `ReadFile` /
//! `WriteFile` internally, so a caller performs file overlapped I/O without
//! touching `OVERLAPPED`, the submission seam, or `unsafe`. They are the file
//! family's realization of the per-family safe-adapter decision; other families
//! follow the same shape.

use std::io;
use std::os::windows::io::AsRawHandle;

use windows_sys::Win32::Foundation::ERROR_IO_PENDING;
use windows_sys::Win32::Storage::FileSystem::{ReadFile, WriteFile};

use crate::{BlockingEndpoint, Operation};

impl BlockingEndpoint {
    /// Read up to `len` bytes starting at `offset`, blocking until the read
    /// completes.
    ///
    /// Returns the buffer truncated to the bytes actually read, together with
    /// that count. The whole operation finishes within this call, so no
    /// `OVERLAPPED` or `unsafe` reaches the caller.
    ///
    /// # Errors
    ///
    /// Returns any error from issuing or completing the read.
    pub fn read(&self, len: usize, offset: u64) -> io::Result<(Vec<u8>, usize)> {
        let mut buffer = vec![0_u8; len];
        let buf_ptr = buffer.as_mut_ptr();
        let buf_len = clamp_u32(len);

        let mut operation = Operation::new(());
        operation.set_offset(offset);
        // SAFETY: issues exactly one overlapped ReadFile into `buffer`, which
        // outlives this blocking call; no other operation is outstanding.
        let read = unsafe {
            self.run(&mut operation, |handle, overlapped| {
                let ok = ReadFile(
                    handle.as_raw_handle(),
                    buf_ptr,
                    buf_len,
                    std::ptr::null_mut(),
                    overlapped,
                );
                classify(ok)
            })
        }?;

        buffer.truncate(read);
        Ok((buffer, read))
    }

    /// Write `data` starting at `offset`, blocking until the write completes, and
    /// return the number of bytes written.
    ///
    /// # Errors
    ///
    /// Returns any error from issuing or completing the write.
    pub fn write(&self, data: &[u8], offset: u64) -> io::Result<usize> {
        let data_ptr = data.as_ptr();
        let data_len = clamp_u32(data.len());

        let mut operation = Operation::new(());
        operation.set_offset(offset);
        // SAFETY: issues exactly one overlapped WriteFile from `data`, which
        // outlives this blocking call; no other operation is outstanding.
        let written = unsafe {
            self.run(&mut operation, |handle, overlapped| {
                let ok = WriteFile(
                    handle.as_raw_handle(),
                    data_ptr,
                    data_len,
                    std::ptr::null_mut(),
                    overlapped,
                );
                classify(ok)
            })
        }?;

        Ok(written)
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

/// Clamp a length to the `u32` byte-count parameter the Win32 calls take.
fn clamp_u32(len: usize) -> u32 {
    u32::try_from(len).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests;
