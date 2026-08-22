// Copyright (c) 2026 Mike Grier
//! Periodic thread-pool timers.

use std::io;
use std::ptr;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::time::{Duration, SystemTime};

use windows_sys::Win32::Foundation::{FALSE, TRUE};
use windows_sys::Win32::System::Threading::{
    CloseThreadpoolTimer, CreateThreadpoolTimer, IsThreadpoolTimerSet, PTP_CALLBACK_INSTANCE,
    PTP_TIMER, WaitForThreadpoolTimerCallbacks,
};

use crate::callback_env::CallbackEnviron;
use crate::timer::{absolute_filetime, arm_raw, disarm_raw, millis_u32, relative_filetime};

/// Heap-allocated callback state kept alive for the lifetime of the timer.
///
/// `timer` is filled in after `CreateThreadpoolTimer` returns, because stopping
/// from inside a callback needs the object the callback belongs to.
struct PeriodicContext {
    timer: AtomicIsize,
    callback: Box<dyn Fn(&PeriodicTick<'_>) + Send + Sync + 'static>,
}

/// One tick of a [`ThreadpoolPeriodicTimer`], handed to its callback.
///
/// A tick may be running concurrently with other ticks of the same timer, so
/// anything this callback touches must tolerate that.
pub struct PeriodicTick<'ctx> {
    ctx: &'ctx PeriodicContext,
}

impl std::fmt::Debug for PeriodicTick<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PeriodicTick").finish_non_exhaustive()
    }
}

impl PeriodicTick<'_> {
    /// Stop the timer from inside its own callback.
    ///
    /// This is how a periodic timer ends itself -- "tick until some condition
    /// holds" needs no external coordination.
    ///
    /// It stops *future* ticks being queued. It does not retract ticks already
    /// queued, and it does not affect ticks already running, including other
    /// concurrent runs of this same callback. Expect the callback to run again
    /// after calling this, and make it idempotent accordingly.
    pub fn stop(&self) {
        let timer = self.ctx.timer.load(Ordering::Acquire);
        debug_assert_ne!(timer, 0, "the timer must be published before callbacks");
        // SAFETY: `timer` is this object's live PTP_TIMER, published before any
        // callback could run.
        unsafe { disarm_raw(timer) };
    }
}

/// Trampoline from the raw `PTP_TIMER_CALLBACK` ABI into the boxed closure.
///
/// SAFETY: `context` must point to a live [`PeriodicContext`] for the entire
/// duration of every callback invocation, which [`ThreadpoolPeriodicTimer`]'s `Drop`
/// ordering guarantees.
unsafe extern "system" fn periodic_trampoline(
    _instance: PTP_CALLBACK_INSTANCE,
    context: *mut core::ffi::c_void,
    _timer: PTP_TIMER,
) {
    // SAFETY: context is a valid *mut PeriodicContext for the full callback duration.
    let ctx = unsafe { &*(context as *const PeriodicContext) };
    let tick = PeriodicTick { ctx };
    // Not contained: see the callback contract in the crate docs.
    (ctx.callback)(&tick);
}

