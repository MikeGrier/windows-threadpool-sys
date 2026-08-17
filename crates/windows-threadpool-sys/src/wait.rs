// Copyright (c) 2026 Mike Grier
//! Thread-pool waits: `CreateThreadpoolWait` / `SetThreadpoolWait` /
//! `WaitForThreadpoolWaitCallbacks` / `CloseThreadpoolWait`.
//!
//! A wait object watches one waitable handle and queues its callback when the
//! handle is signalled or the wait times out. Two SDK contracts shape this API:
//!
//! - **The handle must stay valid while a wait is pending.** [`ThreadpoolWait`]
//!   therefore *owns* its handle rather than borrowing one, so it cannot be
//!   closed underneath a pending wait. Use [`ThreadpoolWait::handle`] to signal
//!   or inspect it.
//! - **A wait fires at most once per arming.** The SDK requires the wait to be
//!   rearmed explicitly for each activation, so the callback receives a
//!   [`WaitActivation`] carrying [`WaitActivation::rearm`]. A callback that does
//!   not rearm simply stops watching.
//!
//! Mutex handles are not supported by the thread pool and must not be passed to
//! [`ThreadpoolWait::new`].

use std::io;
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, OwnedHandle};
use std::ptr;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::time::Duration;

use windows_sys::Win32::Foundation::{FALSE, FILETIME, HANDLE, TRUE, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::{
    CloseThreadpoolWait, CreateThreadpoolWait, PTP_CALLBACK_INSTANCE, PTP_WAIT, SetThreadpoolWait,
    WaitForThreadpoolWaitCallbacks,
};

use crate::callback_env::CallbackEnviron;

/// Wait results the pool reports. Changing either value is a breaking change.
mod wait_status {
    /// `WAIT_OBJECT_0`: the handle was signalled.
    pub const SIGNALLED: u32 = 0;
}

/// 100-nanosecond intervals per second, for building a relative `FILETIME`.
/// Changing this value is a breaking change.
const FILETIME_TICKS_PER_SECOND: u64 = 10_000_000;
/// Nanoseconds per 100-nanosecond interval. Changing it is a breaking change.
const FILETIME_NANOS_PER_TICK: u32 = 100;

/// Build the negative tick count that means "relative timeout".
fn relative_filetime(timeout: Duration) -> FILETIME {
    let ticks = timeout
        .as_secs()
        .saturating_mul(FILETIME_TICKS_PER_SECOND)
        .saturating_add(u64::from(timeout.subsec_nanos() / FILETIME_NANOS_PER_TICK));
    let ticks = i64::try_from(ticks).unwrap_or(i64::MAX);
    let bits = (-ticks) as u64;
    FILETIME {
        dwLowDateTime: bits as u32,
        dwHighDateTime: (bits >> 32) as u32,
    }
}

/// Why a wait callback ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitResult {
    /// The watched handle became signalled.
    Signalled,
    /// The timeout given when the wait was armed elapsed first.
    TimedOut,
    /// The pool reported a result this crate does not model. The raw value is
    /// preserved so a caller can inspect it rather than having it discarded.
    Other(u32),
}

impl WaitResult {
    fn from_raw(value: u32) -> Self {
        match value {
            wait_status::SIGNALLED => Self::Signalled,
            WAIT_TIMEOUT => Self::TimedOut,
            other => Self::Other(other),
        }
    }
}

/// Heap-allocated callback state kept alive for the lifetime of the wait object.
///
/// `wait` is filled in after `CreateThreadpoolWait` returns, because rearming
/// from inside a callback needs the object the callback belongs to.
struct WaitContext {
    wait: AtomicIsize,
    handle: HANDLE,
    callback: Box<dyn Fn(&WaitActivation<'_>) + Send + Sync + 'static>,
}

// SAFETY: `handle` is a raw handle owned by the ThreadpoolWait that outlives
// this context; it is only passed back to SetThreadpoolWait, never closed here.
unsafe impl Send for WaitContext {}
unsafe impl Sync for WaitContext {}

/// One activation of a [`ThreadpoolWait`], handed to its callback.
///
/// The wait is not armed when the callback runs. Call [`WaitActivation::rearm`]
/// to watch the handle again; doing nothing leaves the wait idle.
pub struct WaitActivation<'ctx> {
    result: WaitResult,
    ctx: &'ctx WaitContext,
}

