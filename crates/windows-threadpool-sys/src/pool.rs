// Copyright (c) 2026 Mike Grier
//! Owned private thread pools: `CreateThreadpool` / `CloseThreadpool`.
//!
//! Callbacks run on the process-default pool unless a [`CallbackEnviron`] names
//! a private one. A private pool lets an application bound the threads a
//! subsystem may consume, or isolate long-running callbacks from unrelated work,
//! without affecting the rest of the process.
//!
//! [`CallbackEnviron`]: crate::callback_env::CallbackEnviron

use std::io;
use std::ptr;
use std::sync::Mutex;

use windows_sys::Win32::System::Threading::{
    CloseThreadpool, CreateThreadpool, PTP_POOL, SetThreadpoolThreadMaximum,
    SetThreadpoolThreadMinimum,
};

/// The thread limits this wrapper has been told about.
///
/// Each field is `None` until the corresponding setter succeeds, because Win32
/// offers no way to read a pool's current limits back. A limit we were never
/// given cannot be used to reject its counterpart.
#[derive(Debug, Default)]
struct Limits {
    minimum: Option<u32>,
    maximum: Option<u32>,
}

/// An owned private thread pool.
///
/// Pass it to [`CallbackEnviron::set_pool`] to run an object's callbacks on this
/// pool instead of the process-default one. The environment borrows the pool, so
/// the pool cannot be closed while an environment still names it.
///
/// # Ordering and teardown
///
/// Creating a callback object from an environment that names this pool **binds**
/// the object to the pool. `CloseThreadpool` -- which this type's `Drop` calls --
/// frees the pool immediately only when no object is bound; otherwise it defers
/// the release until every bound object has been freed. A live object can
/// therefore never observe a freed pool, whatever order the pool and its objects
/// are dropped in, so this is not a memory-safety obligation on the caller. (The
/// `CallbackEnviron` borrow of the pool covers the one case binding does not: a
/// freshly created pool with no bound object yet is freed at once, so an
/// environment must not outlive it.)
///
/// What the order *does* control is when teardown blocks. Each object's `Drop`
/// waits for its in-flight callbacks; the pool's deferred release then completes
/// once the last object is gone. Declare the pool before the objects that use
/// it, so it is dropped last and that blocking happens where you expect.
///
/// # Examples
///
/// ```
/// use windows_threadpool_sys::callback_env::CallbackEnviron;
/// use windows_threadpool_sys::pool::ThreadpoolPool;
/// use windows_threadpool_sys::timer::ThreadpoolTimer;
/// use std::time::Duration;
///
/// // Declared first, so it outlives the objects that use it.
/// let pool = ThreadpoolPool::new()?;
/// pool.set_min_threads(1)?;
/// pool.set_max_threads(4)?;
///
/// let mut env = CallbackEnviron::new();
/// env.set_pool(&pool);
///
/// let timer = ThreadpoolTimer::new(|_firing| {}, Some(&mut env))?;
/// timer.set_after(Duration::from_millis(1));
/// // Discharges the drain this timer owes, at a point you choose rather than
/// // leaving the blocking teardown to `Drop`.
/// timer.stop_and_drain();
/// # Ok::<(), std::io::Error>(())
/// ```
///
/// [`CallbackEnviron::set_pool`]: crate::callback_env::CallbackEnviron::set_pool
#[derive(Debug)]
pub struct ThreadpoolPool {
    pool: PTP_POOL,
    limits: Mutex<Limits>,
}

// SAFETY: PTP_POOL is a kernel-managed object usable from any thread; this type
// only owns the handle and hands it to the pool APIs, which are thread-safe.
unsafe impl Send for ThreadpoolPool {}
unsafe impl Sync for ThreadpoolPool {}

