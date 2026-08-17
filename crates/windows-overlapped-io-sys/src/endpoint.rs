// Copyright (c) 2026 Mike Grier
//! Owned overlapped-capable endpoints and controlled association provenance.
//!
//! An endpoint is a Windows handle that has been established for overlapped I/O
//! but not yet associated with a completion backend. Association is a later,
//! consuming transition; this module models only ownership and the provenance
//! that must hold before an endpoint may be trusted for safe completion routing.

use std::fs::OpenOptions;
use std::io;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsHandle, BorrowedHandle, OwnedHandle};
use std::path::Path;

/// The Win32 `FILE_FLAG_OVERLAPPED` flag. Changing this value is a breaking change.
const FILE_FLAG_OVERLAPPED: u32 = 0x4000_0000;

/// An overlapped-capable endpoint that has not yet been associated with a
/// completion backend.
///
/// The endpoint owns its handle and closes it on drop. It is intentionally not
/// `Clone`: a second owner could route completions through a duplicate handle
/// and break the single-association invariant that the completion backends rely
/// on.
#[derive(Debug)]
pub struct UnassociatedEndpoint {
    handle: OwnedHandle,
}

impl UnassociatedEndpoint {
    /// Open a filesystem path (file, directory, or device) for overlapped I/O
    /// and wrap it as an endpoint, establishing overlapped provenance safely.
    ///
    /// The handle is always opened with `FILE_FLAG_OVERLAPPED`, so the overlapped
    /// invariant holds without the unsafe [`UnassociatedEndpoint::assume_overlapped`]
    /// seam. Pass any additional `FILE_FLAG_*` bits in `extra_flags` -- for
    /// example the backup-semantics flag to open a directory handle for change
    /// notifications. Set `read` and/or `write` for the access the operations
    /// will need.
    ///
    /// # Errors
    ///
    /// Returns any error encountered opening the path.
    pub fn open(
        path: impl AsRef<Path>,
        read: bool,
        write: bool,
        extra_flags: u32,
    ) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(read)
            .write(write)
            .custom_flags(FILE_FLAG_OVERLAPPED | extra_flags)
            .open(path)?;
        // SAFETY: the handle was just opened with FILE_FLAG_OVERLAPPED, is fresh
        // and unassociated, has no duplicates, and ownership moves in exclusively.
        Ok(unsafe { Self::assume_overlapped(OwnedHandle::from(file)) })
    }

    /// Wrap an owned handle whose overlapped provenance the caller vouches for.
    ///
    /// This is the narrow unsafe extensibility seam for handles the crate cannot
    /// create itself -- sockets, devices, or handles obtained elsewhere. For
    /// filesystem paths, prefer the safe [`UnassociatedEndpoint::open`].
    ///
    /// # Safety
    ///
    /// The caller guarantees that:
    ///
    /// - the handle was opened for overlapped I/O (for example a file opened
    ///   with `FILE_FLAG_OVERLAPPED`, or an overlapped-capable socket handle);
    /// - the handle is not already associated with any completion port;
    /// - no duplicate of the handle exists that could generate competing
    ///   completions for the same operations; and
    /// - ownership is transferred exclusively into the returned endpoint.
    #[must_use]
    pub unsafe fn assume_overlapped(handle: OwnedHandle) -> Self {
        Self { handle }
    }

    /// Borrow the underlying handle for the duration of a native call.
    ///
    /// The borrow cannot outlive the endpoint and does not confer ownership, so
    /// it cannot be used to establish a competing completion association.
    #[must_use]
    pub fn handle(&self) -> BorrowedHandle<'_> {
        self.handle.as_handle()
    }

    /// Consume the endpoint and recover the owned handle.
    ///
    /// This abandons the overlapped-endpoint invariants; the recovered handle is
    /// an ordinary [`OwnedHandle`] again.
    #[must_use]
    pub fn into_handle(self) -> OwnedHandle {
        self.handle
    }
}

#[cfg(test)]
mod tests;
