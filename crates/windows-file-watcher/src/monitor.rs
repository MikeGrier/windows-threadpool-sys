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

// M3.5's request path has landed; the request-shape comment below is stale and
// removed. `Monitor::directory_count` is test-only until M4.5's integration
// tests need to assert coalescing directly.
#![allow(dead_code)]

use std::collections::HashMap;
use std::io;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use wtf_string::Wtf16String;

use windows_threadpool_sys::timer::ThreadpoolTimer;

use crate::directory::{DirectoryHandle, DirectoryId, OpenFailure};
use crate::queue::{
    DEFAULT_BOUND, Notification, Outcome, Receiver, Reservation, Sender, StandingSlot, WatchId,
    channel_with_bound,
};
use crate::retry::{FaultOperation, clamp};
use crate::route::{Route, RouteScope};
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
        /// The standing fault-question reservation (D-27/D-28), present iff
        /// this subscription can ever need one (`retry == Interactive` or
        /// `report_liveness`).
        fault_slot: Option<StandingSlot>,
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
    /// Internal: re-attempt establishing a still-pending subscription, after
    /// its retry timer fired (M5.1). Never constructed by a client -- only
    /// [`Subscription::Pending`]'s own retry timer submits this.
    Retry {
        /// The subscription to retry.
        watch: WatchId,
    },
    /// A client's answer to an interactive subscription's fault question
    /// (D-27/M5.3), submitted through
    /// [`Session::answer`](crate::session::Session::answer). Not itself a
    /// request with a lifecycle, so it carries no completion; it is simply
    /// ignored if `watch` is not currently awaiting an answer.
    Answer {
        /// The subscription answering.
        watch: WatchId,
        /// The next retry delay, or `None` to decline (counted at the failing
        /// operation's default).
        delay: Option<Duration>,
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
            Request::Retry { watch } => f.debug_struct("Retry").field("watch", watch).finish(),
            Request::Answer { watch, delay } => f
                .debug_struct("Answer")
                .field("watch", watch)
                .field("delay", delay)
                .finish(),
        }
    }
}

/// One live subscription's resident state.
enum Subscription {
    /// Registered, but nothing could be opened yet for a *retryable* reason
    /// (D-22); a retry timer is always running while in this state (M5.1).
    Pending {
        /// The path as the client named it. Re-attempted from here every
        /// retry, since which of `path` and its parent is opened (D-7) can
        /// change once something actually exists there.
        path: PathBuf,
        options: WatchOptions,
        /// This subscription's session sink (D-11), kept so a later successful
        /// establishment (M5.1) or a permanent failure discovered on a retry
        /// has somewhere to deliver.
        sink: Sender,
        /// Fires the next `Request::Retry`. Created once at registration and
        /// re-armed (never recreated) across however many attempts this
        /// subscription needs before it establishes.
        retry_timer: ThreadpoolTimer,
        /// The standing fault-question reservation (D-27/D-28), present iff
        /// this subscription can ever need one.
        fault_slot: Option<StandingSlot>,
        /// Whether an interactive question is currently outstanding for this
        /// subscription's open fault. While `true`, the retry timer is not
        /// armed -- it waits for `Request::Answer` instead (D-27).
        awaiting_answer: bool,
    },
    /// Routed into a coalesced directory watcher (D-6).
    Routed {
        directory: DirectoryId,
        /// Kept because [`RetryMode`] is chosen at registration and consumed
        /// later, by M5.3's fault protocol; the registration call is the only
        /// place it can be stated (D-27).
        options: WatchOptions,
    },
}

impl Subscription {
    fn options(&self) -> WatchOptions {
        match self {
            Subscription::Pending { options, .. } | Subscription::Routed { options, .. } => {
                *options
            }
        }
    }
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
    /// Every subscription, keyed by the identifier that tags its notifications.
    subscriptions: HashMap<WatchId, Subscription>,
    /// Every live coalesced watcher, keyed by directory identity (D-6): a
    /// directory is watched once regardless of how many subscriptions target
    /// entries within it.
    directories: HashMap<DirectoryId, DirectoryWatcher>,
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

        // A pending subscription's retry timer needs to reach `Core::submit`
        // from its own callback, but `Core` cannot exist until after
        // `Servicer::new` returns (the servicer is one of its fields) -- so this
        // cell is captured by the handler now and filled in immediately below,
        // strictly before any request can possibly be serviced.
        let core_cell: Arc<OnceLock<Weak<Core>>> = Arc::new(OnceLock::new());

