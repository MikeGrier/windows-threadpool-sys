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
    fn on(&mut self, notification: &Notification);

    /// Optional invariant check the harness may call after each notification and
    /// at the end of a run. Return `Err(reason)` to signal *your own* pathology
    /// (an invariant your handler must maintain has been violated). The default
    /// reports healthy, so a handler with no cross-notification invariant need
    /// not implement it.
    fn check(&self) -> Result<(), String> {
        Ok(())
    }
}
