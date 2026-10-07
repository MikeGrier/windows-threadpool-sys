// Copyright (c) 2026 Mike Grier
//! Event objects: `CreateEventW`, `SetEvent` and `ResetEvent`.

use std::io;
use std::os::windows::io::{
    AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle, RawHandle,
};
use std::ptr;

use windows_sys::Win32::Foundation::{FALSE, TRUE};
use windows_sys::Win32::System::Threading::{CreateEventW, ResetEvent, SetEvent};

/// How an event returns to unsignalled after it is set.
///
/// Fixed when the event is created: Windows offers no way to change it, or to
/// ask an existing event which it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResetMode {
    /// The event returns to unsignalled as soon as it releases one waiting
    /// thread. Each [`Event::set`] therefore releases exactly one waiter; with
    /// nobody waiting, the event stays signalled until somebody waits.
    Auto,
    /// The event stays signalled, releasing every waiter, until
    /// [`Event::reset`] is called.
    Manual,
}

/// An owned event object.
///
/// Holds a handle to an event and closes it on drop. Because the handle is
/// owned and is known to be an event, [`set`](Self::set) and
/// [`reset`](Self::reset) need no `unsafe`.
///
/// **The handle being an event is what other crates may rely on**
/// ([WS-D-4](../DESIGN-NOTES.md#ws-d-4)).
/// Ownership alone keeps `set` memory-safe; it is event-ness that lets a
/// consumer such as the thread pool, which cannot wait on a mutex, accept an
/// `Event` without `unsafe`. That is why adopting an existing handle,
/// [`from_owned_handle`](Self::from_owned_handle), is `unsafe`.
///
/// Setting and resetting take `&self`: they are atomic kernel operations, and
/// an `Event` is `Send` and `Sync`. Setting an event that is already signalled
/// changes nothing -- an event is either signalled or not, and does not count
/// how often it was set.
///
/// There is no `pulse`; see the crate documentation for why.
#[derive(Debug)]
pub struct Event {
    handle: OwnedHandle,
}

impl Event {
    /// Create an unnamed event.
    ///
    /// # Errors
    ///
    /// Returns the error from `CreateEventW`. No test reaches this path:
    /// creating an unnamed event fails only when the process cannot get
    /// another handle or the kernel cannot allocate the object, which a test
    /// can produce only by exhausting a resource the rest of the run needs.
    pub fn new(reset: ResetMode, initially_signalled: bool) -> io::Result<Self> {
        let manual_reset = match reset {
            ResetMode::Auto => FALSE,
            ResetMode::Manual => TRUE,
        };
        let initial_state = if initially_signalled { TRUE } else { FALSE };
        // SAFETY: an unnamed event with default security; every pointer
        // argument is null, which the function accepts for both.
        let raw = unsafe { CreateEventW(ptr::null(), manual_reset, initial_state, ptr::null()) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the call returned a new handle that nothing else owns.
        let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
        Ok(Self { handle })
    }

    /// Adopt a handle the caller already owns as an event.
    ///
    /// For an event that came from somewhere this crate did not create it, such
    /// as another library or a handle passed in by a parent process.
    ///
    /// A handle without `EVENT_MODIFY_STATE` access is accepted: [`set`](Self::set)
    /// and [`reset`](Self::reset) then report the access error rather than
    /// changing the event.
    ///
    /// # Safety
    ///
    /// The caller guarantees that `handle`:
    ///
    /// - refers to an **event** object -- not a mutex, a semaphore or any other
    ///   kind, because code holding an `Event` may rely on that; and
    /// - carries `SYNCHRONIZE` access, so that anything given the `Event` to
    ///   wait on can wait on it. What a thread-pool wait does with a handle it
    ///   cannot wait on is not documented, and an `Event` must be safe to hand
    ///   it.
    #[must_use]
    pub unsafe fn from_owned_handle(handle: OwnedHandle) -> Self {
        Self { handle }
    }

    /// Set the event to signalled.
    ///
    /// Releases one waiter if the event is [`ResetMode::Auto`], and every
    /// waiter until it is reset if it is [`ResetMode::Manual`]. Setting an
    /// event that is already signalled succeeds and changes nothing.
    ///
    /// # Errors
    ///
    /// Returns the error from `SetEvent`: an access-denied error when the
    /// handle was adopted without `EVENT_MODIFY_STATE`.
    pub fn set(&self) -> io::Result<()> {
        // SAFETY: the handle is owned by `self`, so it is open.
        if unsafe { SetEvent(self.handle.as_raw_handle()) } == FALSE {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Set the event to unsignalled.
    ///
    /// Resetting an event that is already unsignalled succeeds and changes
    /// nothing. Resetting an auto-reset event withdraws a signal no waiter
    /// has taken yet.
    ///
    /// # Errors
    ///
    /// Returns the error from `ResetEvent`: an access-denied error when the
    /// handle was adopted without `EVENT_MODIFY_STATE`.
    pub fn reset(&self) -> io::Result<()> {
        // SAFETY: the handle is owned by `self`, so it is open.
        if unsafe { ResetEvent(self.handle.as_raw_handle()) } == FALSE {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// A second handle to the same event, with the same access.
    ///
    /// Setting or resetting either affects both, and the event lives until
    /// every handle to it is closed, so a clone may outlive the original.
    ///
    /// # Errors
    ///
    /// Returns the error from `DuplicateHandle`. Like [`new`](Self::new)'s, no
    /// test reaches it: duplicating a handle this process owns fails only when
    /// the process cannot get another handle.
    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            handle: self.handle.try_clone()?,
        })
    }
}

impl AsHandle for Event {
    fn as_handle(&self) -> BorrowedHandle<'_> {
        self.handle.as_handle()
    }
}

impl AsRawHandle for Event {
    fn as_raw_handle(&self) -> RawHandle {
        self.handle.as_raw_handle()
    }
}

impl From<Event> for OwnedHandle {
    fn from(event: Event) -> Self {
        event.handle
    }
}

#[cfg(test)]
mod tests;
