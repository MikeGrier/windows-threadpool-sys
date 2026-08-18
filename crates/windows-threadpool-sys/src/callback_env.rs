// Copyright (c) 2026 Mike Grier
//! SDK-equivalent helpers for [`TP_CALLBACK_ENVIRON_V3`].
//!
//! The Windows SDK's callback-environment functions are header-only inline
//! helpers that `windows-sys` does not emit. This module provides Rust
//! equivalents: a properly initialized [`CallbackEnviron`] wrapper and typed
//! mutation methods matching `SetThreadpoolCallback*`.

use core::mem;
use std::marker::PhantomData;

use windows_sys::Win32::System::Threading::{
    PTP_CLEANUP_GROUP, PTP_CLEANUP_GROUP_CANCEL_CALLBACK, TP_CALLBACK_ENVIRON_V3,
    TP_CALLBACK_ENVIRON_V3_0, TP_CALLBACK_PRIORITY, TP_CALLBACK_PRIORITY_NORMAL,
};

use crate::pool::ThreadpoolPool;

/// Bit positions within `TP_CALLBACK_ENVIRON_V3`'s flags word.
///
/// The SDK declares these as a bitfield on `TP_CALLBACK_ENVIRON_V3_0`, which
/// `windows-sys` exposes only as the aliasing `Flags: u32`. The bit positions
/// are therefore part of the ABI this crate depends on, and are named here
/// rather than written inline at the point of use.
///
/// Changing any value is a breaking change: these describe the layout the
/// operating system reads, not a private encoding.
mod environ_flags {
    /// `LongFunction`: the callback may run long, so the pool may add threads.
    /// Set by `SetThreadpoolCallbackRunsLong`.
    pub(super) const LONG_FUNCTION: u32 = 1 << 0;
}

/// The `TP_CALLBACK_ENVIRON_V3` structure version this crate initializes.
///
/// Like the flag bits above, this is an ABI identity the operating system reads,
/// not a private encoding: it selects which layout the pool expects at the
/// address it is handed. Changing it is a breaking change, and it must stay
/// consistent with the `Size` field, which is taken from the V3 struct.
const ENVIRON_VERSION: u32 = 3;

/// Equivalent to `InitializeThreadpoolEnvironment` / `DestroyThreadpoolEnvironment`.
///
/// Wraps [`TP_CALLBACK_ENVIRON_V3`] with a guaranteed-valid initial state and a
/// typed mutation surface. Construct with [`CallbackEnviron::new`] (or
/// [`Default`]), mutate with the `set_*` methods, then pass to a thread-pool
/// object creation function via [`CallbackEnviron::as_mut_ptr`].
///
/// `Drop` models `DestroyThreadpoolEnvironment`, which is currently a no-op in
/// the SDK but marks the lifecycle boundary.
///
/// # Pool lifetime
///
/// An environment that names a [`ThreadpoolPool`] borrows it, so this sequence
/// -- which would otherwise create an object from a dangling pool value -- does
/// not compile:
///
/// ```compile_fail
/// use windows_threadpool_sys::callback_env::CallbackEnviron;
/// use windows_threadpool_sys::pool::ThreadpoolPool;
///
/// let mut env = CallbackEnviron::new();
/// {
///     let pool = ThreadpoolPool::new().expect("create pool");
///     env.set_pool(&pool);
/// } // `pool` is dropped here
/// let _ptr = env.as_mut_ptr(); // error: `pool` does not live long enough
/// ```
pub struct CallbackEnviron<'pool> {
    inner: TP_CALLBACK_ENVIRON_V3,
    /// Ties the environment to the [`ThreadpoolPool`] it names.
    ///
    /// The environment stores only the pool's raw `PTP_POOL` value, which the
    /// thread pool dereferences when an object is created from it. Without this
    /// marker, safe code could set a pool, drop it, and then create an object
    /// from the still-live environment with a dangling pool -- so the borrow has
    /// to be real, not merely implied by the `&ThreadpoolPool` parameter.
    pool: PhantomData<&'pool ThreadpoolPool>,
}

impl<'pool> CallbackEnviron<'pool> {
    /// Returns a properly initialized callback environment.
    ///
    /// Equivalent to `InitializeThreadpoolEnvironment`: sets
    /// `Version = ENVIRON_VERSION`,
    /// `CallbackPriority = TP_CALLBACK_PRIORITY_NORMAL`, and
    /// `Size = sizeof(TP_CALLBACK_ENVIRON_V3)`, with all other fields zeroed
    /// or `None`.
    pub fn new() -> Self {
        Self {
            inner: TP_CALLBACK_ENVIRON_V3 {
                Version: ENVIRON_VERSION,
                Pool: 0,
                CleanupGroup: 0,
                CleanupGroupCancelCallback: None,
                RaceDll: core::ptr::null_mut(),
                ActivationContext: 0,
                FinalizationCallback: None,
                u: TP_CALLBACK_ENVIRON_V3_0 { Flags: 0 },
                CallbackPriority: TP_CALLBACK_PRIORITY_NORMAL,
                Size: mem::size_of::<TP_CALLBACK_ENVIRON_V3>() as u32,
            },
            pool: PhantomData,
        }
    }

