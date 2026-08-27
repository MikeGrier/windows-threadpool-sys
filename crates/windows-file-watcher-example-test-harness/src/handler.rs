// Copyright (c) 2026 Mike Grier
//! The one plug point: your notification-handling logic under test.

use windows_file_watcher::Notification;

/// Your notification-handling logic -- the thing under test. Implement it for
/// your own handler, or reuse [`crate::example_handler::PresenceTracker`].
///
/// The harness calls [`Handler::on`] once per delivered notification, in the
/// order the schedule dictates, on a single thread. It never calls into your
/// code concurrently, so your handler's own synchronization (if any) is exercised
/// only to the extent your `on` uses it.
pub trait Handler {
    /// React to one notification, exactly as your production drain loop would.
    ///
    /// Under the **oracles** ([`crate::run`] / [`crate::run_with_deadline`]) a
    /// panic here is caught and reported as [`crate::PathologyKind::Panicked`].
    /// Under [`crate::drive`] it is **not**: `drive` is a plain driver with no
    /// `catch_unwind`, so a panic propagates to your test as an ordinary test
    /// failure. Both are useful -- use `drive` when a panic *should* fail the
    /// test directly, and an oracle when you want it reported as an outcome.
    fn on(&mut self, notification: &Notification);

    /// Optional invariant check.
    ///
    /// **Only the oracles call it.** [`crate::run`] and
    /// [`crate::run_with_deadline`] call it after *every* delivered
    /// notification and once more at the end of a run -- unconditionally, not
    /// sampled. [`crate::drive`] never calls it at all, so a handler driven
    /// that way has its invariant evaluated only where you call `check`
    /// yourself.
    ///
    /// Return `Err(reason)` to signal *your own* pathology (an invariant your
    /// handler must maintain has been violated), which is reported as
    /// [`crate::PathologyKind::InvariantViolated`]. The default reports healthy,
    /// so a handler with no cross-notification invariant need not implement it.
    ///
    /// Returning `Err` and panicking are both supported and both attributed to
    /// your handler: under an oracle a panic here (an `assert!`, say) is caught
    /// exactly as one in [`Handler::on`] is, and reported as
    /// [`crate::PathologyKind::Panicked`] with the message prefixed
    /// `in check():` -- never charged to the harness.
    fn check(&self) -> Result<(), String> {
        Ok(())
    }
}
