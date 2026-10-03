// Copyright (c) 2026 Mike Grier
//! Thread-pool work objects: `CreateThreadpoolWork` / `SubmitThreadpoolWork` /
//! `WaitForThreadpoolWorkCallbacks` / `CloseThreadpoolWork`.

use core::ffi::c_void;
use std::io;
use std::mem::ManuallyDrop;
use std::ptr;

use windows_sys::Win32::Foundation::{FALSE, TRUE};
use windows_sys::Win32::System::Threading::{
    CloseThreadpoolWork, CreateThreadpoolWork, PTP_CALLBACK_INSTANCE, PTP_WORK,
    SubmitThreadpoolWork, WaitForThreadpoolWorkCallbacks,
};

use crate::callback_env::CallbackEnviron;

/// Heap-allocated callback state kept alive for the lifetime of the work object.
struct WorkContext {
    f: Box<dyn Fn() + Send + Sync + 'static>,
    /// Whether a submission is outstanding that the caller has not waited for.
    ///
    /// Unlike the wait and the one-shot timer, a dispatch does not settle this:
    /// `submit` may be called any number of times, so a trampoline entry would
    /// have to decrement a count rather than clear a flag. The count is not
    /// worth keeping, because a caller who never called `wait` could not have
    /// known the work had finished -- leaving the drain to `Drop` is what they
    /// did regardless of how the race turned out.
    obligation: crate::obligation::CloseObligation,
    /// This pool's entry in the self-heal registry.
    ///
    /// Held for its `Drop`, not read. The claim keeps the entry alive while
    /// this object exists, which is what makes the entry's pre-created repair
    /// work object available -- and bound to the pool, deferring its free --
    /// if a wait on the same pool is later cancelled. Only a wait reads a
    /// registration, because only a wait reaches the removal primitive that
    /// owes a repair.
    #[allow(dead_code)]
    registration: crate::heal::Registration,
}

/// Trampoline from the raw Windows callback ABI into the boxed closure.
///
/// SAFETY: `context` must point to a live `WorkContext` for the entire duration
/// of every callback invocation — guaranteed by `ThreadpoolWork`'s Drop ordering.
unsafe extern "system" fn work_trampoline(
    _instance: PTP_CALLBACK_INSTANCE,
    context: *mut core::ffi::c_void,
    _work: PTP_WORK,
) {
    // SAFETY: context is a valid *mut WorkContext for the full callback duration (see Drop).
    let ctx = unsafe { &*(context as *const WorkContext) };
    crate::trace_record!("work", "trampoline-entered", _work);
    // Stamped before the callback, not after: a dispatch that is still running
    // is evidence the pool is live, and a long callback must not look like
    // silence to the self-heal.
    // Not contained: the callback contract requires that it not unwind, and a
    // callback that breaks it aborts here rather than being silently forgiven.
    (ctx.f)();
    crate::trace_record!("work", "trampoline-left", _work);
}

/// An owned thread-pool work object.
///
/// Each call to [`ThreadpoolWork::submit`] queues one invocation of the callback
/// on the process thread pool. Multiple invocations may execute concurrently.
///
/// [`Drop`] calls `WaitForThreadpoolWorkCallbacks` (allowing in-flight callbacks
/// to complete) before releasing the callback context, so the captured closure
/// remains valid for the full lifetime of every callback execution.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use std::sync::atomic::{AtomicUsize, Ordering};
/// use windows_threadpool_sys::work::ThreadpoolWork;
///
/// let total = Arc::new(AtomicUsize::new(0));
/// let counter = Arc::clone(&total);
///
/// let work = ThreadpoolWork::new(move || {
///     counter.fetch_add(1, Ordering::SeqCst);
/// }, None)?;
///
/// // Each submission queues one independent invocation; they may run
/// // concurrently, so the callback must tolerate that.
/// for _ in 0..8 {
///     work.submit();
/// }
/// work.wait();
///
/// assert_eq!(total.load(Ordering::SeqCst), 8);
/// # Ok::<(), std::io::Error>(())
/// ```
pub struct ThreadpoolWork {
    handle: PTP_WORK,
    // Kept alive as a raw pointer until Drop has drained all callbacks.
    ctx: *mut WorkContext,
}

