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

/// The `SetFileCompletionNotificationModes` flag bits.
///
/// `windows-sys` does not export these, so they are named here rather than
/// written as bare literals at the call site. Changing either value is a
/// breaking change.
pub(crate) mod notification_flags {
    /// `FILE_SKIP_COMPLETION_PORT_ON_SUCCESS`.
    pub(crate) const SKIP_COMPLETION_PORT_ON_SUCCESS: u8 = 0x1;
    /// `FILE_SKIP_SET_EVENT_ON_HANDLE`.
    pub(crate) const SKIP_SET_EVENT_ON_HANDLE: u8 = 0x2;
}

/// Which completion-notification shortcuts a handle should take.
///
/// These are the two `SetFileCompletionNotificationModes` flags. Both trade a
/// notification the I/O Manager would otherwise deliver for the cost of not
/// having it, so both are opt-in per endpoint rather than anything this crate
/// chooses on a caller's behalf.
///
/// Every field defaults to `false`, which is the handle's ordinary behaviour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct NotificationModes {
    /// Suppress the completion packet for an operation that succeeds
    /// immediately, rather than queueing one the caller must still dequeue.
    ///
    /// This is the throughput knob. Ordinarily an IOCP-associated overlapped
    /// handle gets a packet for *every* completed request, including one that
    /// returned success without ever returning `ERROR_IO_PENDING` -- see
    /// [`crate::Issued::Pending`]. Setting this removes the queue, the dequeue,
    /// and the worker wakeup for each such operation, which is a real win where
    /// operations frequently complete synchronously (cached reads, small socket
    /// sends, loopback) and changes nothing for ones that genuinely go
    /// asynchronous.
    ///
    /// The cost is that a submission now has two possible shapes, so every
    /// adapter on this endpoint reports [`crate::Started::Completed`] on the
    /// synchronous path instead of a claim-later token. A caller that does not
    /// handle that arm will lose results.
    pub skip_completion_port_on_success: bool,
    /// Do not set the file object's own event for a request that returns
    /// success, or that returns `ERROR_IO_PENDING` from an asynchronous call.
    ///
    /// Independent of the completion port: it concerns the handle's internal
    /// event, which completion-port-driven code does not wait on. An event
    /// supplied explicitly in the `OVERLAPPED` is still signalled.
    ///
    /// **Do not set this on an endpoint destined for
    /// [`crate::BlockingEndpoint`]**, which waits on exactly that internal
    /// event: suppressing it leaves the wait with nothing to wake it.
    pub skip_set_event_on_handle: bool,
}

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
    /// What [`UnassociatedEndpoint::set_notification_modes`] has established on
    /// the handle. Carried with the endpoint, and onward into association,
    /// because the submission seam has to answer "will a completion packet
    /// arrive" and skip-on-success is what changes that answer.
    modes: NotificationModes,
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
    ///   completions for the same operations;
    /// - ownership is transferred exclusively into the returned endpoint; and
    /// - **any completion-notification mode already set on the handle is
    ///   declared** through [`UnassociatedEndpoint::set_notification_modes`].
    ///   The endpoint is assumed to be in the default mode, and the submission
    ///   seam relies on that to decide whether a completion packet will arrive;
    ///   a handle silently in `FILE_SKIP_COMPLETION_PORT_ON_SUCCESS` mode would
    ///   have its synchronous successes reported as pending, leaving operations
    ///   outstanding forever. Re-declaring is safe: the call is additive and
    ///   idempotent.
    #[must_use]
    pub unsafe fn assume_overlapped(handle: OwnedHandle) -> Self {
        Self {
            handle,
            modes: NotificationModes::default(),
        }
    }

    /// Borrow the underlying handle for the duration of a native call.
    ///
    /// The borrow cannot outlive the endpoint and does not confer ownership, so
    /// it cannot be used to establish a competing completion association.
    #[must_use]
    pub fn handle(&self) -> BorrowedHandle<'_> {
        self.handle.as_handle()
    }

    /// The completion-notification modes established on this endpoint.
    #[must_use]
    pub fn notification_modes(&self) -> NotificationModes {
        self.modes
    }

    /// Consume the endpoint and recover the owned handle.
    ///
    /// This abandons the overlapped-endpoint invariants; the recovered handle is
    /// an ordinary [`OwnedHandle`] again. Any notification mode set on it stays
    /// set -- Win32 offers no way to clear one -- so a handle rewrapped later
    /// must re-declare it.
    #[must_use]
    pub fn into_handle(self) -> OwnedHandle {
        self.handle
    }

    /// Set this endpoint's completion-notification modes before it is
    /// associated with a backend.
    ///
    /// Setting the mode here rather than after association is deliberate: it is
    /// an attribute of the endpoint's provenance, so an endpoint carries it from
    /// the moment it exists and no operation can ever be issued against a handle
    /// whose notification behaviour is still in question.
    ///
    /// Passing every field `false` is a no-op call, not a reset. **A mode cannot
    /// be removed once set** -- that is a Win32 property of the handle, not a
    /// limitation of this wrapper -- so a second call can only ever add modes.
    ///
    /// [`NotificationModes::skip_completion_port_on_success`] takes effect only
    /// once all three of Win32's conditions hold: the handle is associated with
    /// a completion port, it was opened for asynchronous I/O (this type
    /// guarantees that), and the request returns success immediately. Until the
    /// association exists the flag is simply inert, which is why setting it
    /// first is safe.
    ///
    /// Sockets set their modes elsewhere, on `AssociatedSocket::set_notification_modes`
    /// (behind the `socket` feature, so not always linkable here): they have no
    /// unassociated stage to hang provenance on, and Win32 additionally
    /// restricts skip-on-success to Layered Service Providers that return IFS
    /// handles, so that setter probes the socket's own provider rather than
    /// setting the flag blind.
    ///
    /// # Errors
    ///
    /// Returns any error from `SetFileCompletionNotificationModes`, which
    /// reports `ERROR_INVALID_PARAMETER` for a handle whose device does not
    /// support the requested mode.
    pub fn set_notification_modes(&mut self, modes: NotificationModes) -> io::Result<()> {
        use std::os::windows::io::AsRawHandle;

        let mut flags = 0_u8;
        if modes.skip_completion_port_on_success {
            flags |= notification_flags::SKIP_COMPLETION_PORT_ON_SUCCESS;
        }
        if modes.skip_set_event_on_handle {
            flags |= notification_flags::SKIP_SET_EVENT_ON_HANDLE;
        }
        // SAFETY: a live handle this endpoint owns, and a flags byte built only
        // from the two documented bits. The call sets a handle attribute and
        // starts no I/O, so it borrows nothing beyond this statement.
        let ok = unsafe {
            windows_sys::Win32::Storage::FileSystem::SetFileCompletionNotificationModes(
                self.handle.as_raw_handle(),
                flags,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        // Accumulated, never replaced: Win32 cannot clear a mode, so what this
        // endpoint records has to be the union of everything ever set on it.
        self.modes.skip_completion_port_on_success |= modes.skip_completion_port_on_success;
        self.modes.skip_set_event_on_handle |= modes.skip_set_event_on_handle;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
