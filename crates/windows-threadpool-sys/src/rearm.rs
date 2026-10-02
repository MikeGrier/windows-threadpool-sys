// Copyright (c) Mike Grier
//! Suppressing a callback's re-arm while a teardown is in progress.
//!
//! Shared by `ThreadpoolWait` and `ThreadpoolTimer`, which had separate and
//! near-identical copies of this until `M-T4.10`. It is the mechanism [teardown
//! drains rather than cancels](../../../DESIGN-NOTES.md#teardown-drains) rests
//! on: without it a drain can return with the object armed again, and for
//! `Drop` that means closing the object and freeing its context with a fresh
//! callback queued against them.

use std::sync::{Mutex, MutexGuard};

/// How many callers are currently suppressing re-arming: zero means allowed.
///
/// # Why a count and not a flag
///
/// Suppression has two users with different lifetimes. `stop_and_drain` raises
/// it and lowers it again, because the object stays usable afterwards; `Drop`
/// raises it permanently, because there is no afterwards. With a flag, a
/// `stop_and_drain` finishing would clear a suppression that a concurrent one --
/// or a `Drop` -- still needed.
///
/// # What the lock may not be held across
///
/// Only the native arm or disarm call. Never a callback drain: a callback
/// blocked on this lock could then never finish, and the drain waiting for it
/// would never return.
pub(crate) struct RearmSuppression(Mutex<u32>);

impl RearmSuppression {
    /// Nothing suppressed yet.
    pub(crate) const fn new() -> Self {
        Self(Mutex::new(0))
    }

    /// Lock the count, recovering from a panicking holder.
    ///
    /// A poisoned lock here means a callback panicked while arming. The count
    /// is a plain integer with no invariant a panic can break halfway, and
    /// refusing to tear down because of an earlier panic would turn a reported
    /// fault into a hang, so the guard is taken either way.
    ///
    /// Exposed rather than wrapped because the arming paths hold the guard
    /// across their own native call and read the count while deciding: they
    /// take it, return early if it is non-zero, and otherwise arm while still
    /// holding it. That sequence is what makes arming atomic against
    /// [`suppress_and`](Self::suppress_and), so it cannot move in here.
    pub(crate) fn lock(&self) -> MutexGuard<'_, u32> {
        self.0.lock().unwrap_or_else(|poison| poison.into_inner())
    }

    /// Raise the count, then run `disarm` under the same acquisition.
    ///
    /// Doing both under one lock is what makes the pair atomic against a
    /// callback: a re-arm either lands entirely before this, or is suppressed
    /// by it. `disarm` receives the new count, which callers record.
    ///
    /// The native call stays in the closure rather than moving in here, because
    /// it differs per type -- `SetThreadpoolWait` against `SetThreadpoolTimer` --
    /// and so does the trace record that accompanies it.
    pub(crate) fn suppress_and(&self, disarm: impl FnOnce(u32)) {
        let mut suppressed = self.lock();
        *suppressed = suppressed.saturating_add(1);
        disarm(*suppressed);
    }

    /// Stop suppressing re-arming.
    pub(crate) fn release(&self) {
        let mut suppressed = self.lock();
        *suppressed = suppressed.saturating_sub(1);
    }
}

#[cfg(test)]
mod tests;
