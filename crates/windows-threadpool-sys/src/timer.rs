// Copyright (c) 2026 Mike Grier
//! Thread-pool timers: `CreateThreadpoolTimer` / `SetThreadpoolTimer` /
//! `WaitForThreadpoolTimerCallbacks` / `CloseThreadpoolTimer`.
//!
//! Two types share this machinery, and they differ in the one property that
//! matters when writing the callback:
//!
//! - [`ThreadpoolTimer`] fires **exactly once per arming**. A firing cannot overlap
//!   another firing of the same timer, because there is no next one until the
//!   caller arms it again.
//! - [`ThreadpoolPeriodicTimer`] repeats on a fixed period, and the pool may queue the
//!   next callback **while the previous one is still running**. Its callback
//!   must tolerate overlapping with itself.
//!
//! The platform models both with one object and a `period` argument. This crate
//! separates them so that concurrency contract belongs to a type rather than to
//! an argument that is easy to skim past.
//!
//! # Choosing between them
//!
//! Want a fixed cadence, and the callback is short or safe to overlap? Use
//! [`ThreadpoolPeriodicTimer`]. Want the next delay measured from when the previous
//! callback *finished*, with no overlap possible? Use a [`ThreadpoolTimer`] and re-arm it
//! from inside its own callback with [`TimerFiring::rearm_after`].
//!
//! # Due times
//!
//! Relative due times ([`ThreadpoolTimer::set_after`]) count only time the system is
//! awake, so sleep and hibernation do not consume the delay. Absolute due times
//! ([`ThreadpoolTimer::set_at`]) name a wall-clock instant, which sleep and hibernation
//! *do* pass through: a timer set for an instant that elapsed while the machine
//! slept fires promptly on resume.

mod periodic;

pub use periodic::{PeriodicTick, ThreadpoolPeriodicTimer};

use std::io;
use std::ptr;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use windows_sys::Win32::Foundation::{FALSE, FILETIME, TRUE};
use windows_sys::Win32::System::Threading::{
    CloseThreadpoolTimer, CreateThreadpoolTimer, IsThreadpoolTimerSet, PTP_CALLBACK_INSTANCE,
    PTP_TIMER, SetThreadpoolTimer, WaitForThreadpoolTimerCallbacks,
};

use crate::callback_env::CallbackEnviron;

/// Conversion constants for Windows `FILETIME`, which counts 100-nanosecond
/// intervals since 1601-01-01 UTC. Changing any value is a breaking change.
mod filetime {
    /// 100-nanosecond intervals per second.
    pub const TICKS_PER_SECOND: u64 = 10_000_000;
    /// Nanoseconds per 100-nanosecond interval.
    pub const NANOS_PER_TICK: u32 = 100;
    /// Seconds between the `FILETIME` epoch (1601-01-01) and the Unix epoch.
    pub const SECONDS_1601_TO_1970: u64 = 11_644_473_600;
}

/// Split a 64-bit tick count into the `FILETIME` field pair.
fn filetime_from_ticks(ticks: i64) -> FILETIME {
    let bits = ticks as u64;
    FILETIME {
        dwLowDateTime: bits as u32,
        dwHighDateTime: (bits >> 32) as u32,
    }
}

/// Convert a delay into the negative tick count that means "relative" to
/// `SetThreadpoolTimer`.
///
/// Saturates rather than overflowing: a delay beyond the representable range
/// becomes the furthest representable relative time, far past any practical
/// process lifetime.
pub(crate) fn relative_filetime(delay: Duration) -> FILETIME {
    let ticks = delay
        .as_secs()
        .saturating_mul(filetime::TICKS_PER_SECOND)
        .saturating_add(u64::from(delay.subsec_nanos() / filetime::NANOS_PER_TICK));
    let ticks = i64::try_from(ticks).unwrap_or(i64::MAX);
    filetime_from_ticks(-ticks)
}

/// Convert a wall-clock instant into the positive tick count that means
/// "absolute" to `SetThreadpoolTimer`.
///
/// Instants at or before the Unix epoch clamp to zero, which the pool treats as
/// immediately due.
pub(crate) fn absolute_filetime(when: SystemTime) -> FILETIME {
    let since_unix = when.duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO);
    let ticks = since_unix
        .as_secs()
        .saturating_add(filetime::SECONDS_1601_TO_1970)
        .saturating_mul(filetime::TICKS_PER_SECOND)
        .saturating_add(u64::from(
            since_unix.subsec_nanos() / filetime::NANOS_PER_TICK,
        ));
    filetime_from_ticks(i64::try_from(ticks).unwrap_or(i64::MAX))
}