        // The handler captures the resident state directly rather than reaching
        // back through the monitor, which keeps "only the drain mutates resident
        // state" visible in the types and avoids the cycle a handler holding its
        // own servicer would create.
        let state = Arc::clone(&resident);
        let core_ref = Arc::clone(&core_cell);
        let servicer = Servicer::new(move |request: Request| service(&state, &core_ref, request))?;

        let core = Arc::new(Core {
            servicer,
            next_watch: AtomicU64::new(1),
        });
        core_cell
            .set(Arc::downgrade(&core))
            .unwrap_or_else(|_| unreachable!("the core cell is set exactly once, here"));

        Ok(Self { core, resident })
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
        lock(&self.resident).subscriptions.len()
    }

    /// The retry mode a subscription registered with, if it is still live.
    ///
    /// The mode is stated at registration and consumed by M5.3's fault protocol;
    /// this is what makes it observable in the meantime.
    #[must_use]
    pub fn retry_mode(&self, watch: WatchId) -> Option<RetryMode> {
        lock(&self.resident)
            .subscriptions
            .get(&watch)
            .map(|subscription| subscription.options().retry)
    }

    /// Whether a subscription is registered, established or not.
    #[must_use]
    pub fn is_registered(&self, watch: WatchId) -> bool {
        lock(&self.resident).subscriptions.contains_key(&watch)
    }

    /// Whether a subscription currently has a live watcher.
    ///
    /// Distinct from [`Monitor::is_registered`]: a subscription whose target
    /// cannot be opened yet is registered but not watching (D-14).
    #[must_use]
    pub fn is_watching(&self, watch: WatchId) -> bool {
        matches!(
            lock(&self.resident).subscriptions.get(&watch),
            Some(Subscription::Routed { .. })
        )
    }

    /// Why a subscription's watcher stopped *permanently*, if it has (D-22).
    /// A watcher that is merely recovering ([`Monitor::is_faulted`]) reports
    /// nothing here -- it is expected to resolve on its own (D-14).
    #[must_use]
    pub fn stop_reason(&self, watch: WatchId) -> Option<io::Error> {
        let resident = lock(&self.resident);
        let Some(Subscription::Routed { directory, .. }) = resident.subscriptions.get(&watch)
        else {
            return None;
        };
        resident
            .directories
            .get(directory)
            .and_then(DirectoryWatcher::stop_reason)
    }

    /// Whether a subscription's watcher is currently working to re-establish
    /// itself (D-31): asking, awaiting an answer, or backed off. `None` if the
    /// subscription is not registered. A still-`Pending` subscription always
    /// counts, whether this is its first attempt or a later one after an
    /// outage -- either way it is not currently delivering and the monitor is
    /// working to change that (D-14 is why neither case is ever terminal).
    #[must_use]
    pub fn is_faulted(&self, watch: WatchId) -> Option<bool> {
        let resident = lock(&self.resident);
        match resident.subscriptions.get(&watch)? {
            // Being Pending at all means at least one open attempt already
            // failed retryably (D-22) -- whether it is currently awaiting an
            // interactive answer or backed off on its timer, it is recovering.
            Subscription::Pending { .. } => Some(true),
            Subscription::Routed { directory, .. } => resident
                .directories
                .get(directory)
                .map(DirectoryWatcher::is_faulted),
        }
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
        let mut resident = lock(&self.resident);
        let directories = std::mem::take(&mut resident.directories);
        resident.subscriptions.clear();
        drop(resident);
        drop(directories);
    }

    /// How many distinct coalesced watchers are live, as opposed to how many
    /// subscriptions there are (D-6). Test-only: a client has no reason to
    /// distinguish the two, since coalescing is an implementation detail.
    #[cfg(test)]
    pub(crate) fn directory_count(&self) -> usize {
        lock(&self.resident).directories.len()
    }
}