impl std::fmt::Debug for WaitActivation<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The context holds the boxed callback and the raw wait object, neither
        // of which is meaningful to a reader; the result is the whole story.
        f.debug_struct("WaitActivation")
            .field("result", &self.result)
            .finish_non_exhaustive()
    }
}

impl WaitActivation<'_> {
    /// Why this callback ran.
    #[must_use]
    pub fn result(&self) -> WaitResult {
        self.result
    }

    /// Whether the watched handle was signalled.
    #[must_use]
    pub fn is_signalled(&self) -> bool {
        self.result == WaitResult::Signalled
    }

    /// Arm the wait again, so the next signal or timeout activates it.
    ///
    /// `timeout` of `None` waits indefinitely. This is the mechanism the SDK
    /// requires for repeated waits: an activation consumes the arming, so a
    /// callback that wants to keep watching must rearm from inside itself.
    pub fn rearm(&self, timeout: Option<Duration>) {
        let wait = self.ctx.wait.load(Ordering::Acquire);
        debug_assert_ne!(
            wait, 0,
            "the wait object must be published before callbacks"
        );
        // SAFETY: `wait` is this object's live PTP_WAIT, published before any
        // callback could run, and `handle` is owned by that object so it is
        // still open. The timeout, if any, is a live stack value for the call.
        unsafe { arm_raw(wait, self.ctx.handle, timeout) };
    }
}

/// Arm or disarm a wait object.
///
/// SAFETY: `wait` must be a live `PTP_WAIT` and `handle` a live waitable handle
/// (or null to disarm).
unsafe fn arm_raw(wait: PTP_WAIT, handle: HANDLE, timeout: Option<Duration>) {
    match timeout {
        Some(timeout) => {
            let filetime = relative_filetime(timeout);
            // SAFETY: forwarded from this function's contract; `filetime` is
            // read only for the duration of the call.
            unsafe { SetThreadpoolWait(wait, handle, &filetime) };
        }
        // SAFETY: forwarded; a null timeout means "wait indefinitely".
        None => unsafe { SetThreadpoolWait(wait, handle, ptr::null()) },
    }
}

/// Trampoline from the raw `PTP_WAIT_CALLBACK` ABI into the boxed closure.
///
/// SAFETY: `context` must point to a live [`WaitContext`] for the entire
/// duration of every callback invocation, which [`ThreadpoolWait`]'s `Drop`
/// ordering guarantees.
unsafe extern "system" fn wait_trampoline(
    _instance: PTP_CALLBACK_INSTANCE,
    context: *mut core::ffi::c_void,
    _wait: PTP_WAIT,
    wait_result: u32,
) {
    // SAFETY: context is a valid *mut WaitContext for the full callback duration.
    let ctx = unsafe { &*(context as *const WaitContext) };
    let activation = WaitActivation {
        result: WaitResult::from_raw(wait_result),
        ctx,
    };
    // A panic must never unwind across the FFI boundary into the pool's frame.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        (ctx.callback)(&activation);
    }));
}

