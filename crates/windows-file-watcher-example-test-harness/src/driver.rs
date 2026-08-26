// Copyright (c) 2026 Mike Grier
//! Driving a handler through a schedule.

use windows_file_watcher::{DEFAULT_BOUND, channel_with_bound};

use crate::{Handler, Schedule};

/// Drive `handler` through `schedule`.
///
/// This is the whole point of the harness: it builds a real file-watcher
/// notification channel, feeds it the scheduled notifications, and dispatches
/// each drained notification to your handler -- exactly the delivery path
/// production uses, but with the schedule as the sole source of events. No
/// filesystem and no thread pool are involved, so the run is deterministic.
///
/// Each step is sent and then drained, so a long schedule never saturates the
/// bounded queue.
pub fn drive(schedule: &Schedule, handler: &mut impl Handler) {
    let (sender, receiver) = channel_with_bound(DEFAULT_BOUND);
    for spec in &schedule.steps {
        // Best-effort send, mirroring the crate's own cadence path.
        let _ = sender.send(spec.to_notification());
        while let Some(notification) = receiver.try_recv() {
            handler.on(&notification);
        }
    }
    // Drain anything left (there should be nothing, since we drain each step).
    while let Some(notification) = receiver.try_recv() {
        handler.on(&notification);
    }
}
