// Copyright (c) 2026 Mike Grier
//! A client's handle on the monitor: what it submits requests through, and where
//! everything it subscribes to is delivered.
//!
//! # Why one object carries both halves
//!
//! A session bundles the request-submission handle with the notification sender
//! (D-2/D-11) rather than leaving them separate, because that is what makes the
//! binding between a subscription and its destination structural. Every watch
//! created through a session delivers to that session's sender, so a client
//! chooses its routing once -- at session creation -- instead of restating it per
//! subscription and hoping the two agree. A client that wants several independent
//! streams opens several sessions; one that wants everything in one place opens
//! one and clones it.
//!
//! # The sender is concrete, not a trait
//!
//! Delivery is an enqueue *the crate performs* onto storage the crate owns. There
//! is no sink trait and no client closure anywhere on this path, so nothing a
//! client does -- blocking, panicking, being slow -- can stall or unwind a
//! completion callback (D-2/D-11). The client only ever receives, on whatever
//! thread it likes.
//!
//! # Outliving the monitor
//!
//! A session holds the servicing path alive, but that is an allocation, not a
//! lifetime: `Monitor::Drop` shuts the path down whatever sessions still exist
//! (D-20), after which a surviving session reports itself closed and hands back
//! any request it is given. The alternative -- letting a forgotten session keep a
//! monitor and its watchers running -- would make teardown depend on the client
//! having dropped everything in the right order.

use std::io;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::monitor::{Core, Request};
use crate::queue::{Sender, WatchId};
use crate::servicing::Rejected;
use crate::watch::{Watch, WatchOptions};

/// A client's connection to a monitor.
///
/// Obtained from [`Monitor::session`](crate::monitor::Monitor::session), which
/// returns it together with the receiver its notifications arrive on.
///
/// Cloning is how a client gets the multi-producer submission D-11 requires:
/// every clone submits to the same servicing path and delivers to the same
/// receiver, so several client threads can share one stream without coordinating.
#[derive(Clone)]
pub struct Session {
    core: Arc<Core>,
    sink: Sender,
}

impl Session {
    pub(crate) fn new(core: Arc<Core>, sink: Sender) -> Self {
        Self { core, sink }
    }

    /// Watch `path`, delivering to this session's receiver.
    ///
    /// Registration is asynchronous: this returns as soon as the request is
    /// queued, and the watch begins on the monitor's own servicing path. The
    /// returned handle owns the subscription's lifetime -- dropping it cancels --
    /// so it must be kept for as long as the client wants notifications.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::WouldBlock`] if the notification queue has no
    /// room to guarantee this subscription's registration and cancellation
    /// completions (D-33) -- backpressure lands here, synchronously, rather
    /// than at a later delivery that has no safe way to fail. Returns
    /// [`io::ErrorKind::NotConnected`] if the monitor has already shut down.
    /// A path that cannot be watched is not reported by either of those:
    /// whether it can be is not known until the monitor tries, and arrives
    /// asynchronously as a [`Notification::Completion`](crate::Notification::Completion)
    /// (D-30).
    pub fn subscribe(&self, path: impl AsRef<Path>, options: WatchOptions) -> io::Result<Watch> {
        crate::watch::subscribe(self, path.as_ref(), options)
    }

    /// Answer an interactive subscription's [`Notification::RetryQuestion`](crate::Notification::RetryQuestion)
    /// (D-27): `Some(delay)` names the next retry delay, `None` declines and is
    /// counted at the failing operation's default. Not itself a request with a
    /// lifecycle -- it is a response to one the crate posed -- so it carries no
    /// completion and simply does nothing if `watch` is not currently awaiting
    /// an answer (already resolved, already cancelled, or never asked).
    pub fn answer(&self, watch: WatchId, delay: Option<Duration>) {
        let _ = self.submit(Request::Answer { watch, delay });
    }

    /// Submit a request to the monitor.
    ///
    /// Crate-internal; see [`Request`] for why.
    pub(crate) fn submit(&self, request: Request) -> Result<(), Rejected<Request>> {
        self.core.submit(request)
    }

    /// Whether the monitor behind this session is still accepting requests.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.core.is_open()
    }

    /// Mint the next subscription identifier.
    pub(crate) fn next_watch(&self) -> WatchId {
        self.core.next_watch()
    }

    /// The sender every watch created through this session delivers to.
    ///
    /// Crate-internal: a client never sends, only receives (D-11).
    pub(crate) fn sink(&self) -> &Sender {
        &self.sink
    }
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("open", &self.is_open())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