/// Clamp a period or coalescing window to the `u32` millisecond field.
pub(crate) fn millis_u32(duration: Duration) -> u32 {
    u32::try_from(duration.as_millis()).unwrap_or(u32::MAX)
}

/// Arm a raw timer object.
///
/// SAFETY: `timer` must be a live `PTP_TIMER`.
pub(crate) unsafe fn arm_raw(timer: PTP_TIMER, due: FILETIME, period_ms: u32, window_ms: u32) {
    // SAFETY: forwarded from this function's contract; `due` is read only for
    // the duration of the call.
    unsafe { SetThreadpoolTimer(timer, &due, period_ms, window_ms) };
}

/// Stop a raw timer object.
///
/// SAFETY: `timer` must be a live `PTP_TIMER`.
pub(crate) unsafe fn disarm_raw(timer: PTP_TIMER) {
    // SAFETY: forwarded; a null due time is the documented way to stop a timer.
    unsafe { SetThreadpoolTimer(timer, ptr::null(), 0, 0) };
}

/// Heap-allocated callback state kept alive for the lifetime of the timer.
///
/// `timer` is filled in after `CreateThreadpoolTimer` returns, because re-arming
/// from inside a callback needs the object the callback belongs to.
pub(crate) struct TimerContext {
    pub(crate) timer: AtomicIsize,
    callback: Box<dyn Fn(&TimerFiring<'_>) + Send + Sync + 'static>,
}

/// One firing of a [`ThreadpoolTimer`], handed to its callback.
///
/// The timer is not armed while the callback runs, so re-arming from here is
/// what produces repetition whose delay is measured from the *end* of this
/// callback -- repetition that can never overlap itself.
pub struct TimerFiring<'ctx> {
    ctx: &'ctx TimerContext,
}

impl std::fmt::Debug for TimerFiring<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TimerFiring").finish_non_exhaustive()
    }
}

impl TimerFiring<'_> {
    /// Arm the timer again to fire once, `delay` from now.
    ///
    /// Because "now" is inside the callback, successive delays are measured from
    /// the end of each firing rather than from a fixed schedule. That is the
    /// non-overlapping alternative to [`ThreadpoolPeriodicTimer`].
    pub fn rearm_after(&self, delay: Duration) {
        let timer = self.ctx.timer.load(Ordering::Acquire);
        debug_assert_ne!(timer, 0, "the timer must be published before callbacks");
        // SAFETY: `timer` is this object's live PTP_TIMER, published before any
        // callback could run.
        unsafe { arm_raw(timer, relative_filetime(delay), 0, 0) };
    }

    /// Arm the timer again to fire once at the wall-clock instant `when`.
    pub fn rearm_at(&self, when: SystemTime) {
        let timer = self.ctx.timer.load(Ordering::Acquire);
        debug_assert_ne!(timer, 0, "the timer must be published before callbacks");
        // SAFETY: as above.
        unsafe { arm_raw(timer, absolute_filetime(when), 0, 0) };
    }
}

/// Trampoline from the raw `PTP_TIMER_CALLBACK` ABI into the boxed closure.
///
/// SAFETY: `context` must point to a live [`TimerContext`] for the entire
/// duration of every callback invocation, which [`ThreadpoolTimer`]'s `Drop` ordering
/// guarantees.
unsafe extern "system" fn timer_trampoline(
    _instance: PTP_CALLBACK_INSTANCE,
    context: *mut core::ffi::c_void,
    _timer: PTP_TIMER,
) {
    // SAFETY: context is a valid *mut TimerContext for the full callback duration.
    let ctx = unsafe { &*(context as *const TimerContext) };
    let firing = TimerFiring { ctx };
    // A panic must never unwind across the FFI boundary into the pool's frame.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        (ctx.callback)(&firing);
    }));
}