/// An owned thread-pool wait object bound to one waitable handle.
///
/// The object owns the handle, so the handle cannot be closed while a wait is
/// pending. A newly created wait is idle; arm it with [`ThreadpoolWait::arm`],
/// and rearm from inside the callback with [`WaitActivation::rearm`].
///
/// [`Drop`] disarms before draining callbacks, then closes the object and only
/// afterwards releases the callback context and the handle.
///
/// # Examples
///
/// Watch an event once. The wait takes ownership of the handle, and
/// [`ThreadpoolWait::handle`] borrows it back for signalling:
///
/// ```
/// use std::os::windows::io::{FromRawHandle, OwnedHandle};
/// use std::ptr;
/// use std::sync::mpsc;
/// use windows_sys::Win32::Foundation::FALSE;
/// use windows_sys::Win32::System::Threading::{CreateEventW, SetEvent};
/// use std::os::windows::io::AsRawHandle;
/// use windows_threadpool_sys::wait::{ThreadpoolWait, WaitResult};
///
/// // SAFETY: creates an unnamed, manual-reset event with default security.
/// let raw = unsafe { CreateEventW(ptr::null(), 1, FALSE, ptr::null()) };
/// assert!(!raw.is_null());
/// // SAFETY: the call returned a fresh, exclusively owned handle.
/// let event = unsafe { OwnedHandle::from_raw_handle(raw) };
///
/// let (tx, rx) = mpsc::channel();
/// let sender = std::sync::Mutex::new(tx);
/// let wait = ThreadpoolWait::new(event, move |activation| {
///     let _ = sender.lock().expect("send").send(activation.result());
/// }, None)?;
///
/// wait.arm(None);
/// // SAFETY: the wait owns the event, so the handle is still open.
/// unsafe { SetEvent(wait.handle().as_raw_handle()) };
///
/// assert_eq!(rx.recv().expect("activation"), WaitResult::Signalled);
/// # Ok::<(), std::io::Error>(())
/// ```
///
/// Keep watching across activations by rearming from inside the callback, which
/// is what the SDK requires -- an activation consumes the arming:
///
/// ```
/// # use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
/// # use std::ptr;
/// # use std::sync::Arc;
/// # use std::sync::atomic::{AtomicUsize, Ordering};
/// # use windows_sys::Win32::Foundation::FALSE;
/// # use windows_sys::Win32::System::Threading::{CreateEventW, SetEvent};
/// use windows_threadpool_sys::wait::ThreadpoolWait;
///
/// // SAFETY: creates an unnamed, auto-reset event with default security.
/// let raw = unsafe { CreateEventW(ptr::null(), FALSE, FALSE, ptr::null()) };
/// // SAFETY: the call returned a fresh, exclusively owned handle.
/// let event = unsafe { OwnedHandle::from_raw_handle(raw) };
///
/// let seen = Arc::new(AtomicUsize::new(0));
/// let counter = Arc::clone(&seen);
/// let wait = ThreadpoolWait::new(event, move |activation| {
///     counter.fetch_add(1, Ordering::SeqCst);
///     activation.rearm(None);
/// }, None)?;
///
/// wait.arm(None);
/// for _ in 0..3 {
///     // SAFETY: the wait owns the event, so the handle is still open.
///     unsafe { SetEvent(wait.handle().as_raw_handle()) };
///     std::thread::sleep(std::time::Duration::from_millis(5));
/// }
///
/// wait.disarm();
/// wait.wait();
/// assert!(seen.load(Ordering::SeqCst) >= 1);
/// # Ok::<(), std::io::Error>(())
/// ```
pub struct ThreadpoolWait {
    wait: PTP_WAIT,
    handle: OwnedHandle,
    // Kept alive as a raw pointer until Drop has disarmed and drained.
    context: *mut WaitContext,
}

// SAFETY: PTP_WAIT is a cross-thread pool object, OwnedHandle is Send + Sync,
// and the context is Send + Sync; the pointer is only read until Drop frees it
// after all callbacks have finished.
unsafe impl Send for ThreadpoolWait {}
unsafe impl Sync for ThreadpoolWait {}