/// An owned repeating thread-pool timer.
///
/// The period is fixed when the timer is created, so the type says what it is:
/// this object exists to tick on a cadence, and there is no argument that can
/// quietly turn it into a one-shot.
///
/// # Ticks can overlap
///
/// **The pool queues each tick on schedule regardless of whether the previous
/// tick has finished.** If the callback takes longer than the period, two or
/// more runs of it will execute concurrently on different pool threads. This is
/// the property that makes periodic timers surprising in practice, and it
/// follows from the cadence being fixed: the schedule cannot wait for the
/// callback without ceasing to be a schedule.
///
/// So a `ThreadpoolPeriodicTimer` callback must be safe to run concurrently with itself.
/// If that is awkward, the alternative is a one-shot [`ThreadpoolTimer`](crate::timer::ThreadpoolTimer) re-armed from
/// inside its own callback with [`crate::timer::TimerFiring::rearm_after`]:
/// there is never more than one firing outstanding, and the gap is measured from
/// the end of each firing rather than from a fixed schedule.
///
/// |  | [`ThreadpoolPeriodicTimer`] | [`ThreadpoolTimer`](crate::timer::ThreadpoolTimer) + `rearm_after` |
/// |---|---|---|
/// | Cadence | fixed, independent of callback duration | drifts by the callback duration |
/// | Concurrent runs of the callback | possible | never |
/// | Slow callback | ticks pile up and overlap | next tick simply happens later |
///
/// # Teardown
///
/// [`Drop`] stops the timer before draining callbacks, so it cannot requeue
/// during teardown. [`ThreadpoolPeriodicTimer::stop_and_drain`] does the same thing under
/// the caller's control, and is the ordering to copy if doing it by hand:
/// stop first, drain second.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use std::sync::atomic::{AtomicUsize, Ordering};
/// use std::time::Duration;
/// use windows_threadpool_sys::timer::ThreadpoolPeriodicTimer;
///
/// let ticks = Arc::new(AtomicUsize::new(0));
/// let counter = Arc::clone(&ticks);
///
/// // The period belongs to the timer, not to a call.
/// let timer = ThreadpoolPeriodicTimer::new(Duration::from_millis(1), move |_tick| {
///     counter.fetch_add(1, Ordering::SeqCst);
/// }, None)?;
///
/// timer.start();
/// while ticks.load(Ordering::SeqCst) < 3 {
///     std::thread::yield_now();
/// }
///
/// timer.stop_and_drain();
/// assert!(ticks.load(Ordering::SeqCst) >= 3);
/// # Ok::<(), std::io::Error>(())
/// ```
///
/// Stopping from inside the callback, for "tick until done". Note the counter
/// may pass the threshold, because a tick already queued still runs:
///
/// ```
/// use std::sync::Arc;
/// use std::sync::atomic::{AtomicUsize, Ordering};
/// use std::time::Duration;
/// use windows_threadpool_sys::timer::ThreadpoolPeriodicTimer;
///
/// let ticks = Arc::new(AtomicUsize::new(0));
/// let counter = Arc::clone(&ticks);
///
/// let timer = ThreadpoolPeriodicTimer::new(Duration::from_millis(1), move |tick| {
///     if counter.fetch_add(1, Ordering::SeqCst) >= 2 {
///         tick.stop();
///     }
/// }, None)?;
///
/// timer.start();
/// while timer.is_running() {
///     std::thread::yield_now();
/// }
/// timer.stop_and_drain();
/// assert!(ticks.load(Ordering::SeqCst) >= 3);
/// # Ok::<(), std::io::Error>(())
/// ```
pub struct ThreadpoolPeriodicTimer {
    timer: PTP_TIMER,
    period: Duration,
    // Kept alive as a raw pointer until Drop has stopped and drained.
    context: *mut PeriodicContext,
}

// SAFETY: PTP_TIMER is a cross-thread pool object, and the context holds a
// callback that is Fn + Send + Sync; the pointer is only read until Drop frees
// it after all callbacks have finished.
unsafe impl Send for ThreadpoolPeriodicTimer {}
unsafe impl Sync for ThreadpoolPeriodicTimer {}

/// Nanoseconds in a millisecond, for checking a period divides evenly.
const NANOS_PER_MILLI: u32 = 1_000_000;

impl ThreadpoolPeriodicTimer {
    /// The shortest period this timer can express: one millisecond.
    ///
    /// `SetThreadpoolTimer` takes the period as whole milliseconds, so nothing
    /// shorter can be represented. This is a floor on what may be *asked for*,
    /// not a promise about delivery: ticks arrive on the system timer tick,
    /// which is far coarser (~15.6ms by default).
    pub const MIN_PERIOD: Duration = Duration::from_millis(1);

    /// The longest period this timer can express: `u32::MAX` milliseconds, or
    /// just under 50 days.
    ///
    /// The pool's period field is a `u32` count of milliseconds, so a longer
    /// period cannot be represented.
    pub const MAX_PERIOD: Duration = Duration::from_millis(u32::MAX as u64);

