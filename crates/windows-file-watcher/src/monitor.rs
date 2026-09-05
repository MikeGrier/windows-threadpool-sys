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

use std::collections::HashMap;
use std::io;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use wtf_string::Wtf16String;

use windows_sys::Win32::Foundation::ERROR_DIRECTORY;
use windows_threadpool_sys::timer::ThreadpoolTimer;

use crate::directory::{DirectoryHandle, DirectoryId, FaultDetail, OpenFailure, classify_detail};
use crate::queue::{
    DEFAULT_BOUND, Notification, Outcome, Receiver, Reservation, Sender, StandingSlot, WatchId,
    channel_with_bound,
};
use crate::retry::{FaultOperation, clamp};
use crate::route::{Route, RouteScope};
use crate::servicing::{Rejected, Servicer};
use crate::session::Session;
use crate::watch::{RetryMode, VolumeChangeDecision, WatchOptions};
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
        /// `on_volume_change == Confirm`). `report_liveness` alone never
        /// creates one: `Suspended`/`Resumed`/`Established` use best-effort
        /// sends and reserve nothing.
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
    /// A client's answer to a volume-change question (D-78/M12), submitted
    /// through
    /// [`Session::answer_volume_change`](crate::session::Session::answer_volume_change).
    /// Carries no completion, for the same reason `Answer` does not; ignored
    /// if `watch` is not currently awaiting one.
    AnswerVolumeChange {
        /// The subscription answering.
        watch: WatchId,
        /// Whether to keep this subscription running against the new volume.
        decision: VolumeChangeDecision,
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
            Request::AnswerVolumeChange { watch, decision } => f
                .debug_struct("AnswerVolumeChange")
                .field("watch", watch)
                .field("decision", decision)
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
        /// A reservation for the permanent `Failed` completion a later retry
        /// may discover (D-22), carved out when this subscription first became
        /// `Pending` if the queue had room. Present or not, it rides through
        /// every re-park this subscription goes through until it either
        /// establishes (dropped, releasing the slot) or is spent on that
        /// completion -- a terminal outcome must not be lost to best-effort
        /// backpressure the way an ordinary batch can be.
        terminal: Option<Reservation>,
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
/// when teardown races a drain. `pub(crate)` (rather than private) only so a
/// coalesced watcher's own background reopen can name `Weak<Mutex<Resident>>`
/// as a field type and call [`rekey`] (M11.4); its fields stay private, so
/// only this module ever mutates them.
#[derive(Default)]
pub(crate) struct Resident {
    /// Every subscription, keyed by the identifier that tags its notifications.
    subscriptions: HashMap<WatchId, Subscription>,
    /// Every live coalesced watcher, keyed by directory identity (D-6): a
    /// directory is watched once regardless of how many subscriptions target
    /// entries within it.
    directories: HashMap<DirectoryId, DirectoryWatcher>,
}

/// Re-key a coalesced watcher's entry after a path-based reopen fallback
/// landed on a different `DirectoryId` than before (M11.4) -- previously
/// fixed at first insertion and never updated, which would leave a later new
/// subscription to the same path unable to find this watcher (it would look
/// up the new id, find nothing, and spin up a redundant second watcher) and
/// every existing subscription's own stored `directory` stale (breaking
/// `Cancel`/`answer`'s own lookup by that id). A no-op if `old` is no longer
/// present (the watcher was torn down and removed between the reopen and this
/// call) or if `old == new` (a `ReOpenFile` success never reaches here at
/// all, since it cannot change identity -- see `WatcherInner::retry_reestablish`).
///
/// `new` may already be occupied -- a path-based reopen can land on an
/// identity another watcher already owns (for example, the reopened path was
/// replaced by a junction into an already-watched directory, PR #20 review
/// response). Silently `insert`-ing over that entry would drop the existing
/// watcher (and every route it still serves) with nothing to migrate them:
/// `Cancel`/`answer` would still find their subscriptions' `directory`
/// pointing at `new`, but the map entry there is now a different watcher that
/// never received their routes. So the existing entry's routes are migrated
/// onto the just-rekeyed watcher instead, and the existing entry is discarded
/// -- never the other way around: `watcher` is the caller's own `self`,
/// executing this from inside its own retry-timer callback, and dropping it
/// here (via `DirectoryWatcher::stop`) would deadlock waiting on that same
/// callback to finish. A route that cannot be migrated -- reopening the
/// directory races with it disappearing -- is not silently dropped either
/// (PR #20 review response): its subscription is torn down with a terminal
/// `Completion { Failed }`, the same way any other asynchronously discovered
/// permanent open failure is reported, rather than left pointing at a route
/// that no watcher holds.
///
/// The discarded existing entry is dropped only after the resident lock is
/// released (PR #20 review response): its `Drop` calls `stop_and_drain` on
/// its retry timer, which blocks waiting for that timer's own callback to
/// finish -- and that callback, if it is concurrently blocked trying to
/// re-enter `rekey` (or anything else that needs this same resident lock),
/// would then wait on this thread forever while this thread waited on it.
pub(crate) fn rekey(resident: &Mutex<Resident>, old: DirectoryId, new: DirectoryId) {
    if old == new {
        return;
    }
    let mut state = lock(resident);
    let Some(watcher) = state.directories.remove(&old) else {
        return;
    };
    let existing = state.directories.remove(&new);
    if let Some(existing) = &existing {
        for route in existing.take_routes() {
            match DirectoryHandle::open(watcher.path()) {
                Ok(handle) => watcher.add_route(route, handle),
                Err(error) => {
                    // A route that cannot be migrated must not be silently
                    // dropped (PR #20 review response): its
                    // `Subscription::Routed` entry would otherwise keep
                    // reporting as registered while no watcher anywhere
                    // holds its route, so it can never receive another
                    // notification and a later `Cancel` would find nothing
                    // to remove. Reported the same way any other
                    // asynchronously discovered permanent open failure is
                    // (see `Opened::Failed`'s handling above): a terminal
                    // `Completion { Failed }`, best-effort since there is no
                    // reservation carved out for it here, and the stale
                    // subscription entry is removed so the client can
                    // resubscribe cleanly if it wants to keep watching.
                    log::warn!(
                        "windows-file-watcher: could not migrate a route from a redundant \
                         watcher during identity-collision coalescing: {error}"
                    );
                    let detail = error.detail();
                    let _ = route.sink.send(Notification::Completion {
                        watch: route.watch,
                        outcome: Outcome::Failed { detail },
                    });
                    state.subscriptions.remove(&route.watch);
                }
            }
        }
    }
    state.directories.insert(new, watcher);
    for subscription in state.subscriptions.values_mut() {
        if let Subscription::Routed { directory, .. } = subscription
            && *directory == old
        {
            *directory = new;
        }
    }
    drop(state);
    // Tears the redundant watcher down, if there was one: safe out here, but
    // not under the resident lock above (see the doc comment).
    drop(existing);
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
        // own servicer would create. `resident_weak` is handed to every newly
        // constructed coalesced watcher (M11.4), so its own background reopen
        // can re-key its `Resident.directories` entry without a strong cycle
        // back to the map it lives in.
        let state = Arc::clone(&resident);
        let resident_weak = Arc::downgrade(&resident);
        let core_ref = Arc::clone(&core_cell);
        let servicer = Servicer::new(move |request: Request| {
            service(&state, &resident_weak, &core_ref, request)
        })?;

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
    /// cannot be opened yet is registered but not watching (D-14). Also false
    /// once the routed watcher has stopped permanently (D-22) -- being
    /// `Subscription::Routed` alone is not proof of liveness, since a stopped
    /// watcher stays routed until its subscription is cancelled.
    #[must_use]
    pub fn is_watching(&self, watch: WatchId) -> bool {
        let resident = lock(&self.resident);
        let Some(Subscription::Routed { directory, .. }) = resident.subscriptions.get(&watch)
        else {
            return false;
        };
        resident
            .directories
            .get(directory)
            .is_some_and(|watcher| watcher.stop_reason().is_none())
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

    /// The classification and raw code behind a subscription's current fault,
    /// if any (D-79) -- the same detail an interactive route was, or would
    /// have been, asked about. `None` if the subscription is not registered,
    /// still `Pending` (D-22's initial open retry loop tracks no persistent
    /// detail of its own), or not currently faulted.
    #[must_use]
    pub fn fault_detail(&self, watch: WatchId) -> Option<FaultDetail> {
        let resident = lock(&self.resident);
        let Some(Subscription::Routed { directory, .. }) = resident.subscriptions.get(&watch)
        else {
            return None;
        };
        resident
            .directories
            .get(directory)
            .and_then(DirectoryWatcher::fault_detail)
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
fn service(
    resident: &Mutex<Resident>,
    resident_weak: &Weak<Mutex<Resident>>,
    core_ref: &Arc<OnceLock<Weak<Core>>>,
    request: Request,
) {
    match request {
        Request::Subscribe {
            watch,
            path,
            options,
            sink,
            completion,
            fault_slot,
        } => {
            let outcome = subscribe(
                resident,
                resident_weak,
                core_ref,
                watch,
                path,
                options,
                sink,
                fault_slot,
            );
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
                    && let Some((remaining, stopped)) = state
                        .directories
                        .get(&directory)
                        .map(|watcher| watcher.remove_route(watch))
                {
                    // A volume-change resolution this removal triggered may
                    // have decided `Stop` for other routes on the same
                    // watcher too (PR #20 review response); their
                    // subscriptions are now just as stale as this one's,
                    // already removed above.
                    for extra in stopped {
                        state.subscriptions.remove(&extra);
                    }
                    if remaining == 0 {
                        torn_down = state.directories.remove(&directory);
                    }
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
        Request::Retry { watch } => retry_pending(resident, resident_weak, core_ref, watch),
        Request::Answer { watch, delay } => answer(resident, watch, delay),
        Request::AnswerVolumeChange { watch, decision } => {
            answer_volume_change(resident, watch, decision);
        }
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

/// Resolve a client's answer to a volume-change question (D-78/M12),
/// wherever it currently lives -- always a routed subscription's coalesced
/// watcher, since a volume-change question is only ever raised by a
/// path-based reopen of an already-established watcher. A no-op if `watch`
/// is not currently awaiting one (already resolved, already cancelled, or
/// never asked).
fn answer_volume_change(
    resident: &Mutex<Resident>,
    watch: WatchId,
    decision: VolumeChangeDecision,
) {
    // Removed under the lock, dropped outside it, mirroring `Request::Cancel`:
    // dropping tears the watcher down, which blocks on its callbacks.
    let torn_down = {
        let mut state = lock(resident);
        let Some(Subscription::Routed { directory, .. }) = state.subscriptions.get(&watch) else {
            return;
        };
        let directory = *directory;
        let Some((remaining, stopped)) = state
            .directories
            .get(&directory)
            .and_then(|watcher| watcher.answer_volume_change(watch, decision))
        else {
            return;
        };
        // Every watch that decided `Stop` -- including `watch` itself, if
        // that was its own decision -- no longer has a route on this
        // watcher, so its `Resident.subscriptions` entry is stale and would
        // otherwise keep reporting as registered/routed forever (PR #20
        // review response).
        for stopped_watch in &stopped {
            state.subscriptions.remove(stopped_watch);
        }
        if remaining == 0 {
            state.directories.remove(&directory)
        } else {
            None
        }
    };
    drop(torn_down);
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
    Pending(FaultDetail),
    /// Permanent (D-22): nothing can ever be watched here.
    Failed(FaultDetail),
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
        Err(error) if error.failure().is_retryable() => Opened::Pending(error.detail()),
        Err(error) => Opened::Failed(error.detail()),
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
        return Opened::Failed(FaultDetail::synthetic(
            OpenFailure::NotADirectory,
            ERROR_DIRECTORY,
        ));
    };
    // A bare relative leaf (`target.txt`) has an empty `parent()`, which
    // `CreateFileW` rejects outright -- normalize it to `.` (the current
    // directory) so this target actually opens instead of spuriously
    // reporting a permanent or retryable failure that has nothing to do with
    // the target itself.
    let parent = if parent.as_os_str().is_empty() {
        std::path::Path::new(".")
    } else {
        parent
    };
    match DirectoryHandle::open(parent) {
        Ok(handle) => Opened::Handle {
            handle,
            scope: RouteScope::File {
                leaf: Wtf16String::from_os_str(leaf),
            },
            opened_path: parent.to_path_buf(),
        },
        Err(error) if error.failure().is_retryable() => Opened::Pending(error.detail()),
        Err(error) => Opened::Failed(error.detail()),
    }
}

/// Whether an open-class fault must wait for this route's interactive answer.
fn awaits_open_answer(retry: RetryMode, has_fault_slot: bool) -> bool {
    retry == RetryMode::Interactive && has_fault_slot
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
/// that is still retryable. `detail` is the classified failure that made this
/// attempt retryable (D-79), carried through to the interactive question.
#[allow(clippy::too_many_arguments)]
fn park_pending(
    resident: &Mutex<Resident>,
    core_ref: &Arc<OnceLock<Weak<Core>>>,
    watch: WatchId,
    path: PathBuf,
    options: WatchOptions,
    sink: Sender,
    fault_slot: Option<StandingSlot>,
    detail: FaultDetail,
) -> io::Result<()> {
    let retry_timer = make_retry_timer(core_ref, watch)?;
    // Best-effort: if the queue has no room to spare right now, this
    // subscription's eventual permanent failure (rare) falls back to the
    // ordinary best-effort path rather than blocking registration on it.
    let terminal = sink.reserve();
    let awaiting_answer = awaits_open_answer(options.retry, fault_slot.is_some());
    let mut state = lock(resident);
    state.subscriptions.insert(
        watch,
        Subscription::Pending {
            path,
            options,
            sink,
            retry_timer,
            fault_slot,
            terminal,
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
                    detail,
                });
            }
        } else {
            retry_timer.set_after(clamp(FaultOperation::Open.default_delay()));
        }
    }
    Ok(())
}

/// Whether a settled-tier notification accurately describes this route now.
fn should_report_established(report_liveness: bool, is_faulted: bool) -> bool {
    report_liveness && !is_faulted
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
fn route_established(
    resident: &Mutex<Resident>,
    resident_weak: &Weak<Mutex<Resident>>,
    core_ref: &Arc<OnceLock<Weak<Core>>>,
    watch: WatchId,
    original_path: PathBuf,
    handle: DirectoryHandle,
    scope: RouteScope,
    opened_path: PathBuf,
    options: WatchOptions,
    sink: Sender,
    fault_slot: Option<StandingSlot>,
) -> Routed {
    let id = handle.identity();
    let route = Route {
        watch,
        scope,
        sink: sink.clone(),
        retry: options.retry,
        report_liveness: options.report_liveness,
        on_volume_change: options.on_volume_change,
        fault_slot,
    };
    let mut state = lock(resident);
    if let Some(watcher) = state
        .directories
        .get(&id)
        .filter(|watcher| watcher.stop_reason().is_none())
    {
        // Coalesce (D-6): this directory already has a watcher. `handle` is
        // handed to it too, in case this route needs to widen the reach to
        // recursive (M4.4) -- reopening is what that takes, and this handle is
        // exactly what a reopen needs, so nothing further is opened for it.
        watcher.add_route(route, handle);
        let mode = watcher.mode();
        // Read before `state.subscriptions.insert` below, which needs `state`
        // mutably while `watcher` still borrows it immutably.
        let is_faulted = watcher.is_faulted();
        state.subscriptions.insert(
            watch,
            Subscription::Routed {
                directory: id,
                options,
            },
        );
        drop(state);
        // Suppressed while the watcher is faulted (PR #20 review response):
        // either it was already recovering before this route joined, or
        // `add_route`'s own widen-to-recursive reopen just failed and entered
        // the fault loop -- either way there is no settled tier to report
        // right now, and asserting one would contradict `Established`'s
        // documented contract ("whichever tier the directory actually
        // settled on"). This route is still added to `routes` either way, so
        // it already received (or is about to receive) the ordinary
        // `Suspended`/`RetryQuestion` fault notifications, and will get its
        // own `Resumed`/`Established` from `resolve_fault_success` once (if)
        // the watcher recovers.
        if should_report_established(options.report_liveness, is_faulted) {
            let _ = sink.send(Notification::Established { watch, mode });
        }
        return Routed::Live;
    }

    // Either nothing is watching this directory yet, or what remains there
    // stopped permanently (D-22) and can never arm again -- discard it before
    // starting fresh from this route's own handle, rather than silently
    // reporting a live subscription against a watcher that will never
    // deliver. Removed under the lock, dropped outside it: `Drop` blocks on
    // rundown (D-20), and holding the resident lock across that wait would
    // serialise every other reader against a stopped watcher for no reason
    // (the same ordering `Request::Cancel` already uses).
    let stale = state.directories.remove(&id);
    drop(state);
    drop(stale);

    match DirectoryWatcher::start(handle, opened_path, route) {
        Ok(watcher) => {
            // M11.4: so this watcher's own background reopen can re-key its
            // `Resident.directories` entry if a path-based fallback ever
            // lands on a different directory.
            watcher.bind_resident(Weak::clone(resident_weak));
            let mode = watcher.mode();
            let mut state = lock(resident);
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
            Routed::Live
        }
        Err((error, route)) => {
            // Arming failed against a directory that just opened -- D-15's
            // rearm-and-retry classification, not fatal. `start` hands the
            // route back on failure, so its standing fault-question
            // reservation (if any) is reused here rather than silently
            // downgrading an `Interactive` subscription to default retries.
            let detail = classify_detail(&error);
            match park_pending(
                resident,
                core_ref,
                watch,
                original_path,
                options,
                sink,
                route.fault_slot,
                detail,
            ) {
                Ok(()) => Routed::Parked,
                Err(error) => Routed::ParkFailed(FaultDetail::retry_unavailable(&error)),
            }
        }
    }
}

/// What became of routing a freshly opened handle into a watcher (see
/// [`route_established`]).
enum Routed {
    /// Coalesced onto an existing watcher, or a new one started and armed: a
    /// live watcher now backs this subscription.
    Live,
    /// Starting a *new* watcher failed immediately after its directory opened
    /// (D-15's rearm-and-retry case); the subscription was re-parked instead.
    Parked,
    /// Even the fallback park could not be set up (its retry timer failed to
    /// be created -- vanishingly rare resource exhaustion); nothing was
    /// registered for this subscription.
    ParkFailed(FaultDetail),
}

/// Re-attempt establishing a still-`Pending` subscription (M5.1), after its
/// retry timer fired. A no-op if the subscription is no longer `Pending`
/// (already routed by some other path, or already cancelled).
fn retry_pending(
    resident: &Mutex<Resident>,
    resident_weak: &Weak<Mutex<Resident>>,
    core_ref: &Arc<OnceLock<Weak<Core>>>,
    watch: WatchId,
) {
    let taken = {
        let mut state = lock(resident);
        match state.subscriptions.remove(&watch) {
            Some(Subscription::Pending {
                path,
                options,
                sink,
                retry_timer,
                fault_slot,
                terminal,
                ..
            }) => Some((path, options, sink, retry_timer, fault_slot, terminal)),
            Some(other) => {
                state.subscriptions.insert(watch, other);
                None
            }
            None => None,
        }
    };
    let Some((path, options, sink, retry_timer, fault_slot, terminal)) = taken else {
        return;
    };

    match open_target(&path, options.subtree) {
        Opened::Pending(detail) => {
            let awaiting_answer = awaits_open_answer(options.retry, fault_slot.is_some());
            let mut state = lock(resident);
            state.subscriptions.insert(
                watch,
                Subscription::Pending {
                    path,
                    options,
                    sink,
                    retry_timer,
                    fault_slot,
                    terminal,
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
                            detail,
                        });
                    }
                } else {
                    retry_timer.set_after(clamp(FaultOperation::Open.default_delay()));
                }
            }
        }
        Opened::Failed(detail) => {
            // Permanent, discovered only on a later retry rather than at
            // registration: genuinely rare (the target existed retryably, then
            // became permanently unwatchable), but still a terminal outcome the
            // client must not silently lose to backpressure -- delivered
            // through `terminal`'s reservation when park_pending managed to
            // carve one out, falling back to best-effort only if it could not.
            // `retry_timer` and `fault_slot` are dropped here along with the
            // subscription, which is already removed from `resident`.
            let outcome = Outcome::Failed { detail };
            match terminal {
                Some(reservation) => reservation.send(Notification::Completion { watch, outcome }),
                None => {
                    let _ = sink.send(Notification::Completion { watch, outcome });
                }
            }
        }
        Opened::Handle {
            handle,
            scope,
            opened_path,
        } => {
            // Background retry path: no synchronous caller is waiting on an
            // `Outcome` here, but a `ParkFailed` fallback is still a terminal
            // outcome the client was never told about otherwise, so report it
            // (best-effort -- `terminal` was already consumed or never taken
            // for this attempt).
            let outcome_sink = sink.clone();
            match route_established(
                resident,
                resident_weak,
                core_ref,
                watch,
                path,
                handle,
                scope,
                opened_path,
                options,
                sink,
                fault_slot,
            ) {
                Routed::Live | Routed::Parked => {}
                Routed::ParkFailed(detail) => {
                    let _ = outcome_sink.send(Notification::Completion {
                        watch,
                        outcome: Outcome::Failed { detail },
                    });
                }
            }
        }
    }
}

/// Register one subscription, reporting what became of it.
#[allow(clippy::too_many_arguments)]
fn subscribe(
    resident: &Mutex<Resident>,
    resident_weak: &Weak<Mutex<Resident>>,
    core_ref: &Arc<OnceLock<Weak<Core>>>,
    watch: WatchId,
    path: PathBuf,
    options: WatchOptions,
    sink: Sender,
    fault_slot: Option<StandingSlot>,
) -> Outcome {
    match open_target(&path, options.subtree) {
        Opened::Pending(detail) => {
            match park_pending(
                resident, core_ref, watch, path, options, sink, fault_slot, detail,
            ) {
                Ok(()) => Outcome::Establishing,
                Err(error) => Outcome::Failed {
                    detail: FaultDetail::retry_unavailable(&error),
                },
            }
        }
        Opened::Failed(detail) => Outcome::Failed { detail },
        Opened::Handle {
            handle,
            scope,
            opened_path,
        } => {
            // A failed start immediately after opening falls back to
            // `Pending` (D-15's rearm-and-retry case) rather than routing to
            // a live watcher, so the client must be told it is still
            // establishing, not that it is already subscribed; a `ParkFailed`
            // fallback means neither happened, so the client is told this
            // registration simply failed.
            match route_established(
                resident,
                resident_weak,
                core_ref,
                watch,
                path,
                handle,
                scope,
                opened_path,
                options,
                sink,
                fault_slot,
            ) {
                Routed::Live => Outcome::Subscribed,
                Routed::Parked => Outcome::Establishing,
                Routed::ParkFailed(detail) => Outcome::Failed { detail },
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