    /// Equivalent to `SetThreadpoolCallbackPool`.
    ///
    /// Callbacks created with this environment run on `pool` instead of the
    /// process-default pool.
    ///
    /// The environment genuinely borrows the pool for as long as it names it, so
    /// the pool cannot be dropped while this environment is still usable. That
    /// borrow is what makes the setter sound: the environment stores only the
    /// pool's raw value, which the thread pool dereferences when an object is
    /// created, so a dropped pool would otherwise leave a dangling value behind.
    ///
    /// Objects created from the environment copy its contents, so the pool must
    /// also outlive those objects -- see [`ThreadpoolPool`] for that ordering
    /// rule, which the compiler cannot check.
    ///
    /// Use [`CallbackEnviron::clear_pool`] to go back to the default pool.
    pub fn set_pool(&mut self, pool: &'pool ThreadpoolPool) {
        self.inner.Pool = pool.as_raw();
    }

    /// Clear the pool selection, so objects created with this environment use
    /// the process-default pool again.
    ///
    /// This only clears the selection. Any [`ThreadpoolPool`] the environment
    /// named is borrowed, not owned, so it is neither dropped nor closed, and
    /// the environment keeps its `'pool` lifetime -- the borrow is released when
    /// the environment itself is dropped, not here.
    pub fn clear_pool(&mut self) {
        self.inner.Pool = 0;
    }

    /// Equivalent to `SetThreadpoolCallbackCleanupGroup`.
    ///
    /// Prefer [`CleanupGroup`], which creates its own members and upholds every
    /// requirement below for you. This raw seam exists for handing the
    /// environment to a cleanup group this crate does not own.
    ///
    /// # Safety
    ///
    /// This takes a raw `PTP_CLEANUP_GROUP`, so the caller must guarantee that:
    ///
    /// - `group` is a live cleanup group from `CreateThreadpoolCleanupGroup`,
    ///   or `0` to clear the setting;
    /// - it outlives every object created with this environment; and
    /// - once `CloseThreadpoolCleanupGroupMembers` releases those objects, they
    ///   are neither used nor closed again. This crate's individually-owned
    ///   callback objects close themselves on drop, so putting one of those in a
    ///   foreign cleanup group would close it twice. Use [`CleanupGroup`] to get
    ///   members that are released by the group instead.
    ///
    /// [`CleanupGroup`]: crate::cleanup_group::CleanupGroup
    pub unsafe fn set_cleanup_group(
        &mut self,
        group: PTP_CLEANUP_GROUP,
        cancel_callback: PTP_CLEANUP_GROUP_CANCEL_CALLBACK,
    ) {
        self.inner.CleanupGroup = group;
        self.inner.CleanupGroupCancelCallback = cancel_callback;
    }

    /// Equivalent to `SetThreadpoolCallbackPriority`.
    pub fn set_priority(&mut self, priority: TP_CALLBACK_PRIORITY) {
        self.inner.CallbackPriority = priority;
    }

    /// Equivalent to `SetThreadpoolCallbackRunsLong`.
    ///
    /// Hints to the thread pool that this callback may run for a long time,
    /// allowing the pool to spawn additional threads.
    pub fn set_runs_long(&mut self) {
        // SAFETY: `Flags` and `s._bitfield` are the two halves of a union over the
        // same u32, so writing through `Flags` sets the bitfield the SDK reads.
        unsafe { self.inner.u.Flags |= environ_flags::LONG_FUNCTION }
    }

    /// Equivalent to `SetThreadpoolCallbackLibrary`.
    ///
    /// # Safety
    ///
    /// `dll` must be a valid `HMODULE` that remains loaded for the lifetime of
    /// all thread-pool objects created with this environment.
    pub unsafe fn set_library(&mut self, dll: *mut core::ffi::c_void) {
        self.inner.RaceDll = dll;
    }

    /// Wrap an already-initialized environment structure.
    ///
    /// Used to copy an environment rather than mutate a caller's, so that
    /// layering a setting on top -- as a cleanup group does when creating a
    /// member -- cannot be observed through the original.
    pub(crate) fn from_inner(inner: TP_CALLBACK_ENVIRON_V3) -> Self {
        Self {
            inner,
            pool: PhantomData,
        }
    }

    /// Returns a mutable pointer to the inner [`TP_CALLBACK_ENVIRON_V3`].
    ///
    /// For passing to thread-pool object creation functions.
    pub fn as_mut_ptr(&mut self) -> *mut TP_CALLBACK_ENVIRON_V3 {
        &raw mut self.inner
    }

    /// Returns a shared reference to the inner [`TP_CALLBACK_ENVIRON_V3`].
    pub fn as_inner(&self) -> &TP_CALLBACK_ENVIRON_V3 {
        &self.inner
    }
}

impl Default for CallbackEnviron<'_> {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for CallbackEnviron<'_> {
    fn drop(&mut self) {
        // DestroyThreadpoolEnvironment is a no-op in the current SDK; models the lifecycle boundary.
    }
}

#[cfg(test)]
mod tests;
