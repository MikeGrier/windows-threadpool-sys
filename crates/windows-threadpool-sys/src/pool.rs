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
/// # Ordering requirement
///
/// The borrow from [`CallbackEnviron::set_pool`] keeps the pool alive while the
/// *environment* exists, but an environment's contents are **copied** into each
/// callback object at creation time; the object does not keep a reference the
/// compiler can follow. So the pool must also outlive every object created from
/// an environment that named it.
///
/// `Drop` calls `CloseThreadpool`, which the OS defers until the pool's last
/// member is released, so dropping the pool early is not itself a use-after-free
/// -- but the objects must still be dropped before the process relies on the
/// pool being gone. Declare the pool before the objects that use it, so it is
/// dropped last.
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
/// timer.wait();
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
        let pool = unsafe { CreateThreadpool(ptr::null()) };
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
        unsafe { SetThreadpoolThreadMaximum(self.pool, maximum) };
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
        let ok = unsafe { SetThreadpoolThreadMinimum(self.pool, minimum) };
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
        unsafe { CloseThreadpool(self.pool) };
    }
}

#[cfg(test)]
mod tests;
