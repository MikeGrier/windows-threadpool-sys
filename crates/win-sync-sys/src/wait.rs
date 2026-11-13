// Copyright (c) 2026 Mike Grier
//! Waiting on one kernel object: `WaitForSingleObject` and `WaitForSingleObjectEx`.

use std::io;
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle};
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    FALSE, TRUE, WAIT_ABANDONED, WAIT_FAILED, WAIT_IO_COMPLETION, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::System::Threading::{INFINITE, WaitForSingleObject, WaitForSingleObjectEx};

use crate::SealedWaitOnlyEvent;

mod private {
    pub trait Sealed {}
}

/// An object accepted by the thread-based wait functions without exposing its handle to safe code.
///
/// Every [`AsHandle`] type implements this, as do [`SealedWaitOnlyEvent`], its reference, and
/// [`WaitObject`]. Implementations are sealed; downstream types participate through `AsHandle`.
/// This makes no claim about object kind: unlike thread-pool waits, these waits accept mutexes.
///
/// A type without `AsHandle` cannot add an implementation outside this crate:
///
/// ```compile_fail,E0277
/// use std::os::windows::io::BorrowedHandle;
/// use win_sync_sys::Waitable;
/// struct External;
/// impl Waitable for External {
///     unsafe fn waitable_handle(&self) -> BorrowedHandle<'_> { unimplemented!() }
/// }
/// ```
pub trait Waitable: private::Sealed {
    /// Borrow the handle for a wait.
    ///
    /// # Safety
    ///
    /// Preserve [`SealedWaitOnlyEvent`'s handle-access contract](SealedWaitOnlyEvent#handle-access-contract)
    /// for the returned handle and every handle derived from it, even after this borrow ends.
    ///
    /// ```compile_fail,E0133
    /// use win_sync_sys::{SealedWaitOnlyEvent, Waitable};
    /// fn expose(event: &SealedWaitOnlyEvent) { let _ = event.waitable_handle(); }
    /// ```
    unsafe fn waitable_handle(&self) -> BorrowedHandle<'_>;
}

impl<T: AsHandle + ?Sized> private::Sealed for T {}

impl<T: AsHandle + ?Sized> Waitable for T {
    unsafe fn waitable_handle(&self) -> BorrowedHandle<'_> {
        self.as_handle()
    }
}

impl private::Sealed for SealedWaitOnlyEvent {}

impl Waitable for SealedWaitOnlyEvent {
    unsafe fn waitable_handle(&self) -> BorrowedHandle<'_> {
        unsafe { self.as_waitable_handle() }
    }
}

impl private::Sealed for &SealedWaitOnlyEvent {}

impl Waitable for &SealedWaitOnlyEvent {
    unsafe fn waitable_handle(&self) -> BorrowedHandle<'_> {
        unsafe { self.as_waitable_handle() }
    }
}

/// An opaque borrowed wait target for mixing sealed events with ordinary handles.
///
/// Construct from a [`BorrowedHandle`] or [`SealedWaitOnlyEvent::wait_object`]. Copies borrow the
/// same object and never duplicate its handle. Neither `Debug` nor a safe accessor reveals it.
/// The borrow keeps the source alive for the wait; waiting retains the native object's semantics.
///
/// ```compile_fail,E0599
/// use std::os::windows::io::AsHandle;
/// use win_sync_sys::WaitObject;
/// fn expose(object: WaitObject<'_>) { let _ = object.as_handle(); }
/// ```
///
/// ```compile_fail,E0599
/// use std::os::windows::io::AsRawHandle;
/// use win_sync_sys::WaitObject;
/// fn expose(object: WaitObject<'_>) { let _ = object.as_raw_handle(); }
/// ```
///
/// ```compile_fail,E0133
/// use win_sync_sys::{WaitObject, Waitable};
/// fn expose(object: WaitObject<'_>) { let _ = object.waitable_handle(); }
/// ```
///
/// ```compile_fail,E0515
/// use win_sync_sys::{SealedWaitOnlyEvent, WaitObject};
/// fn escape(event: SealedWaitOnlyEvent) -> WaitObject<'static> { event.wait_object() }
/// ```
///
/// ```compile_fail,E0515
/// use std::os::windows::io::{AsHandle, OwnedHandle};
/// use win_sync_sys::WaitObject;
/// fn escape(owner: OwnedHandle) -> WaitObject<'static> { owner.as_handle().into() }
/// ```
///
/// ```compile_fail,E0277
/// use std::os::windows::io::BorrowedHandle;
/// use win_sync_sys::WaitObject;
/// fn expose<'a>(object: WaitObject<'a>) -> BorrowedHandle<'a> { object.into() }
/// ```
#[derive(Clone, Copy)]
pub struct WaitObject<'a> {
    handle: BorrowedHandle<'a>,
}

