// Copyright (c) 2026 Mike Grier
//! Thread-pool timers: `CreateThreadpoolTimer` / `SetThreadpoolTimer` /
//! `WaitForThreadpoolTimerCallbacks` / `CloseThreadpoolTimer`.
//!
//! A timer object queues its callback once at a due time, or repeatedly at a
//! fixed period. The due time is expressed two ways, and the difference is
//! behavioural rather than cosmetic:
//!
//! - **Relative** ([`ThreadpoolTimer::set_after`], [`ThreadpoolTimer::set_periodic`])
//!   counts only time the system is awake, so sleep and hibernation do not
//!   consume the delay.
//! - **Absolute** ([`ThreadpoolTimer::set_at`]) names a wall-clock instant, which
//!   sleep and hibernation *do* pass through -- a timer set for an instant that
//!   elapsed while the machine slept fires promptly on resume.
//!
//! Disarming with [`ThreadpoolTimer::disarm`] stops new callbacks being queued
//! but does not retract callbacks already queued, which is why teardown disarms
//! first and drains second.

use std::io;
use std::ptr;
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
/// becomes the furthest representable relative time, which is far past any
/// practical process lifetime.
fn relative_filetime(delay: Duration) -> FILETIME {
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
/// Instants at or before the `FILETIME` epoch clamp to zero, which the pool
/// treats as immediately due.
fn absolute_filetime(when: SystemTime) -> FILETIME {
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
fn millis_u32(duration: Duration) -> u32 {
    u32::try_from(duration.as_millis()).unwrap_or(u32::MAX)
}

/// Heap-allocated callback state kept alive for the lifetime of the timer.
struct TimerContext {
    callback: Box<dyn Fn() + Send + Sync + 'static>,
}

/// Trampoline from the raw `PTP_TIMER_CALLBACK` ABI into the boxed closure.
///
/// SAFETY: `context` must point to a live [`TimerContext`] for the entire
/// duration of every callback invocation, which [`ThreadpoolTimer`]'s `Drop`
/// ordering guarantees.
unsafe extern "system" fn timer_trampoline(
    _instance: PTP_CALLBACK_INSTANCE,
    context: *mut core::ffi::c_void,
    _timer: PTP_TIMER,
) {
    // SAFETY: context is a valid *mut TimerContext for the full callback duration.
    let ctx = unsafe { &*(context as *const TimerContext) };
    // A panic must never unwind across the FFI boundary into the pool's frame.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        (ctx.callback)();
    }));
}