/// An owned one-shot thread-pool timer.
///
/// Each arming produces exactly one firing, so a firing can never overlap
/// another firing of the same timer. Arm it with [`ThreadpoolTimer::set_after`] or
/// [`ThreadpoolTimer::set_at`], and stop it with [`ThreadpoolTimer::disarm`]. Arming again replaces
/// the previous setting rather than adding to it.
///
/// For repetition, either re-arm from inside the callback with
/// [`TimerFiring::rearm_after`] -- which keeps firings strictly sequential -- or
/// use [`ThreadpoolPeriodicTimer`] when a fixed cadence matters more than avoiding
/// overlap.
///
/// [`Drop`] disarms before draining callbacks, so the captured closure stays
/// valid for the full lifetime of every callback execution.
///
/// # Examples
///
/// Fire once:
///
/// ```
/// use std::sync::mpsc;
/// use std::time::Duration;
/// use windows_threadpool_sys::timer::ThreadpoolTimer;
///
/// let (tx, rx) = mpsc::channel();
/// let sender = std::sync::Mutex::new(tx);
///
/// let timer = ThreadpoolTimer::new(move |_firing| {
///     let _ = sender.lock().expect("send").send(());
/// }, None)?;
///
/// timer.set_after(Duration::from_millis(10));
/// rx.recv().expect("the timer should fire");
/// # Ok::<(), std::io::Error>(())
/// ```
///
/// Repeat without ever overlapping, by re-arming from inside the callback. The
/// gap is measured from the end of each firing, so a slow callback delays the
/// next one instead of racing it:
///
/// ```
/// use std::sync::Arc;
/// use std::sync::atomic::{AtomicUsize, Ordering};
/// use std::time::Duration;
/// use windows_threadpool_sys::timer::ThreadpoolTimer;
///
/// let ticks = Arc::new(AtomicUsize::new(0));
/// let counter = Arc::clone(&ticks);
///
/// let timer = ThreadpoolTimer::new(move |firing| {
///     // Stop after three firings by simply not re-arming.
///     if counter.fetch_add(1, Ordering::SeqCst) < 2 {
///         firing.rearm_after(Duration::from_millis(1));
///     }
/// }, None)?;
///
/// timer.set_after(Duration::from_millis(1));
/// while ticks.load(Ordering::SeqCst) < 3 {
///     std::thread::yield_now();
/// }
/// timer.disarm();
/// timer.wait();
/// assert_eq!(ticks.load(Ordering::SeqCst), 3);
/// # Ok::<(), std::io::Error>(())
/// ```
pub struct ThreadpoolTimer {
    timer: PTP_TIMER,
    // Kept alive as a raw pointer until Drop has disarmed and drained.
    context: *mut TimerContext,
}

// SAFETY: PTP_TIMER is a cross-thread pool object, and the context holds a
// callback that is Fn + Send + Sync; the pointer is only read until Drop frees
// it after all callbacks have finished.
unsafe impl Send for ThreadpoolTimer {}
unsafe impl Sync for ThreadpoolTimer {}

