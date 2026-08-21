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

// A session's request half has nothing to carry until M3.5 defines the request
// variants (D-37), so parts of this surface have no production caller yet.
// Remove this when M3.5 lands.
#![allow(dead_code)]

use std::sync::Arc;

use crate::monitor::Request;
use crate::queue::Sender;
use crate::servicing::{Rejected, Servicer};

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
    servicer: Arc<Servicer<Request>>,
    sink: Sender,
}

impl Session {
    pub(crate) fn new(servicer: Arc<Servicer<Request>>, sink: Sender) -> Self {
        Self { servicer, sink }
    }

    /// Submit a request to the monitor.
    ///
    /// Returns as soon as the request is queued; it is serviced on the monitor's
    /// own path, never on the calling thread.
    ///
    /// # Errors
    ///
    /// Returns the request unserviced if the monitor has shut down.
    pub fn submit(&self, request: Request) -> Result<(), Rejected<Request>> {
        self.servicer.submit(request)
    }

    /// Whether the monitor behind this session is still accepting requests.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.servicer.is_open()
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
