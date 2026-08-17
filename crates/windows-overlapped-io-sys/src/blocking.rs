// Copyright (c) 2026 Mike Grier
//! Blocking backend: complete one overlapped operation at a time by waiting on
//! the handle with `GetOverlappedResult`.
//!
//! This is the backend for overlapped endpoints that are not associated with a
//! completion port. Because it waits on the handle itself to signal completion,
//! it supports at most one outstanding operation at a time and completes it
//! synchronously, so it needs neither ownership transfer nor rundown.

use std::io;
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, OwnedHandle};

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::IO::{GetOverlappedResult, OVERLAPPED};

use crate::{Operation, OperationState, UnassociatedEndpoint};

/// An overlapped endpoint that completes operations synchronously, one at a
/// time, via `GetOverlappedResult`.
#[derive(Debug)]
pub struct BlockingEndpoint {
    handle: OwnedHandle,
}

impl BlockingEndpoint {
    /// Take ownership of an overlapped endpoint for synchronous completion.
    #[must_use]
    pub fn new(endpoint: UnassociatedEndpoint) -> Self {
        Self {
            handle: endpoint.into_handle(),
        }
    }

    /// Borrow the underlying handle for issuing native operations.
    #[must_use]
    pub fn handle(&self) -> BorrowedHandle<'_> {
        self.handle.as_handle()
    }

    /// Issue one overlapped operation and block until it completes, returning the
    /// number of bytes transferred.
    ///
    /// `issue` performs the native call with the operation's `OVERLAPPED`
    /// pointer, returning `Ok` when the operation was accepted (native success or
    /// `ERROR_IO_PENDING`) and `Err` on an immediate failure.
    ///
    /// # Safety
    ///
    /// `issue` must start exactly one overlapped operation using the provided
    /// `OVERLAPPED` pointer and no other storage, and no other operation may be
    /// outstanding on this endpoint until this call returns. Any buffers the
    /// operation reads or writes must stay valid for the duration of the call.
    pub unsafe fn run<P, F>(&self, operation: &mut Operation<P>, issue: F) -> io::Result<usize>
    where
        F: FnOnce(BorrowedHandle<'_>, *mut OVERLAPPED) -> io::Result<()>,
    {
        operation.set_state(OperationState::Submitted);
        let overlapped = operation.overlapped_ptr();
        issue(self.handle(), overlapped)?;
        operation.set_state(OperationState::Pending);

        let mut transferred: u32 = 0;
        // SAFETY: the handle and overlapped are valid; a non-zero `wait` blocks
        // on the handle until this single operation completes.
        let ok = unsafe { GetOverlappedResult(self.raw_handle(), overlapped, &mut transferred, 1) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        operation.set_state(OperationState::Completed);
        Ok(transferred as usize)
    }

    fn raw_handle(&self) -> HANDLE {
        self.handle.as_raw_handle()
    }
}
