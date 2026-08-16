//! Raw I/O completion port backend: port ownership, association, and dequeue.
//!
//! A [`CompletionPort`] owns a completion-port handle and can service many
//! endpoints, each associated with a caller-chosen completion key. Association
//! is the consuming transition that binds an [`UnassociatedEndpoint`] to this
//! backend. The port does not create worker threads; the owner decides where
//! [`CompletionPort::get`] runs. Submission of real overlapped operations, and
//! the reclamation that follows their completion, are built on top of this
//! module.

use std::io;
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle};

use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE, WAIT_TIMEOUT};
use windows_sys::Win32::System::IO::{
    CreateIoCompletionPort, GetQueuedCompletionStatus, OVERLAPPED, PostQueuedCompletionStatus,
};

use crate::UnassociatedEndpoint;

/// An owned I/O completion port.
#[derive(Debug)]
pub struct CompletionPort {
    handle: OwnedHandle,
}

impl CompletionPort {
    /// Create a new completion port.
    ///
    /// `concurrency` is the maximum number of threads the system lets run
    /// completions for this port concurrently; zero means one per processor.
    pub fn new(concurrency: u32) -> io::Result<Self> {
        // SAFETY: creating a fresh port with no associated file handle.
        let handle = unsafe {
            CreateIoCompletionPort(INVALID_HANDLE_VALUE, std::ptr::null_mut(), 0, concurrency)
        };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the call returned a fresh, exclusively owned port handle.
        let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
        Ok(Self { handle })
    }

    /// Associate an overlapped endpoint with this port under `key`.
    ///
    /// Completions for operations issued on the endpoint are delivered to this
    /// port and tagged with `key`. The association is permanent for the life of
    /// the handle, so the returned endpoint borrows the port.
    pub fn associate(
        &self,
        endpoint: UnassociatedEndpoint,
        key: usize,
    ) -> io::Result<AssociatedEndpoint<'_>> {
        let handle = endpoint.into_handle();
        // SAFETY: associating a valid handle with a valid port; the concurrency
        // argument is ignored when an existing port is supplied.
        let result = unsafe { CreateIoCompletionPort(handle.as_raw_handle(), self.raw(), key, 0) };
        if result.is_null() {
            return Err(io::Error::last_os_error());
        }
        Ok(AssociatedEndpoint {
            port: self,
            handle,
            key,
        })
    }

    /// Post a user-defined completion packet to this port.
    ///
    /// The `overlapped` value is delivered verbatim and is not dereferenced, so
    /// it may be null or any sentinel the caller uses to distinguish wakeups.
    pub fn post(
        &self,
        key: usize,
        bytes_transferred: u32,
        overlapped: *mut OVERLAPPED,
    ) -> io::Result<()> {
        // SAFETY: the port handle is valid; the overlapped value is opaque here.
        let ok = unsafe {
            PostQueuedCompletionStatus(self.raw(), bytes_transferred, key, overlapped.cast_const())
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Dequeue one completion packet, waiting up to `timeout_ms` milliseconds.
    ///
    /// Returns `Ok(None)` when the wait times out with no packet. A packet is
    /// returned even when its operation failed; the failure is reported through
    /// [`Completion::error`].
    pub fn get(&self, timeout_ms: u32) -> io::Result<Option<Completion>> {
        let mut bytes_transferred: u32 = 0;
        let mut key: usize = 0;
        let mut overlapped: *mut OVERLAPPED = std::ptr::null_mut();
        // SAFETY: all out-parameters are valid for the duration of the call.
        let ok = unsafe {
            GetQueuedCompletionStatus(
                self.raw(),
                &mut bytes_transferred,
                &mut key,
                &mut overlapped,
                timeout_ms,
            )
        };
        if ok != 0 {
            return Ok(Some(Completion {
                key,
                bytes_transferred,
                overlapped,
                error: None,
            }));
        }

        let error = io::Error::last_os_error();
        if overlapped.is_null() {
            if error.raw_os_error() == Some(WAIT_TIMEOUT as i32) {
                return Ok(None);
            }
            return Err(error);
        }
        // A packet for a failed operation was dequeued.
        Ok(Some(Completion {
            key,
            bytes_transferred,
            overlapped,
            error: Some(error),
        }))
    }

    fn raw(&self) -> HANDLE {
        self.handle.as_raw_handle()
    }
}

/// An overlapped endpoint bound to exactly one [`CompletionPort`].
///
/// The endpoint owns its handle and borrows the port it is associated with, so
/// the port cannot be dropped while any endpoint still routes completions to it.
/// It is intentionally not `Clone`.
#[derive(Debug)]
pub struct AssociatedEndpoint<'port> {
    port: &'port CompletionPort,
    handle: OwnedHandle,
    key: usize,
}