    /// Create a stopped timer that invokes `callback` every `period`.
    ///
    /// `period` must be a whole number of milliseconds between
    /// [`MIN_PERIOD`](Self::MIN_PERIOD) and [`MAX_PERIOD`](Self::MAX_PERIOD);
    /// anything else is rejected. The pool takes the period as a `u32` count of
    /// milliseconds, so every other value would be quietly altered to fit:
    ///
    /// - a period below a millisecond rounds down to zero, and a zero period
    ///   means "do not repeat" -- the timer would fire once and stop, which is a
    ///   `ThreadpoolPeriodicTimer` silently behaving like a
    ///   [`ThreadpoolTimer`](crate::timer::ThreadpoolTimer);
    /// - a fractional period such as 1.5ms is truncated, so the timer would tick
    ///   at 1ms while [`period`](Self::period) still reported 1.5ms;
    /// - a period beyond `u32::MAX` milliseconds would be capped, ticking far
    ///   more often than asked.
    ///
    /// Rejecting rather than rounding keeps [`period`](Self::period) an accurate
    /// report of what was scheduled.
    ///
    /// Pass `Some(env)` to select a private pool or callback priority; `None`
    /// uses the process-default pool with default priority.
    ///
    /// The callback runs on a shared, process-managed pool thread, may run
    /// concurrently with itself (see the type documentation), must restore any
    /// thread state it changes, and must not terminate its thread. It must not
    /// panic: a panic unwinds to the `extern "system"` trampoline and aborts the
    /// process.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::InvalidInput`] if `period` is outside
    /// [`MIN_PERIOD`](Self::MIN_PERIOD)..=[`MAX_PERIOD`](Self::MAX_PERIOD) or is
    /// not a whole number of milliseconds, or the error from
    /// `CreateThreadpoolTimer`.
    pub fn new<F>(
        period: Duration,
        callback: F,
        env: Option<&mut CallbackEnviron>,
    ) -> io::Result<Self>
    where
        F: Fn(&PeriodicTick<'_>) + Send + Sync + 'static,
    {
        if period < Self::MIN_PERIOD {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a ThreadpoolPeriodicTimer needs a period of at least 1ms: the pool takes the period in whole milliseconds, so anything shorter rounds to zero and a zero period means do not repeat; use ThreadpoolTimer for a one-shot",
            ));
        }
        if period > Self::MAX_PERIOD {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a ThreadpoolPeriodicTimer period must fit in u32 milliseconds (just under 50 days); a longer one would be capped and tick far more often than asked",
            ));
        }
        if !period.subsec_nanos().is_multiple_of(NANOS_PER_MILLI) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a ThreadpoolPeriodicTimer period must be a whole number of milliseconds: the pool truncates the remainder, so the timer would tick sooner than the period it reports",
            ));
        }

        let context = Box::into_raw(Box::new(PeriodicContext {
            timer: AtomicIsize::new(0),
            callback: Box::new(callback),
        }));
        let env_ptr = env.map_or(ptr::null_mut(), |e| e.as_mut_ptr());

        // SAFETY: context is a valid heap pointer that outlives every callback,
        // and env_ptr is valid (or null) for the duration of this call.
        let timer = unsafe {
            CreateThreadpoolTimer(
                Some(periodic_trampoline),
                context.cast(),
                env_ptr.cast_const(),
            )
        };

        if timer == 0 {
            let error = io::Error::last_os_error();
            // SAFETY: the pool never saw context; reclaim it immediately.
            unsafe { drop(Box::from_raw(context)) };
            return Err(error);
        }

        // Publish the object before any callback can run. The timer is not
        // started yet, so no callback can observe the unpublished value.
        // SAFETY: context is live and exclusively ours until the first start.
        unsafe { (*context).timer.store(timer, Ordering::Release) };

        Ok(Self {
            timer,
            period,
            context,
        })
    }

    /// The period this timer ticks on.
    #[must_use]
    pub fn period(&self) -> Duration {
        self.period
    }

    /// Start ticking, with the first tick one period from now.
    pub fn start(&self) {
        self.start_after(self.period);
    }

    /// Start ticking, with the first tick `first_delay` from now.
    ///
    /// Subsequent ticks follow every [`ThreadpoolPeriodicTimer::period`]. A zero
    /// `first_delay` makes the first tick due immediately.
    pub fn start_after(&self, first_delay: Duration) {
        // SAFETY: timer is valid for the lifetime of self.
        unsafe {
            arm_raw(
                self.timer,
                relative_filetime(first_delay),
                millis_u32(self.period),
                0,
            );
        }
    }

    /// Start ticking, with the first tick at the wall-clock instant `when`.
    ///
    /// Unlike a relative first delay, an absolute one passes through sleep and
    /// hibernation.
    pub fn start_at(&self, when: SystemTime) {
        // SAFETY: timer is valid for the lifetime of self.
        unsafe {
            arm_raw(
                self.timer,
                absolute_filetime(when),
                millis_u32(self.period),
                0,
            );
        }
    }

    /// Start ticking, allowing the system a coalescing `window` on each tick.
    ///
    /// `window` is the tolerance the system may add so it can group this timer
    /// with other expirations and wake the processor less often, trading timing
    /// precision for power.
    pub fn start_with_window(&self, first_delay: Duration, window: Duration) {
        // SAFETY: timer is valid for the lifetime of self.
        unsafe {
            arm_raw(
                self.timer,
                relative_filetime(first_delay),
                millis_u32(self.period),
                millis_u32(window),
            );
        }
    }

    /// Stop the timer.
    ///
    /// Future ticks stop being queued, but a tick already queued still runs and
    /// ticks already executing are unaffected. Use
    /// [`ThreadpoolPeriodicTimer::stop_and_drain`] to also wait for those.
    pub fn stop(&self) {
        // SAFETY: timer is valid for the lifetime of self.
        unsafe { disarm_raw(self.timer) };
    }

    /// Whether the timer is currently started.
    ///
    /// Ticking does not clear the schedule, so this stays `true` until something
    /// stops the timer -- [`ThreadpoolPeriodicTimer::stop`], [`PeriodicTick::stop`], or
    /// teardown.
    #[must_use]
    pub fn is_running(&self) -> bool {
        // SAFETY: timer is valid for the lifetime of self.
        unsafe { IsThreadpoolTimerSet(self.timer) != 0 }
    }

    /// Block until all queued and executing ticks have completed.
    ///
    /// Stop the timer first, or this waits for a schedule that keeps producing
    /// new ticks. [`ThreadpoolPeriodicTimer::stop_and_drain`] does both in the right order.
    pub fn wait(&self) {
        // SAFETY: timer is valid for the lifetime of self.
        unsafe { WaitForThreadpoolTimerCallbacks(self.timer, FALSE) };
    }

    /// Stop the timer and wait until no tick is queued or executing.
    ///
    /// This is the correct teardown order -- stop first, drain second -- and is
    /// what [`Drop`] performs. Ticks that have not started are dropped rather
    /// than run.
    ///
    /// The result holds provided no other thread starts the timer during the
    /// call. `ThreadpoolPeriodicTimer` is `Sync` and the `start*` methods take
    /// `&self`, so a start landing between the stop and the drain is not
    /// excluded by anything here, and would leave a schedule installed on
    /// return. A caller needing the timer to be provably stopped must own it
    /// exclusively or serialize access to it. Unlike the one-shot timer there is
    /// no re-arm to suppress: [`PeriodicTick::stop`] only ever stops.
    pub fn stop_and_drain(&self) {
        self.stop();
        // SAFETY: timer is valid for the lifetime of self. A cancelled timer
        // callback owns no storage, so dropping queued ticks orphans nothing.
        unsafe { WaitForThreadpoolTimerCallbacks(self.timer, TRUE) };
    }

    /// Give up ownership, returning the raw object, its callback context, and
    /// the period.
    ///
    /// Used only by [`crate::cleanup_group::CleanupGroup`], which takes over the
    /// first two: a group member is released by
    /// `CloseThreadpoolCleanupGroupMembers` and must not close itself, so this
    /// suppresses this type's `Drop`.
    pub(crate) fn into_parts(self) -> (PTP_TIMER, *mut core::ffi::c_void, Duration) {
        let this = std::mem::ManuallyDrop::new(self);
        (this.timer, this.context.cast(), this.period)
    }

    /// Free a context returned by [`ThreadpoolPeriodicTimer::into_parts`].
    ///
    /// # Safety
    ///
    /// `context` must come from `into_parts` on this type, its object must
    /// already have been released, and it must be freed exactly once.
    pub(crate) unsafe fn drop_context(context: *mut core::ffi::c_void) {
        // SAFETY: forwarded from this function's own contract.
        drop(unsafe { Box::from_raw(context.cast::<PeriodicContext>()) });
    }
}

impl Drop for ThreadpoolPeriodicTimer {
    fn drop(&mut self) {
        // Stop before draining, or the timer would queue a fresh tick while the
        // drain is in progress and never settle.
        self.stop_and_drain();

        // SAFETY: no tick can be queued or executing, so the object can be
        // closed and the context freed exactly once.
        unsafe {
            CloseThreadpoolTimer(self.timer);
            drop(Box::from_raw(self.context));
        }
    }
}

impl std::fmt::Debug for ThreadpoolPeriodicTimer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ThreadpoolPeriodicTimer")
            .field("period", &self.period)
            .field("is_running", &self.is_running())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