// SAFETY: PTP_WORK is a cross-thread handle; WorkContext contains Fn + Send + Sync.
unsafe impl Send for ThreadpoolWork {}
unsafe impl Sync for ThreadpoolWork {}

impl ThreadpoolWork {
    /// Creates a new work object that invokes `callback` each time it is submitted.
    ///
    /// Pass `Some(env)` to associate a non-default callback environment; `None`
    /// uses the process-default pool with default priority.
    pub fn new<F>(callback: F, env: Option<&mut CallbackEnviron>) -> io::Result<Self>
    where
        F: Fn() + Send + Sync + 'static,
    {
        // Read before `env` is consumed below, and registered before the object
        // exists: the entry must be able to repair this pool from the moment
        // anything of ours can dispatch on it.
        let registration = crate::heal::register(crate::heal::key_of(env.as_deref()));
        let ctx = Box::into_raw(Box::new(WorkContext {
            f: Box::new(callback),
            obligation: crate::obligation::CloseObligation::new(),
            registration,
        }));

        let env_ptr = env.map_or(ptr::null_mut(), |e| e.as_mut_ptr());

        // SAFETY: ctx is a valid heap pointer; env_ptr is valid (or null) for this call.
        let handle = crate::trace_call!("CreateThreadpoolWork", 0, 0, {
            // SAFETY: ctx is a valid heap pointer; env_ptr is valid (or null) for this call.
            unsafe { CreateThreadpoolWork(Some(work_trampoline), ctx.cast(), env_ptr.cast_const()) }
        });

        if handle == 0 {
            // SAFETY: the pool never saw ctx; reclaim it immediately.
            unsafe { drop(Box::from_raw(ctx)) };
            return Err(io::Error::last_os_error());
        }

        crate::trace_record!("work", "created", handle);
        Ok(Self { handle, ctx })
    }

    /// Queues one invocation of the callback on the thread pool.
    ///
    /// May be called repeatedly; each call queues an independent invocation.
    /// Multiple queued invocations may execute concurrently.
    pub fn submit(&self) {
        // SAFETY: the context outlives every callback and is freed only by Drop,
        // which cannot run while this borrow of self is alive.
        unsafe { &*self.ctx }.obligation.record_live_before(|| {
            crate::trace_call!("SubmitThreadpoolWork", self.handle, 0, {
                // SAFETY: handle is valid for the lifetime of self.
                unsafe { SubmitThreadpoolWork(self.handle) };
            });
            // The submit is the start of the interval a stalled dispatch is
            // measured over; without it, a `trampoline-entered` has nothing to be
            // late relative to.
            crate::trace_record!("work", "submitted", self.handle);
        });
    }

    /// Blocks until all queued and in-progress invocations have completed.
    ///
    /// This type has no separately-named synchronous close: this *is* the drain
    /// that [`Drop`] would otherwise perform, so calling it discharges the
    /// obligation `Drop` reports.
    pub fn wait(&self) {
        crate::trace_call!("WaitForThreadpoolWorkCallbacks", self.handle, 0, {
            // SAFETY: handle is valid for the lifetime of self.
            unsafe { WaitForThreadpoolWorkCallbacks(self.handle, FALSE) };
        });
        // SAFETY: the context outlives every callback and is freed only by Drop,
        // which cannot run while this borrow of self is alive.
        unsafe { &*self.ctx }.obligation.record_settled();
    }

