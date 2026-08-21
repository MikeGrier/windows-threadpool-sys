// Copyright (c) 2026 Mike Grier
//! The monitor: the crate's single servicing authority and the owner of every
//! per-directory watcher.
//!
//! A `Monitor` is created once and lives as long as the watching does. It owns no
//! threads (D-3): its servicing runs on a [`Servicer`], which is one
//! `ThreadpoolWork` draining the request queue, and its watchers run on the
//! pool's I/O completions. Client interaction is entirely queued in both
//! directions -- requests in, notifications out -- so the crate never calls into
//! client code on its cadence path (D-2).
//!
//! # Teardown blocks
//!
//! [`Monitor::shut_down`] closes the servicing path *before* tearing down
//! watchers, which is the ordering D-23 and D-34 exist to enforce: with the queue
//! still open a drain could adopt a watcher after teardown had already walked the
//! table, leaving an armed read behind that nothing would ever cancel. `Drop`
//! delegates to it, so a monitor that goes out of scope blocks until every read is
//! cancelled and every callback has finished (D-20).

// The monitor's request path has no variants to carry until M3.5 defines them,
// and its resident table is populated by the same item; until then only this
// crate's tests reach the surface. Remove this when M3.5 lands.
#![allow(dead_code)]

use std::io;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};

use crate::queue::{DEFAULT_BOUND, Receiver, channel_with_bound};
use crate::servicing::{Rejected, Servicer};
use crate::session::Session;
use crate::watcher::DirectoryWatcher;

/// A message on the monitor's request queue.
///
/// Deliberately uninhabited: M3.1 builds the servicing path, and the requests
/// that travel it are defined by M3.5, which adds subscribe and cancel along with
/// the affine `Watch` handle that issues them. An empty enum states that honestly
/// -- the queue is typed, the handler is exhaustive, and adding a variant is a
/// pure extension -- rather than carrying a placeholder variant that would have to
/// be removed later. What the path *guarantees* is exercised on its own terms in
/// [`crate::servicing`], which is generic over the request type for exactly this
/// reason.
pub enum Request {}

/// The state only the servicing path may mutate.
///
/// Held behind a lock rather than owned outright by the drain because teardown
/// reaches it too, from whichever thread dropped the monitor. The lock is
/// uncontended in the steady state -- the drain is the only writer, by
/// construction -- so it costs nothing and removes any question of what happens
/// when teardown races a drain.
#[derive(Default)]
struct Resident {
    /// Every live watcher. A flat list for now; M4 keys it by directory when
    /// coalescing (D-6) gives it something to key on.
    watchers: Vec<DirectoryWatcher>,
}

/// Owns the servicing path and every watcher created through it.
pub struct Monitor {
    /// Shared with every [`Session`], which is what lets a client submit from its
    /// own threads. A session holding this alive past `Monitor::Drop` keeps only
    /// the allocation: teardown closes the path, so a surviving session reports
    /// itself shut and refuses requests rather than reaching a monitor that is
    /// no longer there.
    servicer: Arc<Servicer<Request>>,
    resident: Arc<Mutex<Resident>>,
}

impl Monitor {
    /// Create a monitor with an idle servicing path.
    ///
    /// # Errors
    ///
    /// Returns the error from creating the servicing path's work object.
    pub fn new() -> io::Result<Self> {
        let resident = Arc::new(Mutex::new(Resident::default()));

        // The handler is built before the monitor exists, so when M3.5 gives it
        // work to do it will capture `Arc::clone(&resident)` directly rather than
        // reaching back through the monitor. That keeps "only the drain mutates
        // resident state" visible in the types, and avoids the cycle a handler
        // holding the monitor would create.
        let servicer = Arc::new(Servicer::new(|request: Request| match request {})?);

        Ok(Self { servicer, resident })
    }

    /// Open a session, returning it with the receiver its notifications arrive on.
    ///
    /// Every watch created through the returned session delivers here (D-11), so
    /// a client chooses its routing once rather than per subscription: one session
    /// per consumer, and each consumer drains only what it asked for.
    ///
    /// The notification queue is bounded at [`DEFAULT_BOUND`]; use
    /// [`Monitor::session_with_bound`] to choose.
    #[must_use]
    pub fn session(&self) -> (Session, Receiver) {
        self.session_with_bound(DEFAULT_BOUND)
    }

    /// As [`Monitor::session`], with an explicit notification-queue bound.
    ///
    /// The bound counts notifications, not changes: one decoded completion is one
    /// batch (D-10), which may carry hundreds of records.
    #[must_use]
    pub fn session_with_bound(&self, bound: NonZeroUsize) -> (Session, Receiver) {
        let (sender, receiver) = channel_with_bound(bound);
        (Session::new(Arc::clone(&self.servicer), sender), receiver)
    }

    /// Enqueue a request for the servicing path.
    ///
    /// # Errors
    ///
    /// Returns the request unserviced if the monitor has shut down.
    pub fn submit(&self, request: Request) -> Result<(), Rejected<Request>> {
        self.servicer.submit(request)
    }

    /// Take ownership of a watcher.
    ///
    /// Called from the serialised handler, which is the only writer of resident
    /// state in the steady state; teardown is the only other reader.
    pub(crate) fn adopt(&self, watcher: DirectoryWatcher) {
        lock(&self.resident).watchers.push(watcher);
    }

    /// How many watchers the monitor currently owns.
    pub fn watcher_count(&self) -> usize {
        lock(&self.resident).watchers.len()
    }

    /// Whether the monitor is still accepting requests.
    pub fn is_running(&self) -> bool {
        self.servicer.is_open()
    }

    /// Stop servicing and tear down every watcher, blocking until it is done.
    ///
    /// Idempotent, so a caller may shut down explicitly and still let `Drop` run.
    /// After it returns, no handler and no completion callback is executing or can
    /// start, and every outstanding read has been cancelled (D-20).
    pub fn shut_down(&self) {
        // Order matters: closing the servicing path first means nothing can adopt
        // a watcher after the table below has been walked. The other order leaves
        // an armed read owned by nobody (D-23/D-34).
        self.servicer.shut_down();

        // Each `stop` cancels that watcher's outstanding read and waits for its
        // callbacks, so this is where the blocking teardown of D-20 actually
        // happens. Taken out of the table first, so the lock is not held across
        // the waits -- a completion callback that is still finishing has no reason
        // to be blocked on it, and holding it would invite exactly the
        // wait-on-yourself deadlock `stop` documents.
        let watchers = std::mem::take(&mut lock(&self.resident).watchers);
        for watcher in &watchers {
            watcher.stop();
        }
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        self.shut_down();
    }
}

impl std::fmt::Debug for Monitor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Monitor")
            .field("running", &self.is_running())
            .field("watchers", &self.watcher_count())
            .finish_non_exhaustive()
    }
}

/// Lock, recovering the guard if a previous holder panicked.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

#[cfg(test)]
mod tests;