impl ThreadpoolPool {
    /// Create a new private thread pool.
    ///
    /// # Errors
    ///
    /// Returns the error from `CreateThreadpool`, which fails when the process
    /// cannot allocate the pool.
    pub fn new() -> io::Result<Self> {
        // SAFETY: the reserved parameter must be null; no other input is read.
        let pool = crate::trace_call!("CreateThreadpool", 0, 0, {
            // SAFETY: a null reserved argument is the documented call.
            unsafe { CreateThreadpool(ptr::null()) }
        });
        if pool == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            pool,
            limits: Mutex::new(Limits::default()),
        })
    }

    /// Set the maximum number of threads this pool may allocate.
    ///
    /// # Conflicting limits
    ///
    /// Win32 lets the two limits contradict each other and resolves the conflict
    /// by *last call wins*, silently and unreportably. A pool given a maximum of
    /// 2 and then a minimum of 4 was measured running **4** callbacks
    /// concurrently, and it did not settle back to 2. This wrapper therefore
    /// tracks the limits it has set and rejects a pair that cannot both hold,
    /// rather than letting one quietly annul the other.
    ///
    /// # The maximum is a steady-state target, not an instantaneous ceiling
    ///
    /// Even where the maximum is the effective limit, it bounds the pool once it
    /// has settled, not every instant. Raising the minimum creates threads
    /// eagerly, and those surplus threads are not retired the moment a lower
    /// maximum is applied: with a minimum of 4 then a maximum of 2, a third
    /// callback was observed running concurrently in roughly 1 trial in 240 when
    /// many pools were being created at once. Do not rely on the maximum as a
    /// mutual-exclusion mechanism; use it to bound resource consumption.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::InvalidInput`] if `maximum` is zero. Such a pool
    /// runs no callbacks at all -- work submitted to it is queued and never
    /// executed -- and `SetThreadpoolThreadMaximum` returns void, so nothing
    /// else could report the mistake. Use
    /// [`CleanupGroup`](crate::cleanup_group::CleanupGroup) or the objects' own
    /// teardown to stop callbacks, rather than starving the pool that runs them.
    ///
    /// Also returns [`io::ErrorKind::InvalidInput`] if `maximum` is below a
    /// minimum previously set through [`set_min_threads`](Self::set_min_threads).
    ///
    /// # Examples
    ///
    /// A maximum below an established minimum is refused instead of silently
    /// overriding it:
    ///
    /// ```
    /// use windows_threadpool_sys::pool::ThreadpoolPool;
    ///
    /// let pool = ThreadpoolPool::new()?;
    /// pool.set_min_threads(4)?;
    /// assert!(pool.set_max_threads(2).is_err());
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn set_max_threads(&self, maximum: u32) -> io::Result<()> {
        if maximum == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a thread pool needs a maximum of at least one thread; a maximum of zero runs no \
                 callbacks at all and the native call cannot report it",
            ));
        }
        let mut limits = self.limits.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(minimum) = limits.minimum
            && maximum < minimum
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "a maximum of {maximum} is below this pool's minimum of {minimum}; Win32 \
                     would let the minimum win silently, so the conflict is refused instead"
                ),
            ));
        }
        // SAFETY: pool is valid for the lifetime of self.
        crate::trace_call!("SetThreadpoolThreadMaximum", self.pool, maximum, {
            // SAFETY: pool is valid for the lifetime of self.
            unsafe { SetThreadpoolThreadMaximum(self.pool, maximum) };
        });
        limits.maximum = Some(maximum);
        Ok(())
    }

    /// Set the minimum number of threads this pool keeps available.
    ///
    /// Raising the minimum makes the pool create threads eagerly, which is what
    /// guarantees forward progress for callbacks that block on one another.
    ///
    /// # Errors
    ///
    /// Returns the error from `SetThreadpoolThreadMinimum`, which fails when the
    /// pool cannot create the requested threads.
    ///
    /// Returns [`io::ErrorKind::InvalidInput`] if `minimum` exceeds a maximum
    /// previously set through [`set_max_threads`](Self::set_max_threads). Win32
    /// would accept it and run up to `minimum` callbacks concurrently, annulling
    /// the maximum without reporting anything; see
    /// [`set_max_threads`](Self::set_max_threads) for the measurements.
    ///
    /// # Examples
    ///
    /// ```
    /// use windows_threadpool_sys::pool::ThreadpoolPool;
    ///
    /// let pool = ThreadpoolPool::new()?;
    /// pool.set_max_threads(2)?;
    /// assert!(pool.set_min_threads(4).is_err());
    /// pool.set_min_threads(2)?;
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn set_min_threads(&self, minimum: u32) -> io::Result<()> {
        let mut limits = self.limits.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(maximum) = limits.maximum
            && minimum > maximum
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "a minimum of {minimum} exceeds this pool's maximum of {maximum}; Win32 \
                     would honour the minimum and annul the maximum silently, so the conflict \
                     is refused instead"
                ),
            ));
        }
        // SAFETY: pool is valid for the lifetime of self.
        // This one really does block: it creates threads, and is documented to
        // fail when it cannot.
        let ok = crate::trace_call!("SetThreadpoolThreadMinimum", self.pool, minimum, {
            // SAFETY: pool is valid for the lifetime of self.
            unsafe { SetThreadpoolThreadMinimum(self.pool, minimum) }
        });
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        limits.minimum = Some(minimum);
        Ok(())
    }

    /// The raw pool value, for storing in a callback environment.
    pub(crate) fn as_raw(&self) -> PTP_POOL {
        self.pool
    }
}

