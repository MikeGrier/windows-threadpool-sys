// Copyright (c) 2026 Mike Grier
//! Event objects: `CreateEventW`, `SetEvent` and `ResetEvent`.

use std::io;
use std::os::windows::io::{
    AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle, RawHandle,
};
use std::ptr;

use windows_sys::Win32::Foundation::{FALSE, TRUE};
use windows_sys::Win32::System::Threading::{CreateEventW, ResetEvent, SetEvent};

use win_handles_sys::{Access, duplicate_handle_with_access};
use win_shared_os_owned_handle::SharedHandle;

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
/// Holds a handle to an event, shared by every clone, and closes it when the
/// last clone drops. A clone is a second holder of the same handle, not a new
/// handle: setting through one is seen through every other
/// ([WS-D-10](../DESIGN-NOTES.md#ws-d-10)). Because the handle is owned and is
/// known to be an event, [`set`](Self::set) and [`reset`](Self::reset) need no
/// `unsafe`.
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
#[derive(Clone, Debug)]
pub struct Event {
    handle: SharedHandle,
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
        Ok(Self {
            handle: SharedHandle::new(handle),
        })
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
        Self {
            handle: SharedHandle::new(handle),
        }
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

    /// The handle, as an [`OwnedHandle`], if `self` is its last holder.
    ///
    /// # Errors
    ///
    /// If a clone still holds the handle, returns `self` unchanged, so nothing is
    /// lost and the caller can try again once the clones have dropped.
    pub fn try_into_owned(self) -> Result<OwnedHandle, Self> {
        self.handle
            .try_into_owned()
            .map_err(|handle| Self { handle })
    }

    /// A new handle to the same event with wait-only access; narrowing access needs a new handle
    /// ([WS-D-10](../DESIGN-NOTES.md#ws-d-10)).
    ///
    /// The handle carries `SYNCHRONIZE` access alone, so it can neither set nor reset the event:
    /// the kernel refuses a `SetEvent` or `ResetEvent` on it. A clone cannot give that, since it
    /// shares the original's handle and so its access. See [`WaitOnlyEvent`] for
    /// what the result is, why it is not an `Event`, and what the guarantee does not cover.
    ///
    /// A wait through it takes what a signalled event gives, as any wait does: waiting through a
    /// wait-only handle consumes an auto-reset event's signal. What it cannot do is signal or
    /// withdraw one.
    ///
    /// # Errors
    ///
    /// Returns the error from `DuplicateHandle`. Like [`new`](Self::new)'s, no test reaches it:
    /// duplicating a handle this process owns fails only when the process cannot get another
    /// handle.
    pub fn duplicate_wait_only(&self) -> io::Result<WaitOnlyEvent> {
        Ok(WaitOnlyEvent {
            handle: SharedHandle::new(duplicate_handle_with_access(self, Access::SYNCHRONIZE)?),
        })
    }

    /// A wait-only duplicate whose handle cannot be obtained by safe code.
    ///
    /// This seals [`duplicate_wait_only`](Self::duplicate_wait_only)'s result; it does not
    /// revoke the owner's access. A satisfied wait still consumes an auto-reset event's signal.
    ///
    /// # Errors
    ///
    /// The errors of [`duplicate_wait_only`](Self::duplicate_wait_only), with the same
    /// resource-exhaustion limitation on testing that path.
    pub fn duplicate_sealed_wait_only(&self) -> io::Result<SealedWaitOnlyEvent> {
        self.duplicate_wait_only().map(SealedWaitOnlyEvent::from)
    }
}

/// A handle to an event that can wait on it and cannot set or reset it.
///
/// Made by [`Event::duplicate_wait_only`], carrying `SYNCHRONIZE` access and no more. Its clones
/// share its handle, as an [`Event`]'s do. It is its own
/// type, not an [`Event`] that fails when set, because what it cannot do is not an error to be
/// discovered at run time: it has no `set` and no `reset`, and the kernel would refuse them if it
/// were handed to code that tried ([WS-D-7](../DESIGN-NOTES.md#ws-d-7)).
///
/// **What that guarantees**: the handle cannot be used to signal the event, by this crate or by a
/// raw call, because the kernel refuses. **What it does not**: code that holds the handle can
/// duplicate it asking for more access, which Windows allows
/// ([`duplicate_handle_with_access`](crate::duplicate_handle_with_access)), so this guards against
/// a handle used for what it was not handed out for, not against a holder bent on signalling.
/// Convert into [`SealedWaitOnlyEvent`] to withhold handle access from safe code through that value.
///
/// **It is still known to be an event**, so what [WS-D-4](../DESIGN-NOTES.md#ws-d-4) lets a consumer
/// rely on holds here too: the handle is not a mutex, which is the one kind of object a thread-pool
/// wait cannot take. There is no way to adopt a handle as one, so that stays true without `unsafe`.
///
/// It lends its handle, so [`wait_for_single_object`](crate::wait_for_single_object) waits on it.
/// `Send` and `Sync`, as an event is.
#[derive(Clone, Debug)]
pub struct WaitOnlyEvent {
    handle: SharedHandle,
}

impl WaitOnlyEvent {
    /// The handle, as an [`OwnedHandle`], if `self` is its last holder.
    ///
    /// # Errors
    ///
    /// If a clone still holds the handle, returns `self` unchanged.
    pub fn try_into_owned(self) -> Result<OwnedHandle, Self> {
        self.handle
            .try_into_owned()
            .map_err(|handle| Self { handle })
    }
}

