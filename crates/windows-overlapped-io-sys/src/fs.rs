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

use crate::operation::payload_ptr_from_overlapped;
use crate::{
    AssociatedEndpoint, BlockingEndpoint, Completion, Issued, Operation, OperationId, Submitted,
};

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

impl AssociatedEndpoint<'_> {
    /// Submit an overlapped read of up to `len` bytes starting at `offset`,
    /// returning a [`FileIo`] token that recovers the buffer and byte count from
    /// the operation's completion.
    ///
    /// The endpoint must not be in `FILE_SKIP_COMPLETION_PORT_ON_SUCCESS` mode;
    /// this adapter always expects a completion packet to arrive.
    ///
    /// # Errors
    ///
    /// Returns any immediate failure from issuing the read.
    #[track_caller]
    pub fn read(&self, len: usize, offset: u64) -> io::Result<FileIo> {
        let buf_len = clamp_u32(len);
        let mut operation = Operation::new(vec![0_u8; len]);
        operation.set_offset(offset);
        // SAFETY: issues exactly one ReadFile into the operation's own payload
        // buffer, reached through the pinned OVERLAPPED; the payload lives until
        // the completion is claimed.
        let submitted = unsafe {
            self.submit(operation, |handle, overlapped| {
                let payload = payload_ptr_from_overlapped::<Vec<u8>>(overlapped);
                let ok = ReadFile(
                    handle.as_raw_handle(),
                    (*payload).as_mut_ptr(),
                    buf_len,
                    std::ptr::null_mut(),
                    overlapped,
                );
                classify_issued(ok)
            })
        };
        finish(submitted)
    }

    /// Submit an overlapped write of `data` starting at `offset`, returning a
    /// [`FileIo`] token that recovers the buffer and byte count from the
    /// operation's completion.
    ///
    /// The endpoint must not be in `FILE_SKIP_COMPLETION_PORT_ON_SUCCESS` mode.
    ///
    /// # Errors
    ///
    /// Returns any immediate failure from issuing the write.
    #[track_caller]
    pub fn write(&self, data: Vec<u8>, offset: u64) -> io::Result<FileIo> {
        let data_len = clamp_u32(data.len());
        let mut operation = Operation::new(data);
        operation.set_offset(offset);
        // SAFETY: issues exactly one WriteFile from the operation's own payload
        // buffer, reached through the pinned OVERLAPPED; the payload lives until
        // the completion is claimed.
        let submitted = unsafe {
            self.submit(operation, |handle, overlapped| {
                let payload = payload_ptr_from_overlapped::<Vec<u8>>(overlapped);
                let ok = WriteFile(
                    handle.as_raw_handle(),
                    (*payload).as_ptr(),
                    data_len,
                    std::ptr::null_mut(),
                    overlapped,
                );
                classify_issued(ok)
            })
        };
        finish(submitted)
    }
}

/// Map a native `BOOL` into the IOCP submission contract, expecting a completion
/// packet on success because the adapter never enables skip-on-success mode.
fn classify_issued(ok: i32) -> io::Result<Issued> {
    if ok != 0 {
        return Ok(Issued::Pending);
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(ERROR_IO_PENDING as i32) {
        Ok(Issued::Pending)
    } else {
        Err(error)
    }
}

/// Turn a submission outcome into a [`FileIo`] token or an immediate error.
fn finish(submitted: Submitted<Vec<u8>>) -> io::Result<FileIo> {
    match submitted {
        Submitted::Pending(id) => Ok(FileIo { id }),
        Submitted::Completed { .. } => Err(io::Error::other(
            "file adapter observed a synchronous completion; the endpoint must not be in \
             FILE_SKIP_COMPLETION_PORT_ON_SUCCESS mode",
        )),
        Submitted::Failed { error, .. } => Err(error),
    }
}

/// A pending file operation submitted through [`AssociatedEndpoint::read`] or
/// [`AssociatedEndpoint::write`].
///
/// The token carries the operation's identity and its `Vec<u8>` payload type, so
/// [`FileIo::claim`] recovers the buffer and byte count safely once the matching
/// completion is dequeued.
#[derive(Debug)]
pub struct FileIo {
    id: OperationId,
}

impl FileIo {
    /// The identity of the in-flight operation, for cancellation or matching.
    #[must_use]
    pub fn id(&self) -> OperationId {
        self.id
    }

    /// Claim this operation's result from `completion`.
    ///
    /// On a match returns `Ok((buffer, result))`: `buffer` is the payload -- the
    /// bytes read, or the data written -- and `result` is the byte count or the
    /// operation's error. Returns `Err(self)` when `completion` belongs to a
    /// different operation, so the caller can try the token against another one.
    pub fn claim(self, completion: &Completion) -> Result<(Vec<u8>, io::Result<usize>), Self> {
        if completion.overlapped_ptr() != self.id.as_ptr() {
            return Err(self);
        }
        // SAFETY: the identity match proves this completion is the
        // Operation<Vec<u8>> this token submitted; claim it exactly once.
        let operation = unsafe { completion.claim::<Vec<u8>>() };
        let buffer = operation.into_payload();
        let result = match completion.error() {
            Some(error) => Err(io::Error::from_raw_os_error(
                error.raw_os_error().unwrap_or_default(),
            )),
            None => Ok(completion.bytes_transferred() as usize),
        };
        Ok((buffer, result))
    }
}

#[cfg(test)]
mod tests;