impl Drop for ThreadpoolPool {
    fn drop(&mut self) {
        // SAFETY: pool is valid and owned; the OS releases it once its last
        // member object is released.
        crate::trace_call!("CloseThreadpool", self.pool, 0, {
            // SAFETY: pool is valid and closed exactly once, here.
            unsafe { CloseThreadpool(self.pool) };
        });
    }
}

/// Make the default process pool create its first worker, and block until one
/// exists.
///
/// # Why this exists
///
/// A measured fault in the Windows thread pool needs a pool holding **zero**
/// threads. A teardown that removes an already-delivered completion packet a few
/// microseconds after it was queued can sever that port's notification to its
/// worker factory, after which the pool dispatches nothing -- no wait, no timer,
/// no I/O completion -- until something submits a work item. The factory reads as
/// perfectly idle throughout: not paused, not shut down, permitted to create,
/// nothing failed.
///
/// Warming the pool first was measured to prevent it: **0 occurrences in 24000
/// runs warm, against 66 in 24000 cold**, with a delay-matched cold control still
/// failing at the cold rate, so it is the worker and not the elapsed time.
///
/// # What it does not buy
///
/// **This is not a fix and does not make a bad teardown safe.** It removes one
/// of the fault's preconditions for as long as the pool stays warm, and the pool
/// stops being warm once it has been idle for its timeout -- 67 seconds on the
/// machine this was measured on. A long-lived process that goes quiet becomes
/// cold again, and this function would have to be called again to matter.
///
/// It does nothing about teardowns in this crate, which already drain and so
/// never remove a delivered packet. Its value is against code you do not
/// control: another library, or a dependency, tearing a wait down badly during
/// your process's startup.
///
/// # It is opt-in, and this crate never calls it for you
///
/// Nothing in this crate warms the pool on your behalf -- not `ThreadpoolWait::new`,
/// not any other constructor. Three reasons, in order of weight:
///
/// 1. **It is a process-wide side effect.** A thread created here belongs to the
///    default pool, which is shared with every other component in the process. A
///    library that silently adds a resident thread to a pool it does not own has
///    made a decision that was not its to make.
/// 2. **This crate's own teardowns do not need it.** They drain, so they never
///    remove a delivered packet. Calling this automatically would protect against
///    *other* code while implying that this crate's paths required it.
/// 3. **It would be a surprising cost in the common case.** Most callers create a
///    wait and never tear it down badly; charging all of them a thread for a
///    hazard they do not have is the wrong default.
///
/// Whether the protection is worth one resident thread is a judgement about the
/// process as a whole, which the caller is in a position to make and this crate
/// is not.
///
/// # What was ruled out
///
/// Holding the pool warm *permanently* would be better, and is not available:
/// `SetThreadpoolThreadMinimum` does not accept a null pool, and calling it that
/// way does not fail -- it **raises `STATUS_INVALID_PARAMETER` and terminates the
/// process**. The default pool's minimum cannot be set. A private
/// [`ThreadpoolPool`] can have [`set_min_threads`](ThreadpoolPool::set_min_threads)
/// applied to it, but that is a different pool.
///
/// # It always submits, and does not check first
///
/// The obvious optimisation is to read the worker count and skip the work item
/// when the pool is already warm. It is not done, for two reasons, and the first
/// is simply that it is slower. Measured on the development machine: prewarming
/// an already-warm pool costs about 27us, while the query needed to decide to
/// skip it costs about 204us, because there is no way to ask "how many workers
/// does the default pool have" without scanning the handle space. The skip would
/// cost roughly seven times what it saves.
///
/// The second reason outlasts the first, and has since become the whole of it.
/// That query reads a layout Microsoft does not publish, through an undocumented
/// entry point. A misread returning a plausible non-zero would make this function
/// skip the warm-up and report success, leaving the caller believing they are
/// protected when they are not -- the exact failure this exists to prevent. An
/// unconditional submit cannot be wrong that way.
///
/// **This crate no longer contains any way to make that query.** The facility
/// that did was removed rather than kept behind a feature; see the decision in
/// the workspace `DESIGN-NOTES.md`. A caller who wants worker counts needs a
/// mechanism built on documented ground, such as an ETW kernel trace.
///
/// # Returns
///
/// Whether a worker is confirmed to exist. A callback having run is the proof,
/// because it ran on one. `false` means the work item could not be created or did
/// not run within the bound, and the pool should be assumed cold.
pub fn prewarm_default_pool() -> bool {
    use std::sync::mpsc;
    use std::time::Duration;

    /// How long to wait for the warm-up callback. Generous: a healthy pool
    /// creates its first worker in well under a millisecond, so reaching this
    /// means something is already wrong.
    const BOUND: Duration = Duration::from_secs(2);

    let (tx, rx) = mpsc::channel();
    let tx = std::sync::Mutex::new(tx);
    let Ok(work) = crate::work::ThreadpoolWork::new(
        move || {
            if let Ok(tx) = tx.lock() {
                let _ = tx.send(());
            }
        },
        None,
    ) else {
        return false;
    };
    work.submit();
    let confirmed = rx.recv_timeout(BOUND).is_ok();
    if !confirmed {
        // Cancel before draining, on this path only.
        //
        // Reaching the timeout means the callback was never dispatched, which
        // is the condition this function exists to report. Draining then waits
        // for that same undispatched callback **with no deadline at all**, so
        // the bounded check above would be followed by an unbounded one and the
        // `false` this is supposed to return would never arrive. The bound
        // would be decoration.
        //
        // Cancelling discards the queued invocation and waits only for one
        // already executing, and only on this object: the warm-up callback does
        // nothing but send on a channel, so it cannot block, and another
        // object's stuck callback is not this call's to wait for.
        // This also discharges the obligation: `cancel_pending` returns with
        // nothing queued and nothing executing, and settles on that basis. The
        // drain below would therefore be a second native synchronisation that
        // waits for a state already reached.
        work.cancel_pending();
    } else {
        // Discharged here rather than left to the drop below. The drop would
        // drain anyway, so this adds no blocking -- but this function is the
        // crate's own use of its own protocol, and leaving the obligation
        // undischarged makes it a reported violation like any other.
        //
        // Needed only on this path, and that asymmetry is deliberate rather
        // than an oversight: a dispatch does not settle a work object's
        // obligation, because a work object can be submitted again and the
        // drain stays the caller's regardless of what has already run. So the
        // confirmed path has a live obligation even though its callback
        // demonstrably ran, while the timeout path above has already cleared
        // one.
        work.stop_and_drain();
    }
    confirmed
}

#[cfg(test)]
mod tests;
