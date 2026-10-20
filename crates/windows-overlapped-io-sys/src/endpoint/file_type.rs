// Copyright (c) 2026 Mike Grier
//! The observer's handle-type query, in every configuration: it classifies any
//! handle, not only a file system's.

use std::io;
use std::os::windows::io::AsRawHandle;

use windows_sys::Win32::Foundation::{GetLastError, NO_ERROR, SetLastError};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_TYPE_CHAR, FILE_TYPE_DISK, FILE_TYPE_PIPE, FILE_TYPE_REMOTE, FILE_TYPE_UNKNOWN,
    GetFileType,
};

use super::EndpointObserver;

#[cfg(test)]
mod tests;

/// What kind of object a handle refers to, as `GetFileType` reports it:
/// [`EndpointObserver::file_type`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FileType {
    /// A disk file or directory (`FILE_TYPE_DISK`).
    Disk,
    /// A character device, such as the NUL device or a console (`FILE_TYPE_CHAR`).
    Character,
    /// A named or anonymous pipe, or a socket (`FILE_TYPE_PIPE`).
    Pipe,
    /// `FILE_TYPE_REMOTE`, which Win32 documents as unused.
    Remote,
    /// `FILE_TYPE_UNKNOWN` with no error: the type is not one Win32 names.
    Unknown,
    /// A value Win32 does not document, carried as reported.
    Other(u32),
}

impl EndpointObserver {
    /// What kind of object the handle refers to: a disk file, a character
    /// device, a pipe, or other.
    ///
    /// Read with `GetFileType`, which is synchronous and issues no I/O, so it is
    /// safe while the endpoint has operations in flight.
    ///
    /// # Errors
    ///
    /// Returns the error `GetFileType` reports with `FILE_TYPE_UNKNOWN`.
    pub fn file_type(&self) -> io::Result<FileType> {
        // SAFETY: SetLastError and GetLastError only touch this thread's last-error
        // value, and GetFileType takes a live handle this observer holds.
        let (raw, error) = unsafe {
            // Cleared first, so a stale error cannot pass for this call's.
            SetLastError(NO_ERROR);
            let raw = GetFileType(self.handle.as_raw_handle());
            (raw, GetLastError())
        };
        classify(raw, error)
    }
}

/// Map `GetFileType`'s return value, and the last error read straight after it,
/// to a [`FileType`].
fn classify(raw: u32, last_error: u32) -> io::Result<FileType> {
    Ok(match raw {
        FILE_TYPE_DISK => FileType::Disk,
        FILE_TYPE_CHAR => FileType::Character,
        FILE_TYPE_PIPE => FileType::Pipe,
        FILE_TYPE_REMOTE => FileType::Remote,
        FILE_TYPE_UNKNOWN if last_error != NO_ERROR => {
            return Err(io::Error::from_raw_os_error(last_error.cast_signed()));
        }
        FILE_TYPE_UNKNOWN => FileType::Unknown,
        other => FileType::Other(other),
    })
}