impl ThreadpoolWait {
    /// Create an idle wait watching `handle`.
    ///
    /// The object takes ownership of the handle and closes it on drop, which is
    /// what guarantees the handle outlives any pending wait.
    ///
    /// Pass `Some(env)` to select a private pool or callback priority; `None`
    /// uses the process-default pool with default priority.
    ///
    /// The callback runs on a shared, process-managed pool thread, must restore
    /// any thread state it changes, and must not terminate its thread. A panic
    /// inside it is caught at the FFI boundary rather than unwinding into the
    /// pool.
    ///
    /// # Errors
    ///
    /// Returns the error from `CreateThreadpoolWait`.
    ///
    /// # Panics
    ///
    /// Does not panic. Passing a mutex handle is unsupported by the thread pool
    /// and produces undefined behaviour rather than an error, so callers must
    /// not do so.
    pub fn new<F>(
        handle: OwnedHandle,
        callback: F,
        env: Option<&mut CallbackEnviron>,
    ) -> io::Result<Self>
    where
        F: Fn(&WaitActivation<'_>) + Send + Sync + 'static,
    {
        let context = Box::into_raw(Box::new(WaitContext {
            wait: AtomicIsize::new(0),
            handle: handle.as_raw_handle(),
            callback: Box::new(callback),
        }));
        let env_ptr = env.map_or(ptr::null_mut(), |e| e.as_mut_ptr());

        // SAFETY: context is a valid heap pointer that outlives every callback,
        // and env_ptr is valid (or null) for the duration of this call.
        let wait = unsafe {
            CreateThreadpoolWait(Some(wait_trampoline), context.cast(), env_ptr.cast_const())
        };

        if wait == 0 {
            let error = io::Error::last_os_error();
            // SAFETY: the pool never saw context; reclaim it immediately.
            unsafe { drop(Box::from_raw(context)) };
            return Err(error);
        }

        // Publish the object before any callback can run. No wait is armed yet,
        // so no callback can observe the unpublished value.
        // SAFETY: context is live and exclusively ours until the first arming.
        unsafe { (*context).wait.store(wait, Ordering::Release) };

        Ok(Self {
            wait,
            handle,
            context,
        })
    }

    /// Borrow the watched handle, for signalling or inspecting it.
    #[must_use]
    pub fn handle(&self) -> BorrowedHandle<'_> {
        self.handle.as_handle()
    }

    /// Arm the wait, so the next signal or timeout runs the callback once.
    ///
    /// `timeout` of `None` waits indefinitely. Arming replaces any previous
    /// arming rather than adding to it, and an activation consumes the arming --
    /// rearm from inside the callback with [`WaitActivation::rearm`] to keep
    /// watching.
    pub fn arm(&self, timeout: Option<Duration>) {
        // SAFETY: `wait` is valid for the lifetime of self, and the handle is
        // owned by self so it is still open.
        unsafe { arm_raw(self.wait, self.handle.as_raw_handle(), timeout) };
    }

    /// Stop watching.
    ///
    /// New activations stop being queued, but a callback already queued still
    /// runs; use [`ThreadpoolWait::cancel_pending`] to drop those as well.
    pub fn disarm(&self) {
        // SAFETY: `wait` is valid for the lifetime of self; a null handle is the
        // documented way to cancel a pending wait.
        unsafe { SetThreadpoolWait(self.wait, ptr::null_mut(), ptr::null()) };
    }

    /// Block until all queued and executing callbacks have completed.
    pub fn wait(&self) {
        // SAFETY: `wait` is valid for the lifetime of self.
        unsafe { WaitForThreadpoolWaitCallbacks(self.wait, FALSE) };
    }

    /// Cancel callbacks that have not yet started, then wait for any currently
    /// executing callback to finish.
    ///
    /// Disarm first, or a callback that rearms could queue a fresh activation
    /// while this is draining.
    pub fn cancel_pending(&self) {
        // SAFETY: `wait` is valid for the lifetime of self. A cancelled wait
        // callback owns no storage, so dropping queued callbacks orphans nothing.
        unsafe { WaitForThreadpoolWaitCallbacks(self.wait, TRUE) };
    }
}

impl Drop for ThreadpoolWait {
    fn drop(&mut self) {
        // Disarm before draining: a callback that rearms would otherwise queue a
        // fresh activation while the drain is in progress and never settle.
        self.disarm();
        self.cancel_pending();

        // SAFETY: no callback can be queued or executing, so the object can be
        // closed and the context freed exactly once. `handle` is dropped after
        // this, when its field is dropped, so it outlives the wait object.
        unsafe {
            CloseThreadpoolWait(self.wait);
            drop(Box::from_raw(self.context));
        }
    }
}

#[cfg(test)]
mod tests;
