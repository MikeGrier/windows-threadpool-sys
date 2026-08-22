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
//! [`WatchOptions`] carries it from the start. The monitor's fault machinery
//! reads it on every fault (D-27's interactive retry protocol): an
//! `Interactive` subscription is asked how long to wait before the next
//! attempt via a [`crate::queue::Notification::RetryQuestion`], while a
//! `Defaults` one retries autonomously at a fixed delay.

use std::io;
use std::path::Path;

use crate::monitor::Request;
use crate::queue::{Reservation, WatchId};
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
    /// Whether `Suspended`/`Resumed`/`Established` are reported (D-13). Off by
    /// default: a subscription that only wants change data need not think about
    /// the fault machine's liveness brackets at all.
    pub report_liveness: bool,
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

    /// Opt in to `Suspended`/`Resumed`/`Established` (D-13).
    #[must_use]
    pub fn report_liveness(mut self, report_liveness: bool) -> Self {
        self.report_liveness = report_liveness;
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
    /// The queue slot the cancellation completion will occupy, reserved at
    /// registration.
    ///
    /// Held for the whole life of the subscription because `Drop` is a
    /// cancellation path with nowhere to report a refused reservation: taking
    /// the room up front is what makes "dropping cancels" reliable rather than
    /// best-effort (D-33). Taken when the cancellation is enqueued, so `None`
    /// also records that this watch has already been cancelled and `Drop` must
    /// not enqueue a second one.
    cancellation: Option<Reservation>,
}

impl Watch {
    pub(crate) fn new(id: WatchId, session: Session, cancellation: Reservation) -> Self {
        Self {
            id,
            session,
            cancellation: Some(cancellation),
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
    /// notification already in the client's queue still arrives. The
    /// `Cancelled` completion marks where the watch actually ended (D-30).
    pub fn cancel(mut self) {
        self.enqueue_cancel();
    }

    /// Enqueue the cancellation exactly once.
    fn enqueue_cancel(&mut self) {
        let Some(completion) = self.cancellation.take() else {
            return;
        };
        // A shut-down monitor has already torn every watcher down, so there is
        // nothing left for this request to do; the rejection is not an error, and
        // dropping the rejected request releases the reserved slot.
        let _ = self.session.submit(Request::Cancel {
            watch: self.id,
            completion,
        });
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
    // Both slots are taken before the request is submitted, which is what makes
    // the two completions undroppable (D-33). Reserving the cancellation slot
    // here rather than at cancellation time is what lets `Drop` cancel reliably:
    // it has nowhere to report a refusal.
    let completion = session.sink().reserve().ok_or_else(saturated)?;
    let cancellation = session.sink().reserve().ok_or_else(saturated)?;

    // A standing fault-question slot (D-27/D-28), taken once here rather than
    // per fault, so asking never competes with the queue's best-effort traffic.
    // Only an `Interactive` subscription can ever be asked (D-57): `Suspended`/
    // `Resumed`/`Established` ride the ordinary best-effort queue like any other
    // observation, so a `report_liveness`-only subscription reserves nothing
    // extra.
    let fault_slot = if options.retry == RetryMode::Interactive {
        Some(session.sink().reserve_standing().ok_or_else(saturated)?)
    } else {
        None
    };

    let id = session.next_watch();
    let request = Request::Subscribe {
        watch: id,
        path: path.to_path_buf(),
        options,
        // The session's own sink travels with the request, which is what binds
        // every notification from this subscription to the receiver the client
        // got alongside the session (D-11).
        sink: session.sink().clone(),
        completion,
        fault_slot,
    };
    session.submit(request).map_err(|_| {
        io::Error::new(
            io::ErrorKind::NotConnected,
            "the monitor has shut down, so no new subscription can be registered",
        )
    })?;
    Ok(Watch::new(id, session.clone(), cancellation))
}

/// The notification queue has no room to promise this subscription's completions.
///
/// This is where backpressure is meant to land: on the client's own thread, at
/// the call that asked for the work, rather than at a delivery with no safe way
/// to fail (D-29/D-33).
fn saturated() -> io::Error {
    io::Error::new(
        io::ErrorKind::WouldBlock,
        "the notification queue is full, so a subscription's completions cannot be guaranteed",
    )
}

#[cfg(test)]
mod tests;
