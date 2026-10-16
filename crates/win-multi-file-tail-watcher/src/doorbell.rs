// Copyright (c) 2026 Mike Grier
//! A manual-reset Windows event, rung by one side and waited on by the other.

use std::io;
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle};
use std::ptr;

use windows_sys::Win32::Foundation::{FALSE, TRUE};
use windows_sys::Win32::System::Threading::{CreateEventW, ResetEvent, SetEvent};

#[cfg(test)]
mod tests;

/// A manual-reset event.
///
/// Manual-reset because an auto-reset event can lose a signal (TW-D-6): ringing stays rung, for
/// every waiter, until someone resets it. Whoever resets it must do so *before* reading what it
/// announces, so an entry written after the reset rings it again; reset after reading can erase
/// a ring that announced an entry not yet read.
#[derive(Debug)]
pub(crate) struct Doorbell {
    event: OwnedHandle,
}

impl Doorbell {
    /// A new, unrung doorbell.
    ///
    /// # Errors
    ///
    /// The error from `CreateEventW`.
    pub(crate) fn new() -> io::Result<Self> {
        // SAFETY: creates an unnamed event with default security attributes; every argument is
        // either null or a plain flag.
        let raw = unsafe { CreateEventW(ptr::null(), TRUE, FALSE, ptr::null()) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the call returned a fresh handle that nothing else owns.
        let event = unsafe { OwnedHandle::from_raw_handle(raw) };
        Ok(Self { event })
    }

    /// Ring it, releasing every waiter and every later wait until it is reset.
    ///
    /// A failure of `SetEvent` is not reported: the handle is live and owned here, so it has no
    /// cause but a handle this type no longer holds.
    pub(crate) fn ring(&self) {
        // SAFETY: a live event owned by this value for as long as it exists; `SetEvent` has no
        // other precondition.
        unsafe {
            SetEvent(self.event.as_raw_handle());
        }
    }

    /// Clear it. Does nothing if it is not rung.
    pub(crate) fn reset(&self) {
        // SAFETY: as in `ring`.
        unsafe {
            ResetEvent(self.event.as_raw_handle());
        }
    }

    /// The event, to wait on. It is rung while the doorbell is.
    pub(crate) fn handle(&self) -> BorrowedHandle<'_> {
        self.event.as_handle()
    }

    /// A second handle to the same event, for a wait that must own the handle it watches.
    ///
    /// # Errors
    ///
    /// The error from `DuplicateHandle`.
    pub(crate) fn duplicate_event(&self) -> io::Result<OwnedHandle> {
        self.event.try_clone()
    }
}
