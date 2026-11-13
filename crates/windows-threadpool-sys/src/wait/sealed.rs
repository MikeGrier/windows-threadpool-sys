// Copyright (c) 2026 Mike Grier
//! Thread-pool waits that preserve a sealed event's handle-access boundary.

use std::io;
use std::time::Duration;

use win_sync_sys::SealedWaitOnlyEvent;

use super::{ThreadpoolWait, WaitActivation, WaitResult, WaitTarget, WaitableHandle};
use crate::callback_env::CallbackEnviron;

/// A sealed event target for [`SealedThreadpoolWait`], sharing its existing handle.
///
/// Conversion does not duplicate the handle or widen access. No safe accessor, conversion or
/// `Debug` output reveals it. The owning event and previously retained unsealed aliases remain
/// outside this boundary, as specified by [`SealedWaitOnlyEvent`].
///
/// ```compile_fail,E0599
/// use windows_threadpool_sys::wait::SealedWaitableHandle;
/// fn expose(target: &SealedWaitableHandle) { let _ = target.handle(); }
/// ```
///
/// ```compile_fail,E0599
/// use windows_threadpool_sys::wait::SealedWaitableHandle;
/// fn expose(target: SealedWaitableHandle) { let _ = target.into_handle(); }
/// ```
///
/// ```compile_fail,E0277
/// use windows_threadpool_sys::wait::{SealedWaitableHandle, WaitableHandle};
/// fn unseal(target: SealedWaitableHandle) { let _ = WaitableHandle::from(target); }
/// ```
///
/// ```compile_fail,E0599
/// use std::os::windows::io::AsHandle;
/// use windows_threadpool_sys::wait::SealedWaitableHandle;
/// fn expose(target: &SealedWaitableHandle) { let _ = target.as_handle(); }
/// ```
///
/// ```compile_fail,E0599
/// use std::os::windows::io::AsRawHandle;
/// use windows_threadpool_sys::wait::SealedWaitableHandle;
/// fn expose(target: &SealedWaitableHandle) { let _ = target.as_raw_handle(); }
/// ```
pub struct SealedWaitableHandle {
    target: WaitTarget,
}

impl From<SealedWaitOnlyEvent> for SealedWaitableHandle {
    fn from(event: SealedWaitOnlyEvent) -> Self {
        Self {
            target: WaitTarget::Shared(unsafe { event.into_shared_handle() }),
        }
    }
}

impl std::fmt::Debug for SealedWaitableHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SealedWaitableHandle")
            .finish_non_exhaustive()
    }
}

/// A sealed wait's callback activation, with its result and rearm operation but no handle access.
///
/// ```compile_fail,E0599
/// use windows_threadpool_sys::wait::SealedWaitActivation;
/// fn expose(activation: &SealedWaitActivation<'_>) { let _ = activation.handle(); }
/// ```
///
/// ```compile_fail,E0599
/// use std::os::windows::io::AsHandle;
/// use windows_threadpool_sys::wait::SealedWaitActivation;
/// fn expose(activation: &SealedWaitActivation<'_>) { let _ = activation.as_handle(); }
/// ```
///
/// ```compile_fail,E0599
/// use std::os::windows::io::AsRawHandle;
/// use windows_threadpool_sys::wait::SealedWaitActivation;
/// fn expose(activation: &SealedWaitActivation<'_>) { let _ = activation.as_raw_handle(); }
/// ```
pub struct SealedWaitActivation<'ctx> {
    inner: &'ctx WaitActivation<'ctx>,
}

impl std::fmt::Debug for SealedWaitActivation<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SealedWaitActivation")
            .field("result", &self.result())
            .finish_non_exhaustive()
    }
}

impl SealedWaitActivation<'_> {
    /// Why this callback ran.
    #[must_use]
    pub fn result(&self) -> WaitResult {
        self.inner.result()
    }

    /// Whether the event was signalled rather than the wait timing out.
    #[must_use]
    pub fn is_signalled(&self) -> bool {
        self.inner.is_signalled()
    }

    /// Rearm using [`WaitActivation::rearm`]'s ordering and teardown rules.
    ///
    /// A manual-reset event must be reset through its owning `Event` before rearming to wait
    /// for a new signal. Otherwise it activates again immediately and callbacks may overlap.
    /// An auto-reset signal is consumed by the wait. `None` waits indefinitely; rearming during
    /// teardown is suppressed. Rearm before the producer's next signal.
    pub fn rearm(&self, timeout: Option<Duration>) {
        self.inner.rearm(timeout);
    }
}