impl AsHandle for WaitOnlyEvent {
    fn as_handle(&self) -> BorrowedHandle<'_> {
        self.handle.as_handle()
    }
}

impl AsRawHandle for WaitOnlyEvent {
    fn as_raw_handle(&self) -> RawHandle {
        self.handle.as_raw_handle()
    }
}

/// The shared handle, given up by this holder; it is still an event's, with `SYNCHRONIZE` alone.
impl From<WaitOnlyEvent> for SharedHandle {
    fn from(event: WaitOnlyEvent) -> Self {
        event.handle
    }
}

/// A shared wait-only event whose handle is accessible only through `unsafe` operations.
///
/// Made by [`Event::duplicate_sealed_wait_only`] or by consuming a [`WaitOnlyEvent`]. Conversion
/// and cloning share the existing handle, with `SYNCHRONIZE` alone; they do not duplicate it.
/// The object is an event, never a mutex, and the type is `Send` and `Sync`.
///
/// The seal restricts access through this value, not through the owning [`Event`] or unsealed
/// aliases retained before conversion. It changes no operating-system security descriptor.
/// A wait on a manual-reset event leaves its level unchanged; a satisfied wait on an auto-reset
/// event consumes the signal. This type has no `set` or `reset`, and its `Debug` hides the handle.
/// The crate's thread-wait functions accept it directly; [`wait_object`](Self::wait_object)
/// supplies an opaque borrow for a mixed set of targets.
///
/// # Handle-access contract
///
/// Treat an extracted handle and every handle derived from it as wait-only: do not explicitly
/// change the event's state, widen access, or expose it to code that could do either. This
/// obligation lasts until those handles are closed, not merely until the accessor returns.
/// Owning, cloning and dropping such handles is permitted while preserving this restriction.
/// The `unsafe` boundary protects protocol analyses relying on this restriction, not just
/// memory safety ([WS-D-11](../DESIGN-NOTES.md#ws-d-11)).
///
/// Safe code cannot borrow, extract, or convert the handle:
///
/// ```compile_fail
/// use std::os::windows::io::AsHandle;
/// use win_sync_sys::SealedWaitOnlyEvent;
/// fn borrow(event: &SealedWaitOnlyEvent) { let _ = event.as_handle(); }
/// ```
///
/// ```compile_fail
/// use std::os::windows::io::AsRawHandle;
/// use win_sync_sys::SealedWaitOnlyEvent;
/// fn raw(event: &SealedWaitOnlyEvent) { let _ = event.as_raw_handle(); }
/// ```
///
/// ```compile_fail
/// use win_shared_os_owned_handle::SharedHandle;
/// use win_sync_sys::SealedWaitOnlyEvent;
/// fn shared(event: SealedWaitOnlyEvent) { let _ = SharedHandle::from(event); }
/// ```
///
/// ```compile_fail
/// use win_sync_sys::SealedWaitOnlyEvent;
/// fn owned(event: SealedWaitOnlyEvent) { let _ = event.try_into_owned(); }
/// ```
///
/// ```compile_fail
/// use win_sync_sys::{SealedWaitOnlyEvent, WaitOnlyEvent};
/// fn unseal(event: SealedWaitOnlyEvent) { let _ = WaitOnlyEvent::from(event); }
/// ```
#[derive(Clone)]
pub struct SealedWaitOnlyEvent {
    handle: SharedHandle,
}

impl std::fmt::Debug for SealedWaitOnlyEvent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SealedWaitOnlyEvent")
            .finish_non_exhaustive()
    }
}

impl SealedWaitOnlyEvent {
    /// Borrow an opaque target for a mixed [`wait_for_any`](crate::wait_for_any) or wait-for-all.
    pub fn wait_object(&self) -> crate::WaitObject<'_> {
        crate::WaitObject::from(self)
    }

    /// Borrow the event's `SYNCHRONIZE`-only handle.
    ///
    /// # Safety
    ///
    /// Preserve the [handle-access contract](Self#handle-access-contract), including for every
    /// derived handle after this borrow ends.
    ///
    /// ```compile_fail
    /// use win_sync_sys::SealedWaitOnlyEvent;
    /// fn borrow(event: &SealedWaitOnlyEvent) { let _ = event.as_waitable_handle(); }
    /// ```
    pub unsafe fn as_waitable_handle(&self) -> BorrowedHandle<'_> {
        self.handle.as_handle()
    }

    /// Transfer this holder's shared handle without duplicating it or requiring the last clone.
    ///
    /// # Safety
    ///
    /// Preserve the [handle-access contract](Self#handle-access-contract) for the returned handle
    /// and all its descendants, including after every sealed holder has dropped.
    ///
    /// ```compile_fail
    /// use win_sync_sys::SealedWaitOnlyEvent;
    /// fn shared(event: SealedWaitOnlyEvent) { let _ = event.into_shared_handle(); }
    /// ```
    pub unsafe fn into_shared_handle(self) -> SharedHandle {
        self.handle
    }
}

/// Seal this holder without revoking any unsealed aliases or creating another handle.
impl From<WaitOnlyEvent> for SealedWaitOnlyEvent {
    fn from(event: WaitOnlyEvent) -> Self {
        Self {
            handle: event.handle,
        }
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

/// The shared handle, given up by this holder: what a consumer that holds handles shared, such as
/// the thread pool's wait target, takes.
impl From<Event> for SharedHandle {
    fn from(event: Event) -> Self {
        event.handle
    }
}

#[cfg(test)]
mod tests;
