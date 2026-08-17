// Copyright (c) 2026 Mike Grier
//! SDK-equivalent helpers for [`TP_CALLBACK_ENVIRON_V3`].
//!
//! The Windows SDK's callback-environment functions are header-only inline
//! helpers that `windows-sys` does not emit. This module provides Rust
//! equivalents: a properly initialized [`CallbackEnviron`] wrapper and typed
//! mutation methods matching `SetThreadpoolCallback*`.

use core::mem;

use windows_sys::Win32::System::Threading::{
    PTP_CLEANUP_GROUP, PTP_CLEANUP_GROUP_CANCEL_CALLBACK, TP_CALLBACK_ENVIRON_V3,
    TP_CALLBACK_ENVIRON_V3_0, TP_CALLBACK_PRIORITY, TP_CALLBACK_PRIORITY_NORMAL,
};

use crate::pool::ThreadpoolPool;

/// Equivalent to `InitializeThreadpoolEnvironment` / `DestroyThreadpoolEnvironment`.
///
/// Wraps [`TP_CALLBACK_ENVIRON_V3`] with a guaranteed-valid initial state and a
/// typed mutation surface. Construct with [`CallbackEnviron::new`] (or
/// [`Default`]), mutate with the `set_*` methods, then pass to a thread-pool
/// object creation function via [`CallbackEnviron::as_mut_ptr`].
///
/// `Drop` models `DestroyThreadpoolEnvironment`, which is currently a no-op in
/// the SDK but marks the lifecycle boundary.
pub struct CallbackEnviron(TP_CALLBACK_ENVIRON_V3);

impl CallbackEnviron {
    /// Returns a properly initialized callback environment.
    ///
    /// Equivalent to `InitializeThreadpoolEnvironment`: sets `Version = 3`,
    /// `CallbackPriority = TP_CALLBACK_PRIORITY_NORMAL`, and
    /// `Size = sizeof(TP_CALLBACK_ENVIRON_V3)`, with all other fields zeroed
    /// or `None`.
    pub fn new() -> Self {
        Self(TP_CALLBACK_ENVIRON_V3 {
            Version: 3,
            Pool: 0,
            CleanupGroup: 0,
            CleanupGroupCancelCallback: None,
            RaceDll: core::ptr::null_mut(),
            ActivationContext: 0,
            FinalizationCallback: None,
            u: TP_CALLBACK_ENVIRON_V3_0 { Flags: 0 },
            CallbackPriority: TP_CALLBACK_PRIORITY_NORMAL,
            Size: mem::size_of::<TP_CALLBACK_ENVIRON_V3>() as u32,
        })
    }

    /// Equivalent to `SetThreadpoolCallbackPool`.
    ///
    /// Callbacks created with this environment run on `pool` instead of the
    /// process-default pool. The environment borrows the pool, and objects
    /// created from the environment copy its contents, so the pool must also
    /// outlive those objects -- see [`ThreadpoolPool`] for the ordering rule.
    ///
    /// Use [`CallbackEnviron::clear_pool`] to go back to the default pool.
    pub fn set_pool(&mut self, pool: &ThreadpoolPool) {
        self.0.Pool = pool.as_raw();
    }

    /// Restore the process-default pool, dropping any [`ThreadpoolPool`] this
    /// environment named.
    pub fn clear_pool(&mut self) {
        self.0.Pool = 0;
    }

    /// Equivalent to `SetThreadpoolCallbackCleanupGroup`.
    ///
    /// # Safety
    ///
    /// This takes a raw `PTP_CLEANUP_GROUP` because the crate has no owned
    /// cleanup group yet, so the caller must guarantee that:
    ///
    /// - `group` is a live cleanup group from `CreateThreadpoolCleanupGroup`,
    ///   or `0` to clear the setting;
    /// - it outlives every object created with this environment; and
    /// - once `CloseThreadpoolCleanupGroupMembers` releases those objects, they
    ///   are neither used nor closed again. This crate's callback objects close
    ///   themselves on drop, so a group-owned object of any of those types would
    ///   be closed twice -- do not place them in a cleanup group until the crate
    ///   models group membership.
    pub unsafe fn set_cleanup_group(
        &mut self,
        group: PTP_CLEANUP_GROUP,
        cancel_callback: PTP_CLEANUP_GROUP_CANCEL_CALLBACK,
    ) {
        self.0.CleanupGroup = group;
        self.0.CleanupGroupCancelCallback = cancel_callback;
    }

    /// Equivalent to `SetThreadpoolCallbackPriority`.
    pub fn set_priority(&mut self, priority: TP_CALLBACK_PRIORITY) {
        self.0.CallbackPriority = priority;
    }

    /// Equivalent to `SetThreadpoolCallbackRunsLong`.
    ///
    /// Hints to the thread pool that this callback may run for a long time,
    /// allowing the pool to spawn additional threads.
    pub fn set_runs_long(&mut self) {
        // SAFETY: Flags and s._bitfield alias the same u32; bit 0 is LongFunction.
        unsafe { self.0.u.Flags |= 1 }
    }

    /// Equivalent to `SetThreadpoolCallbackLibrary`.
    ///
    /// # Safety
    ///
    /// `dll` must be a valid `HMODULE` that remains loaded for the lifetime of
    /// all thread-pool objects created with this environment.
    pub unsafe fn set_library(&mut self, dll: *mut core::ffi::c_void) {
        self.0.RaceDll = dll;
    }

    /// Returns a mutable pointer to the inner [`TP_CALLBACK_ENVIRON_V3`].
    ///
    /// For passing to thread-pool object creation functions.
    pub fn as_mut_ptr(&mut self) -> *mut TP_CALLBACK_ENVIRON_V3 {
        &raw mut self.0
    }

    /// Returns a shared reference to the inner [`TP_CALLBACK_ENVIRON_V3`].
    pub fn as_inner(&self) -> &TP_CALLBACK_ENVIRON_V3 {
        &self.0
    }
}

impl Default for CallbackEnviron {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for CallbackEnviron {
    fn drop(&mut self) {
        // DestroyThreadpoolEnvironment is a no-op in the current SDK; models the lifecycle boundary.
    }
}

#[cfg(test)]
mod tests;
