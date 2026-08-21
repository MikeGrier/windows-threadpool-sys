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

use std::collections::HashMap;
use std::io;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::directory::DirectoryHandle;
use crate::queue::{
    DEFAULT_BOUND, Notification, Outcome, Receiver, Reservation, Sender, WatchId,
    channel_with_bound,
};
use crate::servicing::{Rejected, Servicer};
use crate::session::Session;
use crate::watch::{RetryMode, WatchOptions};
use crate::watcher::DirectoryWatcher;

/// A message on the monitor's request queue.
///
/// Every variant is serviced by the single drain, so the resident state they
/// mutate needs no further synchronisation of its own (D-2).
///
/// Crate-internal: a client reaches this through [`Session::subscribe`] and
/// [`Watch::cancel`](crate::watch::Watch::cancel), which is what guarantees the
/// completion slot a request carries was reserved before it was submitted
/// (D-33). A publicly constructible request could not make that promise.
pub(crate) enum Request {
    /// Begin watching a path on behalf of one subscription.
    Subscribe {
        /// The identifier every notification from this subscription carries.
        watch: WatchId,
        /// The directory to watch. M4 resolves a file target to its parent.
        path: PathBuf,
        /// What the client stated at registration.
        options: WatchOptions,
        /// The session sink this subscription delivers to (D-11).
        sink: Sender,
        /// The slot this request's completion will occupy, taken before the
        /// request was submitted so delivering it cannot fail (D-33).
        completion: Reservation,
    },
    /// Stop watching, and release the watcher.
    Cancel {
        /// The subscription to end.
        watch: WatchId,
        /// As for `Subscribe`, but reserved at *registration* rather than at
        /// cancellation: `Drop` has no way to report a refused reservation, so
        /// the room for a cancellation completion is held for the whole life of
        /// the subscription.
        completion: Reservation,
    },
}

impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Request::Subscribe {
                watch,
                path,
                options,
                ..
            } => f
                .debug_struct("Subscribe")
                .field("watch", watch)
                .field("path", path)
                .field("options", options)
                .finish_non_exhaustive(),
            Request::Cancel { watch, .. } => f
                .debug_struct("Cancel")
                .field("watch", watch)
                .finish_non_exhaustive(),
        }
    }
}

/// One live subscription's resident state.
struct Subscribed {
    /// The watcher, once one could be started.
    ///
    /// `None` while the target cannot be opened for a *retryable* reason (D-22):
    /// the subscription is registered and will be established when the target
    /// becomes openable, which M5.1's state machine is what actually drives.
    /// Until then it exists so the registration is not silently forgotten.
    watcher: Option<DirectoryWatcher>,
    /// Kept because [`RetryMode`] is chosen at registration and consumed later,
    /// by M5.3's fault protocol; the registration call is the only place it can
    /// be stated (D-27).
    options: WatchOptions,
}

/// The state only the servicing path may mutate.
///
/// Held behind a lock rather than owned outright by the drain because teardown
/// reaches it too, from whichever thread dropped the monitor. The lock is
/// uncontended in the steady state -- the drain is the only writer, by
/// construction -- so it costs nothing and removes any question of what happens
/// when teardown races a drain.
#[derive(Default)]
struct Resident {
    /// Every live subscription, keyed by the identifier that tags its
    /// notifications. M4 re-keys this by *directory* when coalescing (D-6) gives
    /// it something to key on, and routes a directory's records to the
    /// subscriptions that match.
    watchers: HashMap<WatchId, Subscribed>,
}

/// What a [`Session`] holds of its monitor.
///
/// Shared so a client can submit from its own threads, and so every session
/// issues identifiers from one sequence -- a `WatchId` tags notifications and
/// keys resident state, so two sessions must never mint the same one.
///
/// Deliberately does **not** contain the resident state: the handler captures
/// that directly, and a handler reaching back through the object that owns its
/// servicer would be a cycle.
pub(crate) struct Core {
    servicer: Servicer<Request>,
    next_watch: AtomicU64,
}

impl Core {
    /// Enqueue a request.
    pub(crate) fn submit(&self, request: Request) -> Result<(), Rejected<Request>> {
        self.servicer.submit(request)
    }

    /// Whether the monitor is still accepting requests.
    pub(crate) fn is_open(&self) -> bool {
        self.servicer.is_open()
    }

    /// Mint the next subscription identifier.
    pub(crate) fn next_watch(&self) -> WatchId {
        WatchId::from_raw(self.next_watch.fetch_add(1, Ordering::Relaxed))
    }
}

/// Owns the servicing path and every watcher created through it.
pub struct Monitor {
    /// Shared with every [`Session`]. A session holding this alive past
    /// `Monitor::Drop` keeps only the allocation: teardown closes the path, so a
    /// surviving session reports itself shut and refuses requests rather than
    /// reaching a monitor that is no longer there.
    core: Arc<Core>,
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

        // The handler captures the resident state directly rather than reaching
        // back through the monitor, which keeps "only the drain mutates resident
        // state" visible in the types and avoids the cycle a handler holding its
        // own servicer would create.
        let state = Arc::clone(&resident);
        let servicer = Servicer::new(move |request: Request| service(&state, request))?;