impl std::fmt::Debug for WaitObject<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("WaitObject").finish_non_exhaustive()
    }
}

impl<'a> From<BorrowedHandle<'a>> for WaitObject<'a> {
    fn from(handle: BorrowedHandle<'a>) -> Self {
        Self { handle }
    }
}

impl<'a> From<&'a SealedWaitOnlyEvent> for WaitObject<'a> {
    fn from(event: &'a SealedWaitOnlyEvent) -> Self {
        Self {
            handle: unsafe { event.as_waitable_handle() },
        }
    }
}

impl private::Sealed for WaitObject<'_> {}

impl Waitable for WaitObject<'_> {
    unsafe fn waitable_handle(&self) -> BorrowedHandle<'_> {
        self.handle
    }
}

impl private::Sealed for &WaitObject<'_> {}

impl Waitable for &WaitObject<'_> {
    unsafe fn waitable_handle(&self) -> BorrowedHandle<'_> {
        self.handle
    }
}

/// Whether a wait can be ended early by an APC queued to the waiting thread.
///
/// An alertable wait runs every APC queued to the thread, and I/O completion routines, before it
/// returns, and then reports [`WaitOutcome::Alerted`]; a wait that is not alertable leaves them
/// queued. [`queue_user_apc`](crate::queue_user_apc) is what queues one.
///
/// **A pending APC wins over a signalled object.** On the Windows this was observed on, an
/// alertable wait with an APC already queued runs it and returns `Alerted` even when the object is
/// signalled, and leaves the object as it was; the next wait takes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Alertable {
    /// The wait ends only by its object, or its timeout.
    No,
    /// A queued APC ends the wait.
    Yes,
}

impl Alertable {
    /// The `BOOL` the Win32 calls take.
    pub(crate) fn as_bool(self) -> i32 {
        match self {
            Self::No => FALSE,
            Self::Yes => TRUE,
        }
    }
}

/// How a wait that did not fail ended.
///
/// Non-exhaustive, so a wait that can end another way can be added.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum WaitOutcome {
    /// The object was signalled, and the wait has taken whatever a signalled object gives: an
    /// auto-reset event is unsignalled again, a mutex is owned by the waiting thread, a semaphore's
    /// count is one lower.
    Signalled,
    /// The object is a mutex whose owner ended without releasing it. The waiting thread owns it
    /// now, and whatever it protected may be inconsistent. No other kind of object ends a wait
    /// this way, but the wait is over any handle, so a caller that cannot rule the handle out
    /// cannot ignore it.
    Abandoned,
    /// The timeout passed with the object not signalled.
    TimedOut,
    /// An alertable wait was ended by an APC queued to the thread, which has run. Only a wait made
    /// [`Alertable::Yes`] ends this way. **The object was not waited for**: nothing was taken from
    /// it, so the caller waits again if it still needs it.
    Alerted,
}

