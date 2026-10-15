// Copyright (c) 2026 Mike Grier
//! A file opened by path and kept open.

use std::io;

use windows_overlapped_io_sys::{EndpointObserver, UnassociatedEndpoint};
use wtf_string::Wtf16Str;

use crate::OpenError;

#[cfg(test)]
mod tests;

/// A file opened by path, and kept open as an endpoint of windows-overlapped-io-sys.
///
/// Opened read-only, sharing read, write and delete -- so a writer can keep writing to the file,
/// and anyone can delete it, while it is held -- and for overlapped I/O, so reads can later
/// complete on the thread pool. The handle closes when the `HeldFile` and every
/// [`EndpointObserver`] taken from it have dropped.
#[derive(Debug)]
pub struct HeldFile {
    // Kept so the handle stays open and, from the iteration that reads bytes, so the endpoint is
    // the one thing that submits I/O on it.
    endpoint: UnassociatedEndpoint,
    start_offset: u64,
}

impl HeldFile {
    /// Open the file at `path` and hold it.
    ///
    /// The path is converted to the form the operating system takes without loss, so a name with
    /// an unpaired surrogate opens the file that has it.
    ///
    /// # Errors
    ///
    /// When the operating system refuses the open -- no such file, no such directory, a sharing
    /// violation, a directory where a file was named -- the error carries its Win32 code, read
    /// with [`io::Error::raw_os_error`]. A `path` containing a NUL is refused before any call is
    /// made, as [`io::ErrorKind::InvalidInput`] with no Win32 code.
    pub fn open(path: &Wtf16Str) -> io::Result<Self> {
        let endpoint = UnassociatedEndpoint::open(path.to_os_string(), true, false, 0)?;
        Ok(Self {
            endpoint,
            start_offset: 0,
        })
    }

    /// Open the file at `path`, hold it, and take `offset` as where the caller's reading starts.
    ///
    /// The offset is checked against the length read through the new handle, so a restarted
    /// parent whose recorded position is ahead of the file hears of it at once. An offset equal
    /// to the length is accepted.
    ///
    /// # Errors
    ///
    /// [`OpenError::Io`] as for [`open`](Self::open), and also when the length cannot be read.
    /// [`OpenError::OffsetBeyondLength`] when `offset` is past the end of the file.
    pub fn open_at(path: &Wtf16Str, offset: u64) -> Result<Self, OpenError> {
        let mut held = Self::open(path)?;
        let len = held.len()?;
        if offset > len {
            return Err(OpenError::OffsetBeyondLength { offset, len });
        }
        held.start_offset = offset;
        Ok(held)
    }

    /// The offset this file was opened at: 0 for [`open`](Self::open).
    #[must_use]
    pub fn start_offset(&self) -> u64 {
        self.start_offset
    }

    /// The file's current length in bytes, read through the held handle.
    ///
    /// The count is what has reached the file system: bytes a writer still buffers on its side
    /// are not in it. The file's path is not consulted, so the answer holds after the file has
    /// been renamed or deleted.
    ///
    /// # Errors
    ///
    /// When the operating system refuses the query, the error carries its Win32 code.
    #[allow(clippy::len_without_is_empty)] // the query can fail, so it is not a collection's len
    pub fn len(&self) -> io::Result<u64> {
        self.observer().file_len()
    }

    /// A new observer of this file's handle.
    ///
    /// It keeps the handle open after this `HeldFile` drops, and can never become a second
    /// endpoint.
    #[must_use]
    pub fn observer(&self) -> EndpointObserver {
        self.endpoint.observer()
    }
}