impl<'port> AssociatedEndpoint<'port> {
    /// Borrow the underlying handle for issuing native operations.
    #[must_use]
    pub fn handle(&self) -> BorrowedHandle<'_> {
        self.handle.as_handle()
    }

    /// The completion key packets from this endpoint are tagged with.
    #[must_use]
    pub fn key(&self) -> usize {
        self.key
    }

    /// The completion port this endpoint is associated with.
    #[must_use]
    pub fn port(&self) -> &'port CompletionPort {
        self.port
    }
}

/// A completion packet dequeued from a [`CompletionPort`].
#[derive(Debug)]
pub struct Completion {
    key: usize,
    bytes_transferred: u32,
    overlapped: *mut OVERLAPPED,
    error: Option<io::Error>,
}

impl Completion {
    /// The completion key the packet was tagged with.
    #[must_use]
    pub fn key(&self) -> usize {
        self.key
    }

    /// The number of bytes transferred by the operation.
    #[must_use]
    pub fn bytes_transferred(&self) -> u32 {
        self.bytes_transferred
    }

    /// The `OVERLAPPED` pointer identifying the completed operation.
    ///
    /// For a user packet this is whatever value was passed to
    /// [`CompletionPort::post`].
    #[must_use]
    pub fn overlapped_ptr(&self) -> *mut OVERLAPPED {
        self.overlapped
    }

    /// The failure of the completed operation, if it did not succeed.
    #[must_use]
    pub fn error(&self) -> Option<&io::Error> {
        self.error.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::CompletionPort;
    use crate::UnassociatedEndpoint;
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::OwnedHandle;

    const FILE_FLAG_OVERLAPPED: u32 = 0x4000_0000;

    #[test]
    fn posts_and_dequeues_a_user_packet() {
        let port = CompletionPort::new(0).expect("create port");
        let sentinel = 0x1234_usize as *mut _;

        port.post(0xABCD, 42, sentinel).expect("post packet");
        let completion = port.get(1_000).expect("get packet").expect("a packet");

        assert_eq!(completion.key(), 0xABCD);
        assert_eq!(completion.bytes_transferred(), 42);
        assert_eq!(completion.overlapped_ptr(), sentinel);
        assert!(completion.error().is_none());
    }

    #[test]
    fn get_times_out_when_empty() {
        let port = CompletionPort::new(0).expect("create port");
        assert!(port.get(0).expect("get").is_none());
    }

    #[test]
    fn associates_an_overlapped_handle() {
        let port = CompletionPort::new(0).expect("create port");
        let path = std::env::temp_dir().join(format!(
            "windows-overlapped-io-sys-iocp-{}.tmp",
            std::process::id()
        ));
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .custom_flags(FILE_FLAG_OVERLAPPED)
            .open(&path)
            .expect("create overlapped temp file");
        let owned = OwnedHandle::from(file);

        // SAFETY: the file was just created with FILE_FLAG_OVERLAPPED, is not
        // associated with any port, has no duplicates, and moves in exclusively.
        let endpoint = unsafe { UnassociatedEndpoint::assume_overlapped(owned) };
        let associated = port.associate(endpoint, 0x55).expect("associate");
        assert_eq!(associated.key(), 0x55);

        drop(associated);
        let _ = std::fs::remove_file(&path);
    }
}
