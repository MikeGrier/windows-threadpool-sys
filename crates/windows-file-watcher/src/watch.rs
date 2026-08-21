// Copyright (c) 2026 Mike Grier
//! The affine subscription handle, and what a client states when it registers
//! one.
//!
//! # Affine, not linear
//!
//! A [`Watch`] owns a subscription's lifetime (D-5). Dropping it cancels;
//! [`Watch::cancel`] is the same thing said explicitly, for a caller that wants
//! the cancellation issued at a point it chooses rather than wherever the value
//! happens to fall out of scope. Rust is affine by nature -- a value can be
//! dropped without being used -- and true linearity is neither available nor
//! needed here, because "dropped" has a correct meaning: stop watching.
//!
//! It is `#[must_use]` for the case that meaning makes surprising:
//! `session.subscribe(...)` with the result discarded reads as "start watching"
//! and does the opposite, since the temporary is dropped at the end of the
//! statement and cancels immediately.
//!
//! Cancellation is *enqueued*, not performed inline: it is a request like any
//! other, serviced on the monitor's own path (D-2). So `cancel` and `drop` return
//! before the watch has actually stopped, and a notification already in the queue
//! still reaches the client. M3.6 adds the completion that tells a client exactly
//! where the cancellation fell in its stream.
//!
//! # The retry mode is stated at registration
//!
//! Recovery behaviour is a property of the *subscription*, not of the monitor
//! (D-27), and registration is the only place a client can state it -- so
//! [`WatchOptions`] carries it from the start even though M5.3 is what will act
//! on it. Adding it later would be a breaking change to the one call that has to
//! carry it.

// The retry mode is recorded at registration and consumed by M5.3; until then it
// is observable state rather than behaviour.
#![allow(dead_code)]

use std::io;
use std::path::Path;

use crate::monitor::Request;
use crate::queue::WatchId;
use crate::session::Session;

/// How the monitor should recover a subscription from a fault (D-27).
///
/// The choice is per subscription because a directory's single coalesced watcher
/// (D-6) may be shared by subscriptions that want different things, and the
/// monitor reconciles them by taking the earliest answer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum RetryMode {
    /// The monitor retries autonomously and indefinitely, asking nothing.
    ///
    /// The default, which is what keeps D-14's "no terminal fault state" true for
    /// a client that says nothing.
    #[default]
    Defaults,
    /// On fault the monitor asks this subscription how long to wait before trying
    /// again, and waits for the answer before scheduling anything.
    ///
    /// The cost is stated plainly: recovery then depends on the client answering.
    /// That is why it is opt-in rather than the default.
    Interactive,
}

/// What a client states when it registers a subscription.
///
/// Non-exhaustive because M4 adds the change-type filter here: build one with
/// [`WatchOptions::new`] and the setters rather than a struct literal, so that
/// addition is not a breaking change.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct WatchOptions {
    /// Whether changes below the target are reported as well.
    pub subtree: bool,
    /// How this subscription should be recovered from faults.
    pub retry: RetryMode,
}

impl WatchOptions {
    /// A non-recursive watch that the monitor recovers autonomously.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Report changes below the target as well.
    #[must_use]
    pub fn subtree(mut self, subtree: bool) -> Self {
        self.subtree = subtree;
        self
    }

    /// Choose how faults are recovered from (D-27).
    #[must_use]
    pub fn retry(mut self, retry: RetryMode) -> Self {
        self.retry = retry;
        self
    }
}

/// An owned subscription.
///
/// Dropping it cancels; see the module docs for why that makes `#[must_use]`
/// load-bearing rather than decorative.
#[must_use = "a Watch cancels its subscription when dropped, so discarding one here stops the watch immediately"]
#[derive(Debug)]
pub struct Watch {
    id: WatchId,
    session: Session,
    /// Set once a cancellation has been enqueued, so `Drop` after an explicit
    /// `cancel` does not enqueue a second one.
    cancelled: bool,
}

impl Watch {
    pub(crate) fn new(id: WatchId, session: Session) -> Self {
        Self {
            id,
            session,
            cancelled: false,
        }
    }

    /// The correlation token every notification from this subscription carries.
    ///
    /// `Copy`, so a client can keep it to route or aggregate without holding the
    /// subscription's lifetime (D-5).
    #[must_use]
    pub fn id(&self) -> WatchId {
        self.id
    }

    /// Stop watching.
    ///
    /// The same operation `Drop` performs, said explicitly. It returns once the
    /// cancellation is *enqueued*, not once it has taken effect, so a
    /// notification already in the client's queue still arrives.
    pub fn cancel(mut self) {
        self.enqueue_cancel();
    }

    /// Enqueue the cancellation exactly once.
    fn enqueue_cancel(&mut self) {
        if self.cancelled {
            return;
        }
        self.cancelled = true;
        // A shut-down monitor has already torn every watcher down, so there is
        // nothing left for this request to do and its rejection is not an error.
        let _ = self.session.submit(Request::Cancel { watch: self.id });
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.enqueue_cancel();
    }
}

/// Register a subscription and hand back its handle.
///
/// Crate-internal: the client-facing entry point is
/// [`Session::subscribe`](crate::session::Session::subscribe), which is where the
/// session's sink is bound to the subscription.
pub(crate) fn subscribe(
    session: &Session,
    path: &Path,
    options: WatchOptions,
) -> io::Result<Watch> {
    let id = session.next_watch();
    let request = Request::Subscribe {
        watch: id,
        path: path.to_path_buf(),
        options,
        // The session's own sink travels with the request, which is what binds
        // every notification from this subscription to the receiver the client
        // got alongside the session (D-11).
        sink: session.sink().clone(),
    };
    session.submit(request).map_err(|_| {
        io::Error::new(
            io::ErrorKind::NotConnected,
            "the monitor has shut down, so no new subscription can be registered",
        )
    })?;
    Ok(Watch::new(id, session.clone()))
}

#[cfg(test)]
mod tests;