/// Apply one request to the resident state.
///
/// Runs on the servicing path, one call at a time, so nothing here needs to
/// consider a concurrent mutation (D-2).
fn service(resident: &Mutex<Resident>, core_ref: &Arc<OnceLock<Weak<Core>>>, request: Request) {
    match request {
        Request::Subscribe {
            watch,
            path,
            options,
            sink,
            completion,
            fault_slot,
        } => {
            let outcome = subscribe(resident, core_ref, watch, path, options, sink, fault_slot);
            completion.send(Notification::Completion { watch, outcome });
        }
        Request::Cancel { watch, completion } => {
            // Removed under the lock, dropped outside it: dropping tears the
            // watcher down, which blocks on its callbacks, and holding the
            // resident lock across that wait would serialise teardown against
            // every other reader for no reason.
            let torn_down = {
                let mut state = lock(resident);
                let mut torn_down = None;
                if let Some(Subscription::Routed { directory, .. }) =
                    state.subscriptions.remove(&watch)
                    && let std::collections::hash_map::Entry::Occupied(entry) =
                        state.directories.entry(directory)
                    && entry.get().remove_route(watch) == 0
                {
                    torn_down = Some(entry.remove());
                }
                torn_down
            };
            drop(torn_down);

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
        Request::Retry { watch } => retry_pending(resident, core_ref, watch),
        Request::Answer { watch, delay } => answer(resident, watch, delay),
    }
}

/// Resolve a client's answer to an interactive fault question (D-27/M5.3),
/// wherever it currently lives -- a still-`Pending` subscription's own open
/// fault, or a routed subscription's coalesced watcher. A no-op if `watch` is
/// not currently awaiting one (already resolved, already cancelled, or never
/// asked).
fn answer(resident: &Mutex<Resident>, watch: WatchId, delay: Option<Duration>) {
    let mut state = lock(resident);
    if let Some(Subscription::Pending {
        awaiting_answer, ..
    }) = state.subscriptions.get(&watch)
        && *awaiting_answer
    {
        let resolved = delay.unwrap_or_else(|| FaultOperation::Open.default_delay());
        if let Some(Subscription::Pending {
            retry_timer,
            awaiting_answer,
            ..
        }) = state.subscriptions.get_mut(&watch)
        {
            *awaiting_answer = false;
            retry_timer.set_after(clamp(resolved));
        }
        return;
    }
    if let Some(Subscription::Routed { directory, .. }) = state.subscriptions.get(&watch)
        && let Some(watcher) = state.directories.get(directory)
    {
        watcher.answer(watch, delay);
    }
}

/// What opening a subscription's target found, and how it should route within
/// whatever was opened (D-7).
enum Opened {
    /// A live handle, plus how this subscription should route within it and the
    /// exact path that was opened (M5.1 needs this to reopen on re-establish;
    /// it is `path` for a directory target, `path`'s parent for a file one).
    Handle {
        handle: DirectoryHandle,
        scope: RouteScope,
        opened_path: PathBuf,
    },
    /// Retryable (D-22): nothing to watch yet.
    Pending,
    /// Permanent (D-22): nothing can ever be watched here.
    Failed(OpenFailure),
}

/// Try to open a subscription's target, resolving a file target to its parent
/// (D-7).
///
/// A subscription names a *path*; whether that path is a directory or a file is
/// not known in advance, only once the open is attempted. `NotADirectory` --
/// something exists at `path`, but it is not a directory -- is exactly the
/// signal that flips this from a directory subscription to a file one, since it
/// is D-22's classification for precisely that condition.
fn open_target(path: &std::path::Path, subtree: bool) -> Opened {
    match DirectoryHandle::open(path) {
        Ok(handle) => Opened::Handle {
            handle,
            scope: RouteScope::Directory { subtree },
            opened_path: path.to_path_buf(),
        },
        Err(error) if error.failure() == OpenFailure::NotADirectory => open_file_target(path),
        Err(error) if error.failure().is_retryable() => Opened::Pending,
        Err(error) => Opened::Failed(error.failure()),
    }
}

/// Open a file target's parent directory, filtered to the file's own leaf name.
///
/// Never recursive: a file is always a direct child of the directory that is
/// actually opened (D-7).
fn open_file_target(path: &std::path::Path) -> Opened {
    let (Some(parent), Some(leaf)) = (path.parent(), path.file_name()) else {
        // A path with no parent (a bare volume root) or no final component
        // cannot be a file target; report the failure that led here.
        return Opened::Failed(OpenFailure::NotADirectory);
    };
    match DirectoryHandle::open(parent) {
        Ok(handle) => Opened::Handle {
            handle,
            scope: RouteScope::File {
                leaf: Wtf16String::from_os_str(leaf),
            },
            opened_path: parent.to_path_buf(),
        },
        Err(error) if error.failure().is_retryable() => Opened::Pending,
        Err(error) => Opened::Failed(error.failure()),
    }
}

/// Build the timer that fires this subscription's next open retry (M5.1). The
/// callback submits `Request::Retry` through a `Weak<Core>`, resolved lazily
/// (see `Monitor::new`) because `Core` cannot exist until after the servicing
/// path that this timer's owner is registered on does.
fn make_retry_timer(
    core_ref: &Arc<OnceLock<Weak<Core>>>,
    watch: WatchId,
) -> io::Result<ThreadpoolTimer> {
    let core_ref = Arc::clone(core_ref);
    ThreadpoolTimer::new(
        move |_firing| {
            if let Some(core) = core_ref.get().and_then(Weak::upgrade) {
                let _ = core.submit(Request::Retry { watch });
            }
        },
        None,
    )
}

/// Park a subscription as `Pending` after a retryable open failure (D-22),
/// asking its interactive question (D-27) or arming the default-delay retry.
/// Used both for a subscription's very first attempt and for every later one
/// that is still retryable.
fn park_pending(
    resident: &Mutex<Resident>,
    core_ref: &Arc<OnceLock<Weak<Core>>>,
    watch: WatchId,
    path: PathBuf,
    options: WatchOptions,
    sink: Sender,
    fault_slot: Option<StandingSlot>,
) -> io::Result<()> {
    let retry_timer = make_retry_timer(core_ref, watch)?;
    let awaiting_answer = options.retry == RetryMode::Interactive && fault_slot.is_some();
    let mut state = lock(resident);
    state.subscriptions.insert(
        watch,
        Subscription::Pending {
            path,
            options,
            sink,
            retry_timer,
            fault_slot,
            awaiting_answer,
        },
    );
    // Arm or ask from the entry just inserted, under the same lock: no window
    // in which the timer could fire before the subscription it retries exists.
    if let Some(Subscription::Pending {
        retry_timer,
        fault_slot,
        ..
    }) = state.subscriptions.get(&watch)
    {
        if awaiting_answer {
            if let Some(slot) = fault_slot {
                slot.send(Notification::RetryQuestion {
                    watch,
                    operation: FaultOperation::Open,
                });
            }
        } else {
            retry_timer.set_after(clamp(FaultOperation::Open.default_delay()));
        }
    }
    Ok(())
}

/// Route a successfully opened target into a coalesced watcher (D-6), starting
/// one if this is the first subscription to reach this directory.
///
/// `Established` (D-13, opt-in) is sent whenever this succeeds, reporting
/// whichever tier the directory actually settled on (D-17) -- on the very
/// first establishment as well as a later one, matching its own documented
/// contract ("reported once at first establishment and again after every
/// re-establishment").
///
/// `original_path` and `core_ref` are needed only for the fallback: if the
/// watcher itself fails to arm despite a successful open (rare -- D-15's
/// rearm-and-retry case), this parks the subscription as `Pending` again
/// rather than losing it, and a fresh open retry needs the client's original
/// path (which may re-resolve differently, D-7) and a working retry timer.
#[allow(clippy::too_many_arguments)]
/// Routes a freshly opened handle to its watcher, coalescing with an existing
/// one for the same directory (D-6) or starting a new [`DirectoryWatcher`].
///
/// Returns whether the subscription is now fully routed to a live watcher
/// (`true`) or fell back to `Pending` because starting a *new* watcher failed
/// immediately after its directory opened (`false`, D-15's rearm-and-retry
/// case) -- the caller needs this to report the correct [`Outcome`]: reaching
/// this function at all means the open succeeded, but that is not the same as
/// the subscription being live.
fn route_established(
    resident: &Mutex<Resident>,
    core_ref: &Arc<OnceLock<Weak<Core>>>,
    watch: WatchId,
    original_path: PathBuf,
    handle: DirectoryHandle,
    scope: RouteScope,
    opened_path: PathBuf,
    options: WatchOptions,
    sink: Sender,
    fault_slot: Option<StandingSlot>,
) -> bool {
    let id = handle.identity();
    let route = Route {
        watch,
        scope,
        sink: sink.clone(),
        retry: options.retry,
        report_liveness: options.report_liveness,
        fault_slot,
    };
    let mut state = lock(resident);
    if let Some(watcher) = state.directories.get(&id) {
        // Coalesce (D-6): this directory already has a watcher. `handle` is
        // handed to it too, in case this route needs to widen the reach to
        // recursive (M4.4) -- reopening is what that takes, and this handle is
        // exactly what a reopen needs, so nothing further is opened for it.
        watcher.add_route(route, handle);
        let mode = watcher.mode();
        state.subscriptions.insert(
            watch,
            Subscription::Routed {
                directory: id,
                options,
            },
        );
        drop(state);
        if options.report_liveness {
            let _ = sink.send(Notification::Established { watch, mode });
        }
        return true;
    }

    match DirectoryWatcher::start(handle, opened_path, route) {
        Ok(watcher) => {
            let mode = watcher.mode();
            state.directories.insert(id, watcher);
            state.subscriptions.insert(
                watch,
                Subscription::Routed {
                    directory: id,
                    options,
                },
            );
            drop(state);
            if options.report_liveness {
                let _ = sink.send(Notification::Established { watch, mode });
            }
            true
        }
        Err(_) => {
            // Arming failed against a directory that just opened -- D-15's
            // rearm-and-retry classification, not fatal. The route (and any
            // fault_slot it carried) was already consumed by the failed start
            // attempt, so this fallback park has no standing slot to reuse; it
            // retries at the default delay regardless of `options.retry` until
            // a future attempt succeeds. This is a rare edge (an open that just
            // succeeded, immediately followed by an arm failure) and is a
            // recorded, deliberate simplification rather than engineering a
            // slot hand-back for it.
            drop(state);
            let _ = park_pending(
                resident,
                core_ref,
                watch,
                original_path,
                options,
                sink,
                None,
            );
            false
        }
    }
}

/// Re-attempt establishing a still-`Pending` subscription (M5.1), after its
/// retry timer fired. A no-op if the subscription is no longer `Pending`
/// (already routed by some other path, or already cancelled).
fn retry_pending(resident: &Mutex<Resident>, core_ref: &Arc<OnceLock<Weak<Core>>>, watch: WatchId) {
    let taken = {
        let mut state = lock(resident);
        match state.subscriptions.remove(&watch) {
            Some(Subscription::Pending {
                path,
                options,
                sink,
                retry_timer,
                fault_slot,
                ..
            }) => Some((path, options, sink, retry_timer, fault_slot)),
            Some(other) => {
                state.subscriptions.insert(watch, other);
                None
            }
            None => None,
        }
    };
    let Some((path, options, sink, retry_timer, fault_slot)) = taken else {
        return;
    };

    match open_target(&path, options.subtree) {
        Opened::Pending => {
            let awaiting_answer = options.retry == RetryMode::Interactive && fault_slot.is_some();
            let mut state = lock(resident);
            state.subscriptions.insert(
                watch,
                Subscription::Pending {
                    path,
                    options,
                    sink,
                    retry_timer,
                    fault_slot,
                    awaiting_answer,
                },
            );
            if let Some(Subscription::Pending {
                retry_timer,
                fault_slot,
                ..
            }) = state.subscriptions.get(&watch)
            {
                if awaiting_answer {
                    if let Some(slot) = fault_slot {
                        slot.send(Notification::RetryQuestion {
                            watch,
                            operation: FaultOperation::Open,
                        });
                    }
                } else {
                    retry_timer.set_after(clamp(FaultOperation::Open.default_delay()));
                }
            }
        }
        Opened::Failed(failure) => {
            // Permanent, discovered only on a later retry rather than at
            // registration: this is genuinely rare (the target existed
            // retryably, then became permanently unwatchable), and unlike the
            // very first Completion this report is best-effort rather than
            // reservation-backed -- the registration reservation was already
            // spent on `Outcome::Establishing`. `retry_timer` and `fault_slot`
            // are dropped here along with the subscription, which is already
            // removed from `resident`.
            let _ = sink.send(Notification::Completion {
                watch,
                outcome: Outcome::Failed { failure },
            });
        }
        Opened::Handle {
            handle,
            scope,
            opened_path,
        } => {
            // Background retry path: no synchronous caller is waiting on an
            // `Outcome` here, so whether this fully routed or fell back to
            // `Pending` again is not this call site's concern.
            let _ = route_established(
                resident,
                core_ref,
                watch,
                path,
                handle,
                scope,
                opened_path,
                options,
                sink,
                fault_slot,
            );
        }
    }
}

/// Register one subscription, reporting what became of it.
fn subscribe(
    resident: &Mutex<Resident>,
    core_ref: &Arc<OnceLock<Weak<Core>>>,
    watch: WatchId,
    path: PathBuf,
    options: WatchOptions,
    sink: Sender,
    fault_slot: Option<StandingSlot>,
) -> Outcome {
    match open_target(&path, options.subtree) {
        Opened::Pending => {
            let _ = park_pending(resident, core_ref, watch, path, options, sink, fault_slot);
            Outcome::Establishing
        }
        Opened::Failed(failure) => Outcome::Failed { failure },
        Opened::Handle {
            handle,
            scope,
            opened_path,
        } => {
            let routed = route_established(
                resident,
                core_ref,
                watch,
                path,
                handle,
                scope,
                opened_path,
                options,
                sink,
                fault_slot,
            );
            // A failed start immediately after opening falls back to
            // `Pending` (D-15's rearm-and-retry case) rather than routing to
            // a live watcher, so the client must be told it is still
            // establishing, not that it is already subscribed.
            if routed {
                Outcome::Subscribed
            } else {
                Outcome::Establishing
            }
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
