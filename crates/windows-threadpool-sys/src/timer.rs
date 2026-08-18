// Copyright (c) 2026 Mike Grier
//! Thread-pool timers: `CreateThreadpoolTimer` / `SetThreadpoolTimer` /
//! `WaitForThreadpoolTimerCallbacks` / `CloseThreadpoolTimer`.
//!
//! Two types share this machinery, and they differ in the one property that
//! matters when writing the callback:
//!
//! - [`ThreadpoolTimer`] fires **exactly once per arming**, and re-arming from
//!   inside its own callback is applied only after that callback returns, so
//!   repetition driven that way never overlaps itself.
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

use std::cell::Cell;
use std::io;
use std::ptr;
use std::sync::Mutex;
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

/// Saturate a coalescing window to the `u32` millisecond field.
///
/// This is the one length in either crate that saturates rather than being
/// rejected, and deliberately so. A window is a permission -- "you may delay
/// this firing by up to this much to batch it with others" -- and the pool is
/// always free to fire earlier, so a saturated window asks for less coalescing
/// rather than producing a wrong result. A truncated *buffer* silently loses
/// data, which is why those are rejected instead.
///
/// Periods also pass through this, but cannot reach the saturation: they are
/// validated against
/// [`MAX_PERIOD`](crate::timer::ThreadpoolPeriodicTimer::MAX_PERIOD) at
/// construction.
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
    /// How many callers are currently suppressing re-arming: zero means allowed.
    ///
    /// Applying a deferred re-arm takes this lock and does nothing while the
    /// count is non-zero. Deferring the re-arm to after the callback returns --
    /// which is what makes the delay run from the end of the firing -- moves it
    /// *past* any disarm performed from outside, so without this a drain could
    /// complete with a due time installed. For `Drop` that meant closing the
    /// object and freeing its context with a fresh callback queued against it.
    ///
    /// A count rather than a flag because suppression has two users with
    /// different lifetimes: [`ThreadpoolTimer::stop_and_drain`] raises it and
    /// lowers it again, while `Drop` raises it permanently. With a flag, a
    /// `stop_and_drain` finishing would clear a suppression that another
    /// concurrent one still needed.
    ///
    /// The lock is only ever held across the native `SetThreadpoolTimer` call,
    /// never across a callback drain, which would deadlock a callback that
    /// happened to be blocked on it.
    suppress_rearm: Mutex<u32>,
    /// Records, for tests, whether each deferred re-arm was actually applied.
    ///
    /// The suppression this observes happens after the callback returns and
    /// before the context is freed, so no user-reachable state can witness it;
    /// this Arc is cloned by the test, which therefore outlives the context.
    #[cfg(test)]
    rearm_observer: Mutex<Option<std::sync::Arc<Mutex<Vec<bool>>>>>,
    callback: Box<dyn Fn(&TimerFiring<'_>) + Send + Sync + 'static>,
}
impl TimerContext {
    /// Lock the suppression count, recovering from a panicking holder.
    fn suppression(&self) -> std::sync::MutexGuard<'_, u32> {
        self.suppress_rearm
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// Start suppressing re-arming, and disarm under the same acquisition.
    ///
    /// Doing both under one lock is what makes the pair atomic against a
    /// callback: a deferred re-arm either lands entirely before this, or is
    /// suppressed by it. The lock is released before any drain.
    fn suppress_and_disarm(&self) {
        let mut suppressed = self.suppression();
        *suppressed = suppressed.saturating_add(1);
        let timer = self.timer.load(Ordering::Acquire);
        if timer != 0 {
            // SAFETY: `timer` is this object's live PTP_TIMER, published before
            // any callback could run and valid until Drop closes it.
            unsafe { disarm_raw(timer) };
        }
    }

    /// Stop suppressing re-arming.
    fn release_suppression(&self) {
        let mut suppressed = self.suppression();
        *suppressed = suppressed.saturating_sub(1);
    }
}

/// A re-arming a callback asked for, applied once the callback has returned.
///
/// Applying it immediately would start the delay from the moment of the call
/// rather than from the end of the firing, which is both what the API documents
/// and what keeps firings from overlapping: a callback that re-armed early and
/// then ran longer than its delay could be entered again concurrently.
#[derive(Clone, Copy)]
enum PendingRearm {
    After(Duration),
    At(SystemTime),
}