/// An owned thread-pool timer object.
///
/// A newly created timer is idle; arm it with [`ThreadpoolTimer::set_after`],
/// [`ThreadpoolTimer::set_periodic`], or [`ThreadpoolTimer::set_at`], and stop it
/// with [`ThreadpoolTimer::disarm`]. Arming again replaces the previous setting
/// rather than adding to it.
///
/// [`Drop`] disarms the timer before draining callbacks, so a periodic timer
/// cannot queue new work during teardown, and the captured closure stays valid
/// for the full lifetime of every callback execution.
///
/// # Examples
///
/// A one-shot timer:
///
/// ```
/// use std::sync::Arc;
/// use std::sync::mpsc;
/// use std::time::Duration;
/// use windows_threadpool_sys::timer::ThreadpoolTimer;
///
/// let (tx, rx) = mpsc::channel();
/// let sender = Arc::new(std::sync::Mutex::new(tx));
///
/// let timer = ThreadpoolTimer::new(move || {
///     let _ = sender.lock().expect("send").send(());
/// }, None)?;
///
/// timer.set_after(Duration::from_millis(10));
/// rx.recv().expect("the timer should fire");
/// # Ok::<(), std::io::Error>(())
/// ```
///
/// A periodic timer, stopped once enough ticks have been seen. Disarm before
/// dropping if you want teardown to be explicit rather than implicit:
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
/// let timer = ThreadpoolTimer::new(move || {
///     counter.fetch_add(1, Ordering::SeqCst);
/// }, None)?;
///
/// timer.set_periodic(Duration::from_millis(1), Duration::from_millis(1));
/// while ticks.load(Ordering::SeqCst) < 3 {
///     std::thread::yield_now();
/// }
///
/// timer.disarm();
/// timer.wait();
/// assert!(ticks.load(Ordering::SeqCst) >= 3);
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
        F: Fn() + Send + Sync + 'static,
    {
        let context = Box::into_raw(Box::new(TimerContext {
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

        Ok(Self { timer, context })
    }

    /// Fire once, `delay` from now.
    ///
    /// The delay counts only time the system is awake. A zero delay makes the
    /// timer due immediately.
    pub fn set_after(&self, delay: Duration) {
        self.arm(relative_filetime(delay), 0, 0);
    }

    /// Fire once after `delay`, then repeatedly every `period`.
    ///
    /// A zero `period` degenerates to a one-shot timer, matching
    /// [`ThreadpoolTimer::set_after`]. Periods are expressed in whole
    /// milliseconds and saturate at `u32::MAX`.
    pub fn set_periodic(&self, delay: Duration, period: Duration) {
        self.arm(relative_filetime(delay), millis_u32(period), 0);
    }

    /// Fire once at the wall-clock instant `when`.
    ///
    /// Unlike the relative forms, an absolute due time passes through sleep and
    /// hibernation: if `when` elapses while the machine is asleep, the timer
    /// fires promptly on resume. An instant already in the past fires
    /// immediately.
    pub fn set_at(&self, when: SystemTime) {
        self.arm(absolute_filetime(when), 0, 0);
    }

    /// Arm with an explicit coalescing window.
    ///
    /// `window` is the tolerance the system may add to the due time so it can
    /// group this timer with other expirations and wake the processor less
    /// often. A larger window trades timing precision for power.
    pub fn set_after_with_window(&self, delay: Duration, period: Duration, window: Duration) {
        self.arm(
            relative_filetime(delay),
            millis_u32(period),
            millis_u32(window),
        );
    }

    /// Stop the timer.
    ///
    /// New callbacks stop being queued, but callbacks already queued still run;
    /// use [`ThreadpoolTimer::cancel_pending`] to drop those as well. Disarming
    /// an idle timer is a no-op.
    pub fn disarm(&self) {
        // SAFETY: timer is valid for the lifetime of self; a null due time is
        // the documented way to stop a timer.
        unsafe { SetThreadpoolTimer(self.timer, ptr::null(), 0, 0) };
    }

    /// Whether the timer currently has a due time.
    ///
    /// This reports whether the timer has been armed and not since disarmed. It
    /// is **not** a prediction that the timer will fire again: expiring does not
    /// clear the due time, so a one-shot timer still reports `true` after its
    /// callback has run. Only [`ThreadpoolTimer::disarm`] makes it `false`.
    ///
    /// Track whether work remains outstanding in the callback itself if that is
    /// the question being asked.
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
    /// Disarm first if the timer is periodic, or it may queue a fresh callback
    /// while this is draining.
    pub fn cancel_pending(&self) {
        // SAFETY: timer is valid for the lifetime of self. Unlike thread-pool
        // I/O, a cancelled timer callback owns no storage, so dropping queued
        // callbacks orphans nothing.
        unsafe { WaitForThreadpoolTimerCallbacks(self.timer, TRUE) };
    }

    fn arm(&self, due: FILETIME, period_ms: u32, window_ms: u32) {
        // SAFETY: timer is valid for the lifetime of self, and `due` is a live
        // stack value read only for the duration of the call.
        unsafe { SetThreadpoolTimer(self.timer, &due, period_ms, window_ms) };
    }
}

impl Drop for ThreadpoolTimer {
    fn drop(&mut self) {
        // Disarm before draining: a periodic timer would otherwise queue a fresh
        // callback while the drain is in progress and never settle.
        self.disarm();
        // Drop callbacks that have not started and wait for one that has, so the
        // context outlives every execution.
        self.cancel_pending();

        // SAFETY: no callback can be queued or executing, so the object can be
        // closed and the context freed exactly once.
        unsafe {
            CloseThreadpoolTimer(self.timer);
            drop(Box::from_raw(self.context));
        }
    }
}

#[cfg(test)]
mod tests;