/// Wait for `object` to be signalled, for up to `timeout`: `None` is no limit, and `Duration::ZERO`
/// only asks whether it is signalled now.
///
/// Accepts any [`AsHandle`] type, a [`SealedWaitOnlyEvent`] or its reference, or a [`WaitObject`].
/// Sealed targets never lend their handle to the caller of this function.
///
/// **A satisfied wait changes the object**, as the Win32 call does: it resets an auto-reset event,
/// acquires a mutex and decrements a semaphore. The wait is not alertable, so a queued APC does
/// not end it; [`wait_for_single_object_ex`] is the one an APC can end.
///
/// A timeout is rounded **up** to whole milliseconds, so a wait for a nonzero time is never a poll.
///
/// # Errors
///
/// [`io::ErrorKind::InvalidInput`] for a timeout the call cannot express: `INFINITE` is `u32::MAX`
/// milliseconds, so the longest finite wait is one millisecond less, about 49.7 days. It is refused
/// rather than shortened, since a wait that ends early has given a wrong answer. Otherwise the
/// error from `GetLastError` when the wait fails, such as access denied for a handle without
/// `SYNCHRONIZE` access.
pub fn wait_for_single_object(
    object: impl Waitable,
    timeout: Option<Duration>,
) -> io::Result<WaitOutcome> {
    let milliseconds = milliseconds(timeout)?;
    // SAFETY: the target stays borrowed and its handle is used only for this wait.
    let code =
        unsafe { WaitForSingleObject(object.waitable_handle().as_raw_handle(), milliseconds) };
    outcome(code)
}

/// [`wait_for_single_object`], which an APC can end: `WaitForSingleObjectEx`.
///
/// With [`Alertable::No`] it is the same wait as [`wait_for_single_object`]. With
/// [`Alertable::Yes`] it returns [`WaitOutcome::Alerted`] when an APC queued to the calling thread
/// ran, without having taken anything from the object.
///
/// Accepts the same [`Waitable`] objects as [`wait_for_single_object`], including
/// [`SealedWaitOnlyEvent`] and [`WaitObject`].
///
/// # Errors
///
/// As [`wait_for_single_object`].
pub fn wait_for_single_object_ex(
    object: impl Waitable,
    timeout: Option<Duration>,
    alertable: Alertable,
) -> io::Result<WaitOutcome> {
    let milliseconds = milliseconds(timeout)?;
    // SAFETY: the target stays borrowed and its handle is used only for this wait.
    let code = unsafe {
        WaitForSingleObjectEx(
            object.waitable_handle().as_raw_handle(),
            milliseconds,
            alertable.as_bool(),
        )
    };
    outcome(code)
}

/// A timeout as the call takes it: whole milliseconds rounded up, `INFINITE` for none.
pub(crate) fn milliseconds(timeout: Option<Duration>) -> io::Result<u32> {
    let Some(timeout) = timeout else {
        return Ok(INFINITE);
    };
    let rounded_up = timeout.as_nanos().div_ceil(1_000_000);
    match u32::try_from(rounded_up) {
        // `INFINITE` is a value of the same parameter, so a finite wait may not take it.
        Ok(milliseconds) if milliseconds != INFINITE => Ok(milliseconds),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a wait of this many milliseconds cannot be expressed: the longest finite wait is \
             u32::MAX - 1",
        )),
    }
}

/// What a wait on one object returned, as an outcome or an error.
pub(crate) fn outcome(code: u32) -> io::Result<WaitOutcome> {
    match code {
        WAIT_OBJECT_0 => Ok(WaitOutcome::Signalled),
        WAIT_ABANDONED => Ok(WaitOutcome::Abandoned),
        WAIT_TIMEOUT => Ok(WaitOutcome::TimedOut),
        WAIT_IO_COMPLETION => Ok(WaitOutcome::Alerted),
        WAIT_FAILED => Err(io::Error::last_os_error()),
        // The call returns no other value; named so the next reader does not have to wonder, and
        // not assumed away.
        other => Err(io::Error::other(format!("a wait returned {other:#x}"))),
    }
}

#[cfg(test)]
mod tests;