        Ok(Self {
            core: Arc::new(Core {
                servicer,
                next_watch: AtomicU64::new(1),
            }),
            resident,
        })
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
        (Session::new(Arc::clone(&self.core), sender), receiver)
    }

    /// Enqueue a request for the servicing path.
    pub(crate) fn submit(&self, request: Request) -> Result<(), Rejected<Request>> {
        self.core.submit(request)
    }

    /// Block until every request submitted so far has been serviced.
    ///
    /// Lets a caller observe resident state at a defined point rather than
    /// polling it: registration is asynchronous by design (D-2), so without this
    /// "no watcher was created" is indistinguishable from "not yet".
    ///
    /// Must not be called from inside the servicing path, which would be waiting
    /// on itself.
    pub fn quiesce(&self) {
        self.core.servicer.quiesce();
    }

    /// How many subscriptions the monitor currently holds.
    #[must_use]
    pub fn watcher_count(&self) -> usize {
        lock(&self.resident).watchers.len()
    }

    /// The retry mode a subscription registered with, if it is still live.
    ///
    /// The mode is stated at registration and consumed by M5.3's fault protocol;
    /// this is what makes it observable in the meantime.
    #[must_use]
    pub fn retry_mode(&self, watch: WatchId) -> Option<RetryMode> {
        lock(&self.resident)
            .watchers
            .get(&watch)
            .map(|subscribed| subscribed.options.retry)
    }

    /// Whether a subscription is registered, established or not.
    #[must_use]
    pub fn is_registered(&self, watch: WatchId) -> bool {
        lock(&self.resident).watchers.contains_key(&watch)
    }

    /// Whether a subscription currently has a live watcher.
    ///
    /// Distinct from [`Monitor::is_registered`]: a subscription whose target
    /// cannot be opened yet is registered but not watching (D-14).
    #[must_use]
    pub fn is_watching(&self, watch: WatchId) -> bool {
        lock(&self.resident)
            .watchers
            .get(&watch)
            .is_some_and(|subscribed| subscribed.watcher.is_some())
    }

    /// Why a subscription's watcher stopped, if it has.
    ///
    /// A watcher that cannot re-arm reports nothing further, which is otherwise
    /// indistinguishable from a directory where nothing is happening -- so the
    /// state has to be observable (D-31). M5 replaces this with re-establishment,
    /// after which a stop is a transient rather than a resting state.
    #[must_use]
    pub fn stop_reason(&self, watch: WatchId) -> Option<io::Error> {
        lock(&self.resident)
            .watchers
            .get(&watch)
            .and_then(|subscribed| subscribed.watcher.as_ref())
            .and_then(DirectoryWatcher::stop_reason)
    }

    /// Whether the monitor is still accepting requests.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.core.is_open()
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
        self.core.servicer.shut_down();

        // Each watcher's drop cancels its outstanding read and waits for its
        // callbacks, so this is where the blocking teardown of D-20 actually
        // happens. Taken out of the table first, so the lock is not held across
        // the waits -- a completion callback that is still finishing has no reason
        // to be blocked on it, and holding it would invite exactly the
        // wait-on-yourself deadlock `stop` documents.
        let watchers = std::mem::take(&mut lock(&self.resident).watchers);
        drop(watchers);
    }
}

/// Apply one request to the resident state.
///
/// Runs on the servicing path, one call at a time, so nothing here needs to
/// consider a concurrent mutation (D-2).
fn service(resident: &Mutex<Resident>, request: Request) {
    match request {
        Request::Subscribe {
            watch,
            path,
            options,
            sink,
            completion,
        } => {
            let outcome = subscribe(resident, watch, &path, options, sink);
            completion.send(Notification::Completion { watch, outcome });
        }
        Request::Cancel { watch, completion } => {
            // Removed under the lock, dropped outside it: dropping tears the
            // watcher down, which blocks on its callbacks, and holding the
            // resident lock across that wait would serialise teardown against
            // every other reader for no reason.
            let removed = lock(resident).watchers.remove(&watch);
            drop(removed);

            // Sent only once the watcher is fully stopped, which is what makes
            // the ordering guarantee structural: nothing from this subscription
            // can be enqueued after this point, so a client seeing `Cancelled`
            // knows everything before it belongs to the live watch and nothing
            // after it does (D-30).
            completion.send(Notification::Completion {
                watch,
                outcome: Outcome::Cancelled,
            });
        }
    }
}

/// Register one subscription, reporting what became of it.
fn subscribe(
    resident: &Mutex<Resident>,
    watch: WatchId,
    path: &std::path::Path,
    options: WatchOptions,
    sink: Sender,
) -> Outcome {
    let directory = match DirectoryHandle::open(path) {
        Ok(directory) => directory,
        Err(error) if error.failure().is_retryable() => {
            // Registered but not yet established. D-14 has no terminal fault
            // state, so this is a state to recover from rather than a failure to
            // report; M5.1 is what drives the recovery.
            lock(resident).watchers.insert(
                watch,
                Subscribed {
                    watcher: None,
                    options,
                },
            );
            return Outcome::Establishing;
        }
        Err(error) => {
            // Permanent (D-22): the caller named something that can never be a
            // watched directory, so nothing is registered and the client is told
            // rather than left holding a `Watch` that can never fire.
            return Outcome::Failed {
                failure: error.failure(),
            };
        }
    };

    match DirectoryWatcher::start(directory, options.subtree, watch, sink) {
        Ok(watcher) => {
            lock(resident).watchers.insert(
                watch,
                Subscribed {
                    watcher: Some(watcher),
                    options,
                },
            );
            Outcome::Subscribed
        }
        Err(_) => {
            // Arming failed against a directory that opened, which D-15 classifies
            // as rearm-and-retry rather than fatal.
            lock(resident).watchers.insert(
                watch,
                Subscribed {
                    watcher: None,
                    options,
                },
            );
            Outcome::Establishing
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