/// One firing of a [`ThreadpoolTimer`], handed to its callback.
///
/// The timer is not armed while the callback runs, so re-arming from here is
/// what produces repetition whose delay is measured from the *end* of this
/// callback -- repetition that can never overlap itself.
pub struct TimerFiring<'ctx> {
    ctx: &'ctx TimerContext,
    /// What the callback asked for, applied by the trampoline after it returns.
    ///
    /// A `Cell` rather than a lock because the firing is borrowed only by the
    /// one callback invocation that owns it.
    pending: Cell<Option<PendingRearm>>,
}

impl std::fmt::Debug for TimerFiring<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TimerFiring").finish_non_exhaustive()
    }
}

impl TimerFiring<'_> {
    /// Arm the timer again to fire once, `delay` from now.
    ///
    /// The arming is applied after this callback returns, so `delay` is measured
    /// from the **end** of this firing regardless of where in the callback it is
    /// requested. That is what keeps successive firings strictly sequential: a
    /// callback that re-armed at its start and then ran longer than `delay`
    /// would otherwise be entered again while still running.
    ///
    /// Calling this more than once in a firing keeps the last request.
    pub fn rearm_after(&self, delay: Duration) {
        self.pending.set(Some(PendingRearm::After(delay)));
    }

    /// Arm the timer again to fire once at the wall-clock instant `when`.
    ///
    /// Like [`TimerFiring::rearm_after`], this is applied after the callback
    /// returns. An instant that has already passed by then fires immediately.
    pub fn rearm_at(&self, when: SystemTime) {
        self.pending.set(Some(PendingRearm::At(when)));
    }

    /// Apply whatever the callback asked for, once it has returned.
    ///
    /// Suppressed once teardown has begun, so a request made during the last
    /// callback cannot re-arm the timer behind `Drop`'s disarm.
    fn apply_pending(&self) {
        let applied = self.apply_pending_reporting();
        #[cfg(test)]
        if let Some(applied) = applied {
            let observer = self
                .ctx
                .rearm_observer
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .clone();
            if let Some(observer) = observer {
                observer
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .push(applied);
            }
        }
        let _ = applied;
    }

    /// Apply the pending re-arm, reporting whether it was actually installed.
    ///
    /// `None` means the callback asked for nothing; `Some(false)` means it asked
    /// but teardown suppressed the request.
    fn apply_pending_reporting(&self) -> Option<bool> {
        let pending = self.pending.get()?;
        // Taken before arming and held across it, so this either happens before
        // a suppressing caller raises the count or is suppressed by it -- never
        // in between.
        let suppressed = self.ctx.suppression();
        if *suppressed > 0 {
            return Some(false);
        }
        let timer = self.ctx.timer.load(Ordering::Acquire);
        debug_assert_ne!(timer, 0, "the timer must be published before callbacks");
        let due = match pending {
            PendingRearm::After(delay) => relative_filetime(delay),
            PendingRearm::At(when) => absolute_filetime(when),
        };
        // SAFETY: `timer` is this object's live PTP_TIMER, published before any
        // callback could run.
        unsafe { arm_raw(timer, due, 0, 0) };
        drop(suppressed);
        Some(true)
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
    let firing = TimerFiring {
        ctx,
        pending: Cell::new(None),
    };
    // A panic must never unwind across the FFI boundary into the pool's frame.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        (ctx.callback)(&firing);
    }));
    // Applied only now that the callback has returned, so a requested delay runs
    // from the end of this firing and the next one cannot overlap it. A callback
    // that panicked after requesting a re-arm still gets it, matching the
    // accounting behaviour of the crate's other trampolines.
    firing.apply_pending();
}

