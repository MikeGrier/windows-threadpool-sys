// Copyright (c) 2026 Mike Grier
//! Blocking backend: complete one overlapped operation at a time by waiting on
//! an event of the endpoint's own with `GetOverlappedResult`.
//!
//! This is the backend for overlapped endpoints that are not associated with a
//! completion port. Because one event signals whichever operation completes,
//! it supports at most one outstanding operation at a time and completes it
//! synchronously, so it needs neither ownership transfer nor rundown.

use std::io;
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle};

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::IO::{GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Threading::{CreateEventW, ResetEvent};

use crate::{EndpointHandle, EndpointObserver, Operation, OperationState, UnassociatedEndpoint};

#[cfg(test)]
mod tests;

/// An overlapped endpoint that completes operations synchronously, one at a
/// time, via `GetOverlappedResult`.
///
/// "One at a time" is enforced, not merely documented: every safe adapter on
/// this type takes `&mut self`, so a second operation while one is in flight is
/// a borrow-check error. That matters because `GetOverlappedResult` waits on
/// this endpoint's one event, which is signalled by whichever operation
/// completes -- with two outstanding, a call could return the other one's
/// result and hand back buffers the kernel is still writing into.
///
/// The event is the endpoint's own rather than the handle's, because a
/// synchronous query through an [`EndpointObserver`] resets the handle's event
/// and could strand a wait on it (M15.6).
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
    handle: EndpointHandle,
    /// Manual-reset; lent to each operation's `OVERLAPPED.hEvent` for one `run`.
    event: OwnedHandle,
}

impl BlockingEndpoint {
    /// Take ownership of an overlapped endpoint for synchronous completion.
    ///
    /// Any notification mode is accepted. `skip_set_event_on_handle` was
    /// refused before M15.6, when the wait used the handle's own event; it now
    /// uses an event this endpoint creates, which that mode does not suppress.
    ///
    /// # Errors
    ///
    /// Returns [`TryFromEndpointError`], recoverable back into `endpoint` via
    /// [`TryFromEndpointError::into_endpoint`], if `CreateEventW` fails to
    /// create the endpoint's event.
    pub fn new(endpoint: UnassociatedEndpoint) -> Result<Self, TryFromEndpointError> {
        // SAFETY: an unnamed manual-reset event, initially unsignalled, with
        // default security; null reports failure.
        let raw = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        // No test reaches this: event creation fails only when the process is out of handles or memory.
        if raw.is_null() {
            return Err(TryFromEndpointError {
                endpoint,
                cause: io::Error::last_os_error(),
            });
        }
        Ok(Self {
            handle: endpoint.into_endpoint_handle(),
            // SAFETY: a fresh event handle that nothing else owns.
            event: unsafe { OwnedHandle::from_raw_handle(raw) },
        })
    }

    /// Borrow the underlying handle for issuing native operations.
    #[must_use]
    pub fn handle(&self) -> BorrowedHandle<'_> {
        self.handle.as_handle()
    }

    /// A new observer of this endpoint's handle.
    ///
    /// An observer cannot issue I/O. Its queries do reset the handle's own
    /// event, but [`run`](Self::run) waits on an event of this endpoint's own.
    #[must_use]
    pub fn observer(&self) -> EndpointObserver {
        self.handle.observer()
    }

    /// Issue one overlapped operation and block until it completes, returning the
    /// number of bytes transferred.
    ///
    /// `issue` performs the native call with the operation's `OVERLAPPED`
    /// pointer, returning `Ok` when the operation was accepted (native success or
    /// `ERROR_IO_PENDING`) and `Err` on an immediate failure.
    ///
    /// For the call's duration the `OVERLAPPED`'s `hEvent` is this endpoint's
    /// event; it is null again when `run` returns, so the operation carries no
    /// event into another backend.
    ///
    /// # Safety
    ///
    /// `issue` must start exactly one overlapped operation using the provided
    /// `OVERLAPPED` pointer and no other storage, must not change its `hEvent`,
    /// and no other operation may be outstanding on this endpoint until this
    /// call returns. Any buffers the operation reads or writes must stay valid
    /// for the duration of the call.
    ///
    /// This takes `&self` rather than `&mut self` so a caller driving the raw
    /// seam can hold other borrows of the endpoint; the exclusivity requirement
    /// is theirs to uphold, which is what makes this `unsafe`. The safe adapters
    /// built on it take `&mut self` instead, so they cannot violate it.
    pub unsafe fn run<P, F>(&self, operation: &mut Operation<P>, issue: F) -> io::Result<usize>
    where
        F: FnOnce(BorrowedHandle<'_>, *mut OVERLAPPED) -> io::Result<()>,
    {
        let event = self.event.as_raw_handle();
        // Native overlapped calls reset hEvent themselves; this covers an `issue` that does not.
        // SAFETY: a live event this endpoint owns.
        if unsafe { ResetEvent(event) } == 0 {
            return Err(io::Error::last_os_error());
        }
        operation.set_state(OperationState::Submitted);
        let overlapped = operation.overlapped_ptr();
        let _lent = LentEvent::lend(overlapped, event);
        issue(self.handle(), overlapped)?;
        operation.set_state(OperationState::Pending);

        let mut transferred: u32 = 0;
        // SAFETY: the handle and overlapped are valid; a non-zero `wait` blocks
        // on `hEvent`, this endpoint's event, until this single operation
        // completes.
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

/// An event lent to an `OVERLAPPED` for one `run`, taken back on every exit.
struct LentEvent(*mut OVERLAPPED);

impl LentEvent {
    fn lend(overlapped: *mut OVERLAPPED, event: HANDLE) -> Self {
        // SAFETY: the operation is exclusively borrowed by `run` and idle, so
        // nothing else reads or writes its OVERLAPPED.
        unsafe { (*overlapped).hEvent = event };
        Self(overlapped)
    }
}

impl Drop for LentEvent {
    fn drop(&mut self) {
        // SAFETY: as in `lend`; by now the operation is complete or never started.
        unsafe { (*self.0).hEvent = std::ptr::null_mut() };
    }
}

/// [`BlockingEndpoint::new`]'s failure: the endpoint's event could not be
/// created. Carries the endpoint back so a caller loses nothing.
#[derive(Debug)]
pub struct TryFromEndpointError {
    endpoint: UnassociatedEndpoint,
    cause: io::Error,
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
            "cannot construct a BlockingEndpoint: creating its completion event failed: {}",
            self.cause
        )
    }
}

impl std::error::Error for TryFromEndpointError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.cause)
    }
}

impl From<TryFromEndpointError> for io::Error {
    fn from(error: TryFromEndpointError) -> Self {
        io::Error::new(error.cause.kind(), error)
    }
}
