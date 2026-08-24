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
    // Not contained: the callback contract requires that it not unwind, and a
    // callback that breaks it aborts here rather than being silently forgiven.
    (ctx.f)();
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
        let ctx = Box::into_raw(Box::new(WorkContext {
            f: Box::new(callback),
        }));

        let env_ptr = env.map_or(ptr::null_mut(), |e| e.as_mut_ptr());

        // SAFETY: ctx is a valid heap pointer; env_ptr is valid (or null) for this call.
        let handle = unsafe {
            CreateThreadpoolWork(Some(work_trampoline), ctx.cast(), env_ptr.cast_const())
        };

        if handle == 0 {
            // SAFETY: the pool never saw ctx; reclaim it immediately.
            unsafe { drop(Box::from_raw(ctx)) };
            return Err(io::Error::last_os_error());
        }

        Ok(Self { handle, ctx })
    }

    /// Queues one invocation of the callback on the thread pool.
    ///
    /// May be called repeatedly; each call queues an independent invocation.
    /// Multiple queued invocations may execute concurrently.
    pub fn submit(&self) {
        // SAFETY: handle is valid for the lifetime of self.
        unsafe { SubmitThreadpoolWork(self.handle) };
    }

    /// Blocks until all queued and in-progress invocations have completed.
    pub fn wait(&self) {
        // SAFETY: handle is valid for the lifetime of self.
        unsafe { WaitForThreadpoolWorkCallbacks(self.handle, FALSE) };
    }

    /// Cancels callbacks that have not yet started, then waits for any
    /// currently-executing invocations to finish.
    pub fn cancel_pending(&self) {
        // SAFETY: handle is valid for the lifetime of self.
        unsafe { WaitForThreadpoolWorkCallbacks(self.handle, TRUE) };
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
}

impl Drop for ThreadpoolWork {
    fn drop(&mut self) {
        unsafe {
            // Let all in-flight callbacks run to completion before freeing the context.
            WaitForThreadpoolWorkCallbacks(self.handle, FALSE);
            CloseThreadpoolWork(self.handle);
            drop(Box::from_raw(self.ctx));
        }
    }
}

#[cfg(test)]
mod tests;