/// An owned one-shot thread-pool timer.
///
/// Each arming produces exactly one firing. Arm it with
/// [`ThreadpoolTimer::set_after`] or [`ThreadpoolTimer::set_at`], and stop it
/// with [`ThreadpoolTimer::disarm`]. Arming again replaces the previous setting
/// rather than adding to it.
///
/// For repetition, either re-arm from inside the callback with
/// [`TimerFiring::rearm_after`] -- which keeps firings strictly sequential -- or
/// use [`ThreadpoolPeriodicTimer`] when a fixed cadence matters more than
/// avoiding overlap.
///
/// # When firings can overlap
///
/// Re-arming through [`TimerFiring`] never overlaps: the request is applied
/// after the callback returns, so the next firing cannot begin until this one
/// has finished. That is the intended way to repeat.
///
/// Arming from *outside* the callback is a different matter. Calling
/// [`ThreadpoolTimer::set_after`] while a callback is running can queue the next
/// firing before the current one returns, and the two then run concurrently on
/// different pool threads. The callback is `Fn + Sync`, so this is permitted
/// rather than unsound -- but it means a callback that assumes it is the only
/// one running must not be driven that way. Re-arm from the callback, or use
/// [`ThreadpoolTimer::disarm`] and [`ThreadpoolTimer::wait`] before re-arming
/// externally.
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
            suppress_rearm: Mutex::new(0),
            #[cfg(test)]
            rearm_observer: Mutex::new(None),
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

    /// Record, into `observer`, whether each deferred re-arm is actually applied.
    ///
    /// The caller keeps its own clone, so the record survives this timer's
    /// teardown -- which is the only moment a re-arm is suppressed.
    #[cfg(test)]
    pub(crate) fn observe_rearms(&self, observer: &std::sync::Arc<Mutex<Vec<bool>>>) {
        // SAFETY: the context outlives self; Drop frees it after the drain.
        let ctx = unsafe { &*self.context };
        *ctx.rearm_observer
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = Some(std::sync::Arc::clone(observer));
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

    /// Let every queued callback run, and block until none is executing.
    ///
    /// This does **not** leave a self-re-arming timer idle. A callback's
    /// [`TimerFiring::rearm_after`] is applied after the callback returns, so a
    /// firing that runs during this call installs a fresh due time and the timer
    /// is armed again when it returns. Use
    /// [`stop_and_drain`](Self::stop_and_drain) to reach quiescence.
    pub fn wait(&self) {
        // SAFETY: timer is valid for the lifetime of self.
        unsafe { WaitForThreadpoolTimerCallbacks(self.timer, FALSE) };
    }

    /// Drop callbacks that have not started, then wait for any executing one.
    ///
    /// Like [`wait`](Self::wait), this does not by itself leave a self-re-arming
    /// timer idle: it does not suppress the deferred re-arm of a callback that
    /// is already running. Use [`stop_and_drain`](Self::stop_and_drain) when the
    /// timer must actually be quiescent afterwards.
    pub fn cancel_pending(&self) {
        // SAFETY: timer is valid for the lifetime of self. A cancelled timer
        // callback owns no storage, so dropping queued callbacks orphans nothing.
        unsafe { WaitForThreadpoolTimerCallbacks(self.timer, TRUE) };
    }

    /// Stop the timer and block until it is idle, leaving it reusable.
    ///
    /// This exists because neither [`disarm`](Self::disarm) nor
    /// [`cancel_pending`](Self::cancel_pending) can stop a self-re-arming timer
    /// on its own: a callback already running requests its re-arm through
    /// [`TimerFiring::rearm_after`], and the trampoline applies it *after* the
    /// callback returns -- which is after any disarm from outside. This
    /// suppresses that deferred re-arm for the duration of the call, using the
    /// same mechanism `Drop` uses, and lifts the suppression before returning so
    /// the timer can be armed again afterwards.
    ///
    /// # What this guarantees
    ///
    /// On return, provided no other thread arms the timer during the call:
    ///
    /// - no callback is queued or executing, and
    /// - the timer has no due time -- a re-arm requested by a callback that ran
    ///   during the call is discarded rather than deferred.
    ///
    /// # What it does not
    ///
    /// **A concurrent arm from another thread is not excluded.** `ThreadpoolTimer`
    /// is `Sync` and [`set_after`](Self::set_after), [`set_at`](Self::set_at) and
    /// [`set_after_with_window`](Self::set_after_with_window) all take `&self`,
    /// so they do not pass through the suppression this uses. Nothing in this
    /// crate orders such a call against this one.
    ///
    /// In practice the drain currently cancels a due time installed that way --
    /// `WaitForThreadpoolTimerCallbacks` with cancellation clears one even when
    /// no callback is queued, measurably so. That is not a documented contract
    /// and is not relied upon here: if a caller needs the timer to be provably
    /// idle, it must ensure nothing else arms it for the duration, by owning it
    /// exclusively or serializing access to it.
    ///
    /// Calling this from inside the timer's own callback would deadlock, because
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
        // Close the door on re-arming before disarming, and do both under the
        // same lock. Disarming alone is not enough: a callback still running can
        // have a deferred re-arm that the trampoline applies after it returns,
        // the drain below could then return with the timer armed, and the close
        // and context free would race a freshly queued callback.
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