    /// Stop accepting work and block until none is queued or executing.
    ///
    /// The drain every type in this crate offers under this name, so a caller
    /// tearing down a mixed set of objects can reach for one method.
    ///
    /// There is nothing to *stop* on a work object -- a submission cannot be
    /// withdrawn, only waited for -- so this is exactly [`wait`](Self::wait),
    /// which is why that method is documented as this type's drain. The name
    /// exists because a caller should not have to know which of this crate's
    /// types has something to stop.
    pub fn stop_and_drain(&self) {
        self.wait();
    }

    /// Cancels callbacks that have not yet started, then waits for any
    /// currently-executing invocations to finish.
    pub fn cancel_pending(&self) {
        crate::trace_call!("WaitForThreadpoolWorkCallbacks(cancel)", self.handle, 1, {
            // SAFETY: handle is valid for the lifetime of self.
            unsafe { WaitForThreadpoolWorkCallbacks(self.handle, TRUE) };
        });
    }

    /// Give up ownership, returning the raw object and its callback context.
    ///
    /// Used only by [`crate::cleanup_group::CleanupGroup`], which takes over
    /// both: a group member is released by `CloseThreadpoolCleanupGroupMembers`
    /// and must not close itself, so this suppresses this type's `Drop`.
    pub(crate) fn into_parts(self) -> (PTP_WORK, *mut c_void) {
        let this = ManuallyDrop::new(self);
        (this.handle, this.ctx.cast())
    }

    /// Free a context returned by [`ThreadpoolWork::into_parts`].
    ///
    /// # Safety
    ///
    /// `context` must come from `into_parts` on this type, its object must
    /// already have been released, and it must be freed exactly once.
    pub(crate) unsafe fn drop_context(context: *mut c_void) {
        // SAFETY: forwarded from this function's own contract.
        drop(unsafe { Box::from_raw(context.cast::<WorkContext>()) });
    }

    /// Whether `Drop` would report an undischarged drain obligation right now.
    ///
    /// Exists so the obligation's wiring can be asserted without depending on
    /// the trace, whose filter is fixed before `main` and so cannot be narrowed
    /// from inside a test.
    #[cfg(test)]
    pub(crate) fn obligation_owed(&self) -> bool {
        // SAFETY: the context outlives every callback and is freed only by Drop.
        unsafe { &*self.ctx }.obligation.is_owed()
    }
}

impl Drop for ThreadpoolWork {
    fn drop(&mut self) {
        crate::trace_record!("work", "drop-begin", self.handle);
        // Read before the drain, and emitted before it: the record marks the
        // start of the blocking interval it is reporting, so a reader sees what
        // the following gap is for rather than learning it afterwards.
        // SAFETY: the context is still live; it is freed at the end of this body.
        // Captured, not re-read later: the context is freed before the
        // fail-fast, so this must be a value rather than a borrow.
        let owed = unsafe { &*self.ctx }.obligation.is_owed();
        if owed {
            crate::trace_record!("work", crate::obligation::DROP_OBLIGATION_OWED, self.handle);
        }
        crate::trace_call!("WaitForThreadpoolWorkCallbacks", self.handle, 0, {
            // Let all in-flight callbacks run to completion before freeing the context.
            // SAFETY: handle is valid until it is closed just below.
            unsafe { WaitForThreadpoolWorkCallbacks(self.handle, FALSE) };
        });
        crate::trace_record!("work", "drop-drained", self.handle);
        crate::trace_call!("CloseThreadpoolWork", self.handle, 0, {
            // SAFETY: no callback remains, so the object can be closed once.
            unsafe { CloseThreadpoolWork(self.handle) };
        });
        // SAFETY: nothing can reach the context again; free it exactly once.
        unsafe { drop(Box::from_raw(self.ctx)) };
        crate::trace_record!("work", "drop-closed", self.handle);
        // Last, after the drain, the close and the context free: a panic
        // unwinds, so anything after it would be skipped.
        crate::obligation::fail_fast_if_owed(owed, "ThreadpoolWork", "stop_and_drain");
    }
}

#[cfg(test)]
mod tests;