impl ThreadpoolTimer {
    /// Create an idle timer that invokes `callback` each time it expires.
    ///
    /// Pass `Some(env)` to select a private pool or callback priority; `None`
    /// uses the process-default pool with default priority.
    ///
    /// The callback runs on a shared, process-managed pool thread. It must
    /// restore any thread state it changes and must not terminate its thread. A
    /// panic inside it is caught at the FFI boundary rather than unwinding into
    /// the pool.
    ///
    /// # Errors
    ///
    /// Returns the error from `CreateThreadpoolTimer`.
    pub fn new<F>(callback: F, env: Option<&mut CallbackEnviron>) -> io::Result<Self>
    where
        F: Fn(&TimerFiring<'_>) + Send + Sync + 'static,
    {
        let context = Box::into_raw(Box::new(TimerContext {
            timer: AtomicIsize::new(0),
            callback: Box::new(callback),
        }));
        let env_ptr = env.map_or(ptr::null_mut(), |e| e.as_mut_ptr());

        // SAFETY: context is a valid heap pointer that outlives every callback,
        // and env_ptr is valid (or null) for the duration of this call.
        let timer = unsafe {
            CreateThreadpoolTimer(Some(timer_trampoline), context.cast(), env_ptr.cast_const())
        };

        if timer == 0 {
            let error = io::Error::last_os_error();
            // SAFETY: the pool never saw context; reclaim it immediately.
            unsafe { drop(Box::from_raw(context)) };
            return Err(error);
        }

        // Publish the object before any callback can run. The timer is not armed
        // yet, so no callback can observe the unpublished value.
        // SAFETY: context is live and exclusively ours until the first arming.
        unsafe { (*context).timer.store(timer, Ordering::Release) };

        Ok(Self { timer, context })
    }

    /// Fire once, `delay` from now.
    ///
    /// The delay counts only time the system is awake. A zero delay makes the
    /// timer due immediately.
    pub fn set_after(&self, delay: Duration) {
        // SAFETY: timer is valid for the lifetime of self.
        unsafe { arm_raw(self.timer, relative_filetime(delay), 0, 0) };
    }

    /// Fire once at the wall-clock instant `when`.
    ///
    /// Unlike a relative due time, an absolute one passes through sleep and
    /// hibernation: if `when` elapses while the machine is asleep, the timer
    /// fires promptly on resume. An instant already in the past fires
    /// immediately.
    pub fn set_at(&self, when: SystemTime) {
        // SAFETY: timer is valid for the lifetime of self.
        unsafe { arm_raw(self.timer, absolute_filetime(when), 0, 0) };
    }

    /// Fire once after `delay`, allowing the system a coalescing `window`.
    ///
    /// `window` is the tolerance the system may add to the due time so it can
    /// group this timer with other expirations and wake the processor less
    /// often. A larger window trades timing precision for power.
    pub fn set_after_with_window(&self, delay: Duration, window: Duration) {
        // SAFETY: timer is valid for the lifetime of self.
        unsafe { arm_raw(self.timer, relative_filetime(delay), 0, millis_u32(window)) };
    }

    /// Stop the timer.
    ///
    /// New callbacks stop being queued, but a callback already queued still
    /// runs; use [`ThreadpoolTimer::cancel_pending`] to drop those as well. Disarming an
    /// idle timer is a no-op.
    pub fn disarm(&self) {
        // SAFETY: timer is valid for the lifetime of self.
        unsafe { disarm_raw(self.timer) };
    }

    /// Whether the timer currently has a due time.
    ///
    /// This reports whether the timer has been armed and not since disarmed. It
    /// is **not** a prediction that the timer will fire again: expiring does not
    /// clear the due time, so a fired timer still reports `true`. Only
    /// [`ThreadpoolTimer::disarm`] makes it `false`.
    #[must_use]
    pub fn is_set(&self) -> bool {
        // SAFETY: timer is valid for the lifetime of self.
        unsafe { IsThreadpoolTimerSet(self.timer) != 0 }
    }

    /// Block until all queued and executing callbacks have completed.
    pub fn wait(&self) {
        // SAFETY: timer is valid for the lifetime of self.
        unsafe { WaitForThreadpoolTimerCallbacks(self.timer, FALSE) };
    }

    /// Cancel callbacks that have not yet started, then wait for any currently
    /// executing callback to finish.
    ///
    /// Disarm first if the callback re-arms, or it may queue a fresh firing
    /// while this is draining.
    pub fn cancel_pending(&self) {
        // SAFETY: timer is valid for the lifetime of self. A cancelled timer
        // callback owns no storage, so dropping queued callbacks orphans nothing.
        unsafe { WaitForThreadpoolTimerCallbacks(self.timer, TRUE) };
    }

    /// Give up ownership, returning the raw object and its callback context.
    ///
    /// Used only by [`crate::cleanup_group::CleanupGroup`], which takes over
    /// both: a group member is released by `CloseThreadpoolCleanupGroupMembers`
    /// and must not close itself, so this suppresses this type's `Drop`.
    pub(crate) fn into_parts(self) -> (PTP_TIMER, *mut core::ffi::c_void) {
        let this = std::mem::ManuallyDrop::new(self);
        (this.timer, this.context.cast())
    }

    /// Free a context returned by [`ThreadpoolTimer::into_parts`].
    ///
    /// # Safety
    ///
    /// `context` must come from `into_parts` on this type, its object must
    /// already have been released, and it must be freed exactly once.
    pub(crate) unsafe fn drop_context(context: *mut core::ffi::c_void) {
        // SAFETY: forwarded from this function's own contract.
        drop(unsafe { Box::from_raw(context.cast::<TimerContext>()) });
    }
}

impl Drop for ThreadpoolTimer {
    fn drop(&mut self) {
        // Disarm before draining: a callback that re-arms would otherwise queue a
        // fresh firing while the drain is in progress and never settle.
        self.disarm();
        self.cancel_pending();

        // SAFETY: no callback can be queued or executing, so the object can be
        // closed and the context freed exactly once.
        unsafe {
            CloseThreadpoolTimer(self.timer);
            drop(Box::from_raw(self.context));
        }
    }
}

impl std::fmt::Debug for ThreadpoolTimer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ThreadpoolTimer")
            .field("is_set", &self.is_set())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