/// A thread-pool wait whose target and callback never expose the sealed event handle to safe code.
///
/// Registration uses the native thread pool, without a dedicated waiting thread. The lifecycle
/// is [`ThreadpoolWait`]'s: explicit [`stop_and_drain`](Self::stop_and_drain) settles the drain
/// obligation; dropping drains callbacks before releasing the target and retains the underlying
/// `fail-fast` behavior. Neither the wrapper nor its activation dereferences to the unsealed type.
/// With `trace` enabled, event-handle fields are zero; wait-object identifiers and lifecycle
/// records remain available. No redaction is applied to ordinary [`ThreadpoolWait`] targets.
///
/// ```compile_fail,E0599
/// use windows_threadpool_sys::wait::SealedThreadpoolWait;
/// fn expose(wait: &SealedThreadpoolWait) { let _ = wait.handle(); }
/// ```
///
/// ```compile_fail,E0599
/// use std::os::windows::io::AsHandle;
/// use windows_threadpool_sys::wait::SealedThreadpoolWait;
/// fn expose(wait: &SealedThreadpoolWait) { let _ = wait.as_handle(); }
/// ```
///
/// ```compile_fail,E0599
/// use std::os::windows::io::AsRawHandle;
/// use windows_threadpool_sys::wait::SealedThreadpoolWait;
/// fn expose(wait: &SealedThreadpoolWait) { let _ = wait.as_raw_handle(); }
/// ```
///
/// ```compile_fail,E0308
/// use windows_threadpool_sys::wait::{SealedThreadpoolWait, ThreadpoolWait};
/// fn unseal(wait: SealedThreadpoolWait) { let _ = ThreadpoolWait::from(wait); }
/// ```
pub struct SealedThreadpoolWait {
    inner: ThreadpoolWait,
}

impl std::fmt::Debug for SealedThreadpoolWait {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SealedThreadpoolWait")
            .finish_non_exhaustive()
    }
}

impl SealedThreadpoolWait {
    /// Create an idle wait with [`ThreadpoolWait::new`]'s environment and callback requirements.
    ///
    /// The callback runs on a pool thread, must restore any thread state it changes, and must
    /// neither terminate that thread nor panic. The target stays alive through callback drainage.
    ///
    /// # Errors
    ///
    /// Returns `CreateThreadpoolWait`'s error. Allocation failure cannot be forced by these tests
    /// without exhausting resources needed by the test process.
    pub fn new<F>(
        handle: SealedWaitableHandle,
        callback: F,
        env: Option<&mut CallbackEnviron<'_>>,
    ) -> io::Result<Self>
    where
        F: Fn(&SealedWaitActivation<'_>) + Send + Sync + 'static,
    {
        ThreadpoolWait::new_with_trace(
            WaitableHandle {
                target: handle.target,
            },
            move |activation| callback(&SealedWaitActivation { inner: activation }),
            env,
            false,
        )
        .map(|inner| Self { inner })
    }

    /// Arm before signalling, following [`ThreadpoolWait::arm`]; `None` waits indefinitely.
    pub fn arm(&self, timeout: Option<Duration>) {
        self.inner.arm(timeout);
    }

    /// Stop watching without discarding callbacks already queued, as [`ThreadpoolWait::disarm`].
    pub fn disarm(&self) {
        self.inner.disarm();
    }

    /// Drain queued and executing callbacks, as [`ThreadpoolWait::wait`].
    ///
    /// A callback may rearm during this call; use [`stop_and_drain`](Self::stop_and_drain)
    /// for quiescence. Do not call from this wait's own callback, which would wait for itself.
    pub fn wait(&self) {
        self.inner.wait();
    }

    /// Cancel pending callbacks with [`ThreadpoolWait::try_cancel_pending`]'s pool repair.
    ///
    /// Requires `self-heal`. This does not suppress rearming by an executing callback.
    #[cfg(feature = "self-heal")]
    pub fn try_cancel_pending(&self) {
        self.inner.try_cancel_pending();
    }

    /// Cancel pending callbacks without pool-repair tracking; available in every feature build.
    ///
    /// # Safety
    ///
    /// Uphold [`ThreadpoolWait::try_cancel_pending_no_heal_tracking`]'s pool-repair obligation:
    /// ensure the pool is repaired by submitting work or otherwise keeping it live.
    ///
    /// ```compile_fail,E0133
    /// use windows_threadpool_sys::wait::SealedThreadpoolWait;
    /// fn cancel(wait: &SealedThreadpoolWait) { wait.try_cancel_pending_no_heal_tracking(); }
    /// ```
    pub unsafe fn try_cancel_pending_no_heal_tracking(&self) {
        unsafe { self.inner.try_cancel_pending_no_heal_tracking() };
    }

    /// Suppress callback rearming, disarm and drain, as [`ThreadpoolWait::stop_and_drain`].
    ///
    /// Queued callbacks run. Serialize external `arm` calls if the wait must stay idle.
    /// Calling this from the wait's own callback deadlocks.
    pub fn stop_and_drain(&self) {
        self.inner.stop_and_drain();
    }
}
