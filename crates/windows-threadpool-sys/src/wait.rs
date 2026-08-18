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
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle};
use std::ptr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::time::Duration;

use windows_sys::Win32::Foundation::{FALSE, FILETIME, HANDLE, TRUE, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::{
    CloseThreadpoolWait, CreateEventW, CreateThreadpoolWait, PTP_CALLBACK_INSTANCE, PTP_WAIT,
    SetThreadpoolWait, WaitForThreadpoolWaitCallbacks,
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

/// A handle the thread pool is able to wait on.
///
/// The pool does not support every waitable object: a mutex handle in
/// particular produces undefined behaviour rather than an error. Requiring this
/// type instead of a bare [`OwnedHandle`] moves that precondition from prose
/// into the type system, so a safe caller cannot reach the undefined case.
///
/// Construct one safely with [`WaitableHandle::event`], or vouch for a handle
/// obtained elsewhere with the narrow [`WaitableHandle::assume_waitable`] seam.
/// This mirrors `UnassociatedEndpoint` in `windows-overlapped-io-sys`, which
/// pairs a safe `open` with an `assume_overlapped` escape hatch for the same
/// reason.
#[derive(Debug)]
pub struct WaitableHandle {
    handle: OwnedHandle,
}

impl WaitableHandle {
    /// Create an event and wrap it as a waitable handle.
    ///
    /// An event is always a supported wait target, so this needs no `unsafe`.
    /// A `manual_reset` event stays signalled until it is reset; an auto-reset
    /// event returns to unsignalled as soon as one wait is satisfied, which
    /// makes it the usual choice for handing off work one activation at a time.
    ///
    /// # Errors
    ///
    /// Returns the error from `CreateEventW`.
    pub fn event(manual_reset: bool, initially_signalled: bool) -> io::Result<Self> {
        // SAFETY: creating an unnamed event with default security attributes;
        // all pointer arguments are null by design.
        let raw = unsafe {
            CreateEventW(
                ptr::null(),
                if manual_reset { TRUE } else { FALSE },
                if initially_signalled { TRUE } else { FALSE },
                ptr::null(),
            )
        };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the call returned a fresh, exclusively owned event handle.
        Ok(Self {
            handle: unsafe { OwnedHandle::from_raw_handle(raw) },
        })
    }

    /// Wrap a handle whose wait support the caller vouches for.
    ///
    /// This is the extensibility seam for wait targets this crate cannot create
    /// itself -- semaphores, waitable timers, processes, threads, console input,
    /// change notifications, and so on.
    ///
    /// # Safety
    ///
    /// The caller guarantees that:
    ///
    /// - the handle is a waitable object the thread pool supports, and in
    ///   particular is **not a mutex**, which the SDK does not support and which
    ///   yields undefined behaviour rather than an error; and
    /// - ownership transfers exclusively into the returned value, so nothing
    ///   else closes the handle while a wait on it is pending.
    #[must_use]
    pub unsafe fn assume_waitable(handle: OwnedHandle) -> Self {
        Self { handle }
    }

    /// Borrow the underlying handle, for signalling or inspecting it.
    #[must_use]
    pub fn handle(&self) -> BorrowedHandle<'_> {
        self.handle.as_handle()
    }

    /// Consume the wrapper and recover the owned handle.
    #[must_use]
    pub fn into_handle(self) -> OwnedHandle {
        self.handle
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
    /// How many callers are currently suppressing re-arming: zero means allowed.
    ///
    /// Arming takes this lock and does nothing while the count is non-zero, so a
    /// callback that re-arms cannot start watching again after a disarm from
    /// outside: without it, a drain could complete with the object armed again,
    /// and for `Drop` that meant closing the object and freeing its context with
    /// a fresh callback queued against them.
    ///
    /// A count rather than a flag because suppression has two users with
    /// different lifetimes: [`ThreadpoolWait::stop_and_drain`] raises it and
    /// lowers it again, while `Drop` raises it permanently. With a flag, a
    /// `stop_and_drain` finishing would clear a suppression that another
    /// concurrent one still needed.
    ///
    /// The lock is only ever held across the native `SetThreadpoolWait` call,
    /// never across a callback drain, which would deadlock a callback that
    /// happened to be blocked on it.
    suppress_rearm: Mutex<u32>,
    callback: Box<dyn Fn(&WaitActivation<'_>) + Send + Sync + 'static>,
}

impl WaitContext {
    /// Lock the suppression count, recovering from a panicking holder.
    fn suppression(&self) -> std::sync::MutexGuard<'_, u32> {
        self.suppress_rearm
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// Start suppressing re-arming, and disarm under the same acquisition.
    ///
    /// Doing both under one lock is what makes the pair atomic against a
    /// callback: a re-arm either lands entirely before this, or is suppressed by
    /// it. The lock is released before any drain.
    fn suppress_and_disarm(&self) {
        let mut suppressed = self.suppression();
        *suppressed = suppressed.saturating_add(1);
        let wait = self.wait.load(Ordering::Acquire);
        if wait != 0 {
            // SAFETY: `wait` is this object's live PTP_WAIT, published before any
            // callback could run and valid until Drop closes it.
            unsafe { disarm_raw(wait) };
        }
    }

    /// Stop suppressing re-arming.
    fn release_suppression(&self) {
        let mut suppressed = self.suppression();
        *suppressed = suppressed.saturating_sub(1);
    }
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

    /// Borrow the handle this activation was for.
    ///
    /// The wait owns the handle and outlives every callback, so it is open for
    /// the duration of this borrow. This is what makes the documented way out of
    /// the overlap hazard on [`rearm`](Self::rearm) reachable: a callback
    /// watching a manual-reset event can reset it before re-arming, so the next
    /// activation waits for a fresh signal instead of starting immediately
    /// alongside this one.
    #[must_use]
    pub fn handle(&self) -> BorrowedHandle<'_> {
        // SAFETY: the wait owns this handle and cannot be dropped while a
        // callback is running, so it is open for at least this borrow.
        unsafe { BorrowedHandle::borrow_raw(self.ctx.handle) }
    }

    /// Arm the wait again, so the next signal or timeout activates it.
    ///
    /// `timeout` of `None` waits indefinitely. This is the mechanism the SDK
    /// requires for repeated waits: an activation consumes the arming, so a
    /// callback that wants to keep watching must rearm from inside itself.
    ///
    /// # This can overlap the callback with itself
    ///
    /// Re-arming takes effect immediately, and the pool activates as soon as the
    /// handle is signalled. If the handle is *still signalled* when this is
    /// called -- which is the normal state of a manual-reset event -- the next
    /// activation is queued at once and can begin before the current callback
    /// returns. Re-arming early in a long callback therefore runs it
    /// concurrently with itself, repeatedly: a 20ms callback that re-armed at
    /// its start was measured entering 7529 times in 400ms, 5110 of those
    /// overlapping an earlier entry.
    ///
    /// This is not the guarantee [`TimerFiring::rearm_after`] gives. A one-shot
    /// timer's re-arm is deferred until the callback returns, precisely so
    /// firings stay sequential; a wait's re-arm is not, because the SDK requires
    /// the wait to be re-armed for the handle's *current* signal state to be
    /// observed.
    ///
    /// Either reset the handle before re-arming, using
    /// [`handle`](Self::handle), so the next activation waits for a fresh
    /// signal:
    ///
    /// ```no_run
    /// # use std::os::windows::io::AsRawHandle;
    /// # use windows_sys::Win32::System::Threading::ResetEvent;
    /// # fn example(activation: &windows_threadpool_sys::wait::WaitActivation<'_>) {
    /// // SAFETY: the wait owns the event, so the handle is open here.
    /// unsafe { ResetEvent(activation.handle().as_raw_handle()) };
    /// activation.rearm(None);
    /// # }
    /// ```
    ///
    /// or accept the concurrency and make everything the callback touches
    /// tolerate it. An auto-reset event does not have this problem, because the
    /// wait consumes the signal.
    ///
    /// [`TimerFiring::rearm_after`]: crate::timer::TimerFiring::rearm_after
    ///
    /// # Teardown
    ///
    /// Re-arming after the object has begun tearing down does nothing, so a
    /// callback racing [`ThreadpoolWait`]'s `Drop` cannot leave the object armed
    /// behind it.
    pub fn rearm(&self, timeout: Option<Duration>) {
        let _ = self.rearm_reporting(timeout);
    }

    /// [`rearm`](Self::rearm), reporting whether the arming actually happened.
    ///
    /// Returns `false` when the request was suppressed because the object is
    /// tearing down. The public entry point discards this, because a caller
    /// cannot act on it: by the time it could look, the object is gone. Tests
    /// use it to observe the suppression directly, which is otherwise only
    /// visible as the absence of undefined behaviour.
    pub(crate) fn rearm_reporting(&self, timeout: Option<Duration>) -> bool {
        // Taken before arming and held across it, so this either happens before
        // a suppressing caller raises the count or is suppressed by it -- never
        // in between.
        let suppressed = self.ctx.suppression();
        if *suppressed > 0 {
            return false;
        }
        let wait = self.ctx.wait.load(Ordering::Acquire);
        debug_assert_ne!(
            wait, 0,
            "the wait object must be published before callbacks"
        );
        // SAFETY: `wait` is this object's live PTP_WAIT, published before any
        // callback could run, and `handle` is owned by that object so it is
        // still open. The timeout, if any, is a live stack value for the call.
        unsafe { arm_raw(wait, self.ctx.handle, timeout) };
        drop(suppressed);
        true
    }
}

/// Stop a raw wait object.
///
/// SAFETY: `wait` must be a live `PTP_WAIT`.
pub(crate) unsafe fn disarm_raw(wait: PTP_WAIT) {
    // SAFETY: forwarded; a null handle is the documented way to cancel a wait.
    unsafe { SetThreadpoolWait(wait, ptr::null_mut(), ptr::null()) };
}

/// Arm a raw wait object against a borrowed handle.
///
/// SAFETY: `wait` must be a live `PTP_WAIT` and `handle` must stay open until
/// the wait is disarmed or the object released.
pub(crate) unsafe fn arm_member(wait: PTP_WAIT, handle: &OwnedHandle, timeout: Option<Duration>) {
    // SAFETY: forwarded from this function's own contract.
    unsafe { arm_raw(wait, handle.as_raw_handle(), timeout) };
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
/// Unlike [`ThreadpoolTimer`](crate::timer::ThreadpoolTimer), **the callback can
/// run concurrently with itself**: a wait's re-arm takes effect immediately, so
/// re-arming while the handle is still signalled queues the next activation
/// before the current callback returns. See [`WaitActivation::rearm`] for the
/// measurements and the two ways to avoid it.
///
/// # Examples
///
/// Watch an event once. The wait takes ownership of the handle, and
/// [`ThreadpoolWait::handle`] borrows it back for signalling:
///
/// ```
/// use std::os::windows::io::AsRawHandle;
/// use std::sync::mpsc;
/// use windows_sys::Win32::System::Threading::SetEvent;
/// use windows_threadpool_sys::wait::{ThreadpoolWait, WaitResult, WaitableHandle};
///
/// let event = WaitableHandle::event(true, false)?;
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
/// # use std::os::windows::io::AsRawHandle;
/// # use std::sync::Arc;
/// # use std::sync::atomic::{AtomicUsize, Ordering};
/// # use windows_sys::Win32::System::Threading::SetEvent;
/// use windows_threadpool_sys::wait::{ThreadpoolWait, WaitableHandle};
///
/// let event = WaitableHandle::event(false, false)?;
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
    /// Taking a [`WaitableHandle`] rather than a bare handle is what keeps this
    /// constructor safe: the thread pool does not support every waitable object,
    /// and a mutex handle in particular is undefined rather than an error.
    ///
    /// # Errors
    ///
    /// Returns the error from `CreateThreadpoolWait`.
    pub fn new<F>(
        handle: WaitableHandle,
        callback: F,
        env: Option<&mut CallbackEnviron<'_>>,
    ) -> io::Result<Self>
    where
        F: Fn(&WaitActivation<'_>) + Send + Sync + 'static,
    {
        let handle = handle.into_handle();
        let context = Box::into_raw(Box::new(WaitContext {
            wait: AtomicIsize::new(0),
            handle: handle.as_raw_handle(),
            suppress_rearm: Mutex::new(0),
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

    /// Let every queued callback run, and block until none is executing.
    ///
    /// This does **not** leave a self-re-arming wait idle: a callback running
    /// during this call can [`rearm`](WaitActivation::rearm) before it returns,
    /// so the object is watching again when this returns. Use
    /// [`stop_and_drain`](Self::stop_and_drain) to reach quiescence.
    pub fn wait(&self) {
        // SAFETY: `wait` is valid for the lifetime of self.
        unsafe { WaitForThreadpoolWaitCallbacks(self.wait, FALSE) };
    }

    /// Drop callbacks that have not started, then wait for any executing one.
    ///
    /// Like [`wait`](Self::wait), this does not by itself leave a self-re-arming
    /// wait idle: it does not suppress the re-arm of a callback that is already
    /// running. Use [`stop_and_drain`](Self::stop_and_drain) when the wait must
    /// actually be quiescent afterwards.
    pub fn cancel_pending(&self) {
        // SAFETY: `wait` is valid for the lifetime of self. A cancelled wait
        // callback owns no storage, so dropping queued callbacks orphans nothing.
        unsafe { WaitForThreadpoolWaitCallbacks(self.wait, TRUE) };
    }

    /// Stop watching and block until the wait is idle, leaving it reusable.
    ///
    /// This exists because neither [`disarm`](Self::disarm) nor
    /// [`cancel_pending`](Self::cancel_pending) can stop a self-re-arming wait on
    /// its own: a callback already running can call [`WaitActivation::rearm`]
    /// after a disarm from outside has taken effect. This suppresses re-arming
    /// for the duration of the call, using the same mechanism `Drop` uses, and
    /// lifts the suppression before returning so the wait can be armed again.
    ///
    /// # What this guarantees
    ///
    /// On return, provided no other thread arms the wait during the call:
    ///
    /// - no callback is queued or executing, and
    /// - the object is not watching -- a re-arm requested by a callback that ran
    ///   during the call is discarded rather than deferred.
    ///
    /// # What it does not
    ///
    /// **A concurrent [`arm`](Self::arm) from another thread is not excluded.**
    /// `ThreadpoolWait` is `Sync` and `arm` takes `&self`, so it does not pass
    /// through the suppression this uses, and nothing in this crate orders such
    /// a call against this one. A caller needing the wait to be provably idle
    /// must ensure nothing else arms it for the duration, by owning it
    /// exclusively or serializing access to it.
    ///
    /// Calling this from inside the wait's own callback would deadlock, because
    /// it waits for that callback to finish.
    pub fn stop_and_drain(&self) {
        // SAFETY: the context outlives every callback and is freed only by Drop,
        // which cannot run while this borrow of self is alive.
        let ctx = unsafe { &*self.context };
        ctx.suppress_and_disarm();
        // Drained with the lock released: a callback blocked on it would
        // otherwise never finish, and this would never return.
        self.cancel_pending();
        ctx.release_suppression();
    }

    /// Give up ownership, returning the raw object, its callback context, and
    /// the watched handle.
    ///
    /// Used only by [`crate::cleanup_group::CleanupGroup`], which takes over all
    /// three. The handle must go with them: the pool may still be watching it
    /// until the group releases the member, so it cannot be closed when the
    /// borrowing member goes out of scope.
    pub(crate) fn into_parts(self) -> (PTP_WAIT, *mut core::ffi::c_void, OwnedHandle) {
        let this = std::mem::ManuallyDrop::new(self);
        // SAFETY: `this` is never dropped, so moving the handle out cannot be
        // observed by a later drop of the original value.
        let handle = unsafe { ptr::read(&this.handle) };
        (this.wait, this.context.cast(), handle)
    }

    /// Free a context returned by [`ThreadpoolWait::into_parts`].
    ///
    /// # Safety
    ///
    /// `context` must come from `into_parts` on this type, its object must
    /// already have been released, and it must be freed exactly once.
    pub(crate) unsafe fn drop_context(context: *mut core::ffi::c_void) {
        // SAFETY: forwarded from this function's own contract.
        drop(unsafe { Box::from_raw(context.cast::<WaitContext>()) });
    }
}

impl Drop for ThreadpoolWait {
    fn drop(&mut self) {
        // Close the door on re-arming before disarming, and do both under the
        // same lock. Disarming alone is not enough: a callback already running
        // could re-arm afterwards, the drain below could then return with the
        // object armed, and the close and context free would race a freshly
        // queued callback.
        // SAFETY: the context outlives every callback; Drop frees it below,
        // after the drain.
        let ctx = unsafe { &*self.context };
        // Raised and never released: unlike `stop_and_drain`, there is no
        // afterwards for this object.
        ctx.suppress_and_disarm();
        // The lock is released before draining: a callback blocked on it would
        // otherwise never finish, and this wait would never return.
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
