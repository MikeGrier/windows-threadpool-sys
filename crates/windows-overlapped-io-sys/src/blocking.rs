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

#[cfg(test)]
mod tests;

/// An overlapped endpoint that completes operations synchronously, one at a
/// time, via `GetOverlappedResult`.
///
/// "One at a time" is enforced, not merely documented: every safe adapter on
/// this type takes `&mut self`, so a second operation while one is in flight is
/// a borrow-check error. That matters because `GetOverlappedResult` waits on the
/// *handle*, which is signalled by whichever operation completes -- with two
/// outstanding, a call could return the other one's result and hand back buffers
/// the kernel is still writing into.
///
/// The type is still `Send + Sync`, so an endpoint can be moved between threads
/// or shared behind a `Mutex`; what it cannot do is have two operations in
/// flight, which is what the mutual exclusion buys.
///
/// One owner issuing operations in sequence is the supported shape; sharing one
/// endpoint across threads and operating from both is rejected at compile time
/// rather than corrupting a result at run time, since every operation method
/// takes `&mut self` while an `Arc` hands out only `&BlockingEndpoint`. See the
/// `read` method (available with the `fs` feature) for runnable examples of
/// both -- the examples live there because they call `read`, which the `fs`
/// feature provides, so they compile in every configuration that has it.
#[derive(Debug)]
pub struct BlockingEndpoint {
    handle: OwnedHandle,
}

impl BlockingEndpoint {
    /// Take ownership of an overlapped endpoint for synchronous completion.
    ///
    /// # Errors
    ///
    /// Returns [`TryFromEndpointError`], recoverable back into `endpoint` via
    /// [`TryFromEndpointError::into_endpoint`], if `endpoint` has
    /// [`NotificationModes::skip_set_event_on_handle`](crate::NotificationModes::skip_set_event_on_handle)
    /// set (PR #20 review response). `run` below waits on the handle's own
    /// internal event via `GetOverlappedResult`, which is exactly the
    /// notification that mode suppresses -- constructing a `BlockingEndpoint`
    /// from such an endpoint would have no wakeup source for a genuinely
    /// pending (`ERROR_IO_PENDING`) operation and could block forever. Win32
    /// offers no way to clear the mode once set (see
    /// [`UnassociatedEndpoint::into_handle`]), so this is the one place the
    /// incompatibility can be caught, and it is checked here rather than left
    /// as a documentation-only warning.
    pub fn new(endpoint: UnassociatedEndpoint) -> Result<Self, TryFromEndpointError> {
        if endpoint.notification_modes().skip_set_event_on_handle {
            return Err(TryFromEndpointError { endpoint });
        }
        Ok(Self {
            handle: endpoint.into_handle(),
        })
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
    ///
    /// This takes `&self` rather than `&mut self` so a caller driving the raw
    /// seam can hold other borrows of the endpoint; the exclusivity requirement
    /// is theirs to uphold, which is what makes this `unsafe`. The safe adapters
    /// built on it take `&mut self` instead, so they cannot violate it.
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

/// [`BlockingEndpoint::new`]'s rejection: `endpoint` has
/// [`crate::NotificationModes::skip_set_event_on_handle`] set, which is
/// incompatible with the blocking backend (see `new`'s docs). Carries the
/// endpoint back so a caller that constructed it in error loses nothing.
#[derive(Debug)]
pub struct TryFromEndpointError {
    endpoint: UnassociatedEndpoint,
}

impl TryFromEndpointError {
    /// Recover the endpoint this rejection carries.
    #[must_use]
    pub fn into_endpoint(self) -> UnassociatedEndpoint {
        self.endpoint
    }
}

impl std::fmt::Display for TryFromEndpointError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "cannot construct a BlockingEndpoint from an endpoint with \
             skip_set_event_on_handle set: GetOverlappedResult's wait relies \
             on exactly the notification that mode suppresses"
        )
    }
}

impl std::error::Error for TryFromEndpointError {}

impl From<TryFromEndpointError> for io::Error {
    fn from(error: TryFromEndpointError) -> Self {
        io::Error::new(io::ErrorKind::InvalidInput, error)
    }
}
