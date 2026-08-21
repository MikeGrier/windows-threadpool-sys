// Copyright (c) 2026 Mike Grier
//! The crate-owned notification queue: where a watcher puts what it decodes, and
//! where a client takes it from.
//!
//! The queue is the boundary that keeps client behaviour off this crate's cadence
//! path. Delivery is an enqueue *the crate performs*; the client only ever
//! receives. There is no sink trait and no client-supplied closure, so nothing a
//! client does -- blocking, panicking, being slow -- can stall or unwind a
//! completion callback (D-2/D-11).
//!
//! Note this is a statement about the **call graph**, not about threads. Which
//! thread a client drains on is entirely its own business, and draining from its
//! own thread-pool callback is an expected integration; that is the client's pool
//! object and the client's cadence. The M3.4 doorbell (D-25) exists to make
//! precisely that integration possible without dedicating a thread.
//!
//! # Reserved is guaranteed, unreserved is best-effort
//!
//! The queue is bounded, so it can fill, and the two kinds of thing that travel
//! it cannot survive that the same way. A lost change notification is recoverable
//! -- re-scanning is exactly what [`DesyncCause`] exists to ask for -- while a
//! lost completion or fault report is a liveness bug, because the client waits
//! forever for something that already happened.
//!
//! Rather than sort that out per message type at the enqueue, reliability is a
//! property of **reserved capacity** (D-33). A control message takes its slot
//! through [`Sender::reserve`] *before* whatever produces it is allowed to
//! proceed, so [`Reservation::send`] cannot fail -- the room is already the
//! sender's. Change notifications reserve nothing and go through
//! [`Sender::send`], which is allowed to fail. One line covers both: *reserved is
//! guaranteed, unreserved is best-effort.*
//!
//! # Reporting a full queue without needing room to do it
//!
//! A desync is an ordinary notification and rides the queue in order like any
//! other (D-12/D-26), which is what lets a client say "everything before this is
//! accounted for". That breaks down in exactly one place: reporting that the
//! queue is full cannot itself require a slot in the full queue, and reserving
//! one only defers the problem to the second overflow.
//!
//! So a failed enqueue latches the affected [`WatchId`] in a set held *outside*
//! the bounded queue, where it coalesces -- lossless, because a desync is
//! idempotent and the answer is always a re-scan. The latch is drained back into
//! the queue at the next successful enqueue, which is also the position the loss
//! actually occupies: the queue was full when the loss happened, so everything
//! still queued precedes it and everything enqueued afterwards follows it.
//! A receiver that reaches an empty queue synthesises any remainder directly, so
//! a latched desync is delivered even if nothing further is ever sent.
//!
//! # The doorbell
//!
//! A queue drainable only by a blocking [`Receiver::recv`] would force a client
//! that already owns a thread pool to dedicate a thread to it -- contradicting
//! this crate's premise that nobody should have to own threads. So the receiver
//! hands out a manual-reset event ([`Receiver::doorbell`]) that a client can wait
//! on however it likes, including from its own `ThreadpoolWait`.
//!
//! It is created **lazily**, so a client that only ever calls `recv` allocates no
//! kernel object at all.
//!
//! *Why not a `Doorbell` trait the client implements?* Because that would be a
//! client callback on this crate's cadence path, which is the one thing D-2
//! forbids, and it would have made `Monitor`, `Session`, and `Sender` all generic
//! over it. The composition argument for a trait does not survive contact with
//! the platform either: on Windows a HANDLE **is** the universal waitable
//! currency -- `WaitForSingleObject`, `WaitForMultipleObjects`,
//! `MsgWaitForMultipleObjects`, `ThreadpoolWait` and alertable waits all take one
//! -- so an event is the native composition point rather than a lowest common
//! denominator. The single case it does not reach, an async `Waker`, is a short
//! bridge the client writes on its own pool, which is where that code belongs.
//! Owning the doorbell also makes the reset discipline an internal invariant
//! rather than a client obligation (D-25).
//!
//! That invariant is one line: **the event is signalled exactly when the receiver
//! has something to observe**, which is a queued notification, an owed loss
//! report, or the end of the stream. It is re-established under the queue lock at
//! the end of every mutation -- the same lock a receiver holds while deciding
//! there is nothing to take -- so a wakeup cannot be lost in the gap between
//! those two decisions, because there is no gap.
//!
//! This is the interim, entirely in-crate endpoint for M2. The session/receiver
//! split, the bounded overflow policy with its latched `Desync { QueueFull }`,
//! and the doorbell all land in M3.

// The queue reaches a client through `Session`, but its reservation half has no
// production producer until M3.6 issues request completions and M5 issues fault
// reports, so part of this surface reads as dead under default features. Remove
// this when those land.
#![allow(dead_code)]

use std::collections::VecDeque;
use std::io;
use std::num::NonZeroUsize;
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle};
use std::ptr;
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle, FALSE, TRUE};
use windows_sys::Win32::System::Threading::{
    CreateEventW, GetCurrentProcess, ResetEvent, SetEvent,
};

use crate::directory::OpenFailure;
use crate::notify::{Change, DesyncCause};

/// The bound used when a caller does not choose one.
///
/// Counts **notifications**, not changes: one decoded completion is one batch
/// (D-10) and a batch can carry hundreds of records, so this is a far deeper
/// backlog than the number suggests. It is chosen to be ample for a client that
/// drains promptly and still bounded for one that stops.
pub const DEFAULT_BOUND: NonZeroUsize = match NonZeroUsize::new(1024) {
    Some(bound) => bound,
    None => unreachable!(),
};

/// Identifies the subscription a notification belongs to.
///
/// `Copy`, so a client can retain it to route or aggregate without holding the
/// subscription's lifecycle object (D-5). M3.4 issues these from the monitor;
/// until then a watcher is constructed with one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WatchId(u64);

impl WatchId {
    /// Build an identifier from a raw value.
    ///
    /// M3.4 replaces this with monitor-issued identifiers; it exists so M2 can
    /// tag records before the monitor exists.
    #[must_use]
    pub fn from_raw(value: u64) -> Self {
        Self(value)
    }

    /// The raw value, for a client that wants to key a map on it.
    #[must_use]
    pub fn get(self) -> u64 {
        self.0
    }
}

/// How a request turned out.
///
/// Registration is asynchronous (D-2), so the outcome cannot be the return value
/// of the call that made the request; it arrives here instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Outcome {
    /// The subscription is registered and the monitor is watching for it.
    Subscribed,
    /// The subscription is registered, but the target could not be opened yet.
    ///
    /// Retryable in D-22's sense -- a path that does not exist yet, a network
    /// share that is briefly unreachable -- so the monitor keeps the
    /// subscription and will establish it when it can. This is not a failure and
    /// carries no terminal meaning (D-14).
    Establishing,
    /// The target can never be watched, so nothing was registered.
    ///
    /// Only D-22's permanent pair reaches here, and both mean the caller named
    /// something that is not a watchable directory rather than that anything went
    /// wrong in the environment. Retrying would spin forever against input that
    /// will never become valid.
    Failed {
        /// Which permanent failure it was.
        failure: OpenFailure,
    },
    /// The subscription has ended and its watcher is released.
    ///
    /// Its position in the stream is the guarantee: everything before it belongs
    /// to the live watch, and nothing after it does (D-30).
    Cancelled,
}

/// One item a client receives, always tagged with the subscription it belongs to.
///
/// Changes and desyncs ride the same queue so their order relative to one another
/// is well defined within a subscription (D-12): a client that sees a `Desync`
/// knows every change enqueued before it, and none after, is accounted for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Notification {
    /// The changes one completion carried, in the order the kernel reported them.
    Batch {
        /// The subscription these changes belong to.
        watch: WatchId,
        /// The changes, in kernel order.
        changes: Vec<Change>,
    },
    /// Changes were lost; the client should re-scan (D-12).
    Desync {
        /// The subscription affected.
        watch: WatchId,
        /// How the gap arose. Advisory: the response is a re-scan either way.
        cause: DesyncCause,
    },
    /// A request the client made has been serviced (D-30).
    ///
    /// Carried on this queue rather than a side channel so that its ordering
    /// against data is **structural** rather than temporal.
    Completion {
        /// The subscription the request concerned.
        watch: WatchId,
        /// What happened.
        outcome: Outcome,
    },
}

impl Notification {
    /// The subscription this notification belongs to.
    #[must_use]
    pub fn watch(&self) -> WatchId {
        match self {
            Notification::Batch { watch, .. }
            | Notification::Desync { watch, .. }
            | Notification::Completion { watch, .. } => *watch,
        }
    }
}

/// The shared queue storage.
struct Shared {
    items: Mutex<State>,
    arrived: Condvar,
    /// The client-facing wake, created on first request and never replaced.
    ///
    /// Outside the lock so a borrow of it can outlive the guard, but only ever
    /// signalled or reset *under* the lock -- see [`refresh_doorbell`].
    doorbell: OnceLock<OwnedHandle>,
}

/// A crate-owned producer that stopped because the queue was full, and must be
/// prodded when there is room again.
///
/// This is **not** the client callback D-2 forbids and D-25 rejected: every
/// implementor is this crate's own code, and the contract is one line -- return
/// promptly, touch nothing of the queue's. Observation is the only tier that
/// needs it, because it is the only one that reserves nothing (D-33).
pub(crate) trait Resume: Send + Sync {
    /// There may be room now. Called with no lock of the queue's held.
    fn resume(&self);
}

struct State {
    queue: VecDeque<Notification>,
    /// The bound. Never zero, which is what makes the delivery guarantee
    /// non-vacuous: a queue with no room could never carry the desync that
    /// reports its own saturation.
    capacity: usize,
    /// Slots taken by an outstanding [`Reservation`] but not yet filled. Held
    /// away from the best-effort path, which is the whole mechanism: a reserved
    /// send cannot fail because nothing else can take its room.
    reserved: usize,
    /// Subscriptions with a loss that could not be enqueued, in the order they
    /// were first latched.
    ///
    /// Deliberately outside the bounded queue, and deliberately coalesced: a
    /// second loss for a subscription that already owes a desync adds nothing,
    /// because the client's response to one is the same as its response to ten.
    latched: VecDeque<WatchId>,
    /// Set when every sender is gone, so a blocked receiver can stop waiting
    /// rather than hang forever on a queue nothing can fill.
    senders: usize,
    /// Producers to prod when a saturated queue frees a slot.
    ///
    /// Weak, so a producer that has gone away is simply forgotten rather than
    /// kept alive by the queue it fed.
    resumers: Vec<Weak<dyn Resume>>,
}

impl State {
    /// Slots available to the best-effort path: neither occupied nor reserved.
    fn free(&self) -> usize {
        self.capacity - self.queue.len() - self.reserved
    }

    /// Whether a receiver has anything to observe.
    ///
    /// Disconnection counts: a client waiting on the doorbell must learn that the
    /// stream has ended, or it would wait for a notification nothing can send.
    fn pending(&self) -> bool {
        !self.queue.is_empty() || !self.latched.is_empty() || self.senders == 0
    }

    /// Move latched losses back into the queue, as many as there is room for.
    ///
    /// Called immediately before an enqueue, which is what puts each desync in
    /// the position the loss actually occupies -- see the module docs.
    fn flush_latched(&mut self) {
        while self.free() > 0 {
            let Some(watch) = self.latched.pop_front() else {
                break;
            };
            self.queue.push_back(Notification::Desync {
                watch,
                cause: DesyncCause::QueueFull,
            });
        }
    }

    /// Record that a subscription lost something, coalescing with any loss it is
    /// already owed.
    fn latch(&mut self, watch: WatchId) {
        if !self.latched.contains(&watch) {
            self.latched.push_back(watch);
        }
    }
}

/// What became of a best-effort notification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[must_use = "a latched notification was lost, which M3.7 responds to by not re-arming"]
pub enum Delivery {
    /// Enqueued. The client will receive it.
    Queued,
    /// The queue was full, so it was dropped and a `Desync { QueueFull }` is now
    /// owed to its subscription.
    Latched,
}

/// The crate-side half: enqueues, never blocks.
///
/// Cloneable and `Send + Sync` because several watchers -- whose completions run
/// on different pool threads -- feed one client queue (D-11).
pub struct Sender {
    shared: Arc<Shared>,
}

impl Clone for Sender {
    fn clone(&self) -> Self {
        let mut state = lock(&self.shared.items);
        state.senders += 1;
        drop(state);
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl Sender {
    /// Enqueue one notification, best-effort.
    ///
    /// Runs on the cadence path, so it never blocks. It reserves nothing, so it
    /// *can* fail: a full queue drops the notification and latches a
    /// `Desync { QueueFull }` against its subscription (D-29/D-33). Use
    /// [`Sender::reserve`] for anything whose loss a client cannot recover from.
    pub fn send(&self, notification: Notification) -> Delivery {
        let watch = notification.watch();
        let delivery = {
            let mut state = lock(&self.shared.items);
            // Before the new item, so a loss is reported at the point it happened
            // rather than after changes that preceded it.
            state.flush_latched();
            let delivery = if state.free() > 0 {
                state.queue.push_back(notification);
                Delivery::Queued
            } else {
                state.latch(watch);
                Delivery::Latched
            };
            refresh_doorbell(&self.shared, &state);
            delivery
        };
        self.shared.arrived.notify_all();
        delivery
    }

    /// Claim one slot for a message that must not be lost.
    ///
    /// Take this *before* starting whatever produces the message (D-33): a
    /// request reserves its completion slot at submit, an interactive
    /// subscription its fault slot at registration. Backpressure then lands here,
    /// on the caller's own thread, where it can be handled -- rather than at a
    /// delivery that has no way to fail safely.
    ///
    /// Returns `None` if the queue is full. Dropping the reservation without
    /// sending releases the slot.
    pub fn reserve(&self) -> Option<Reservation> {
        {
            let mut state = lock(&self.shared.items);
            if state.free() == 0 {
                return None;
            }
            state.reserved += 1;
        }
        // Cloning takes the same lock, so it happens after the guard above is
        // released. The clone is what keeps the queue connected for as long as
        // the reservation is outstanding: a reservation can still deliver, so a
        // receiver must not see the stream as finished while one exists.
        Some(Reservation {
            sender: self.clone(),
            used: false,
        })
    }

    /// Whether a best-effort notification would be accepted right now.
    ///
    /// The observation tier checks this *before* arming a read rather than
    /// discovering it at the enqueue, which is what turns saturation into a
    /// grace period in the kernel's own change buffer instead of a loss (D-29).
    pub fn has_room(&self) -> bool {
        lock(&self.shared.items).free() > 0
    }

    /// Ask to be prodded when a saturated queue frees a slot.
    ///
    /// Registering is idempotent in effect: a spurious prod costs a producer only
    /// a re-check.
    pub(crate) fn register_resume(&self, who: &Arc<impl Resume + 'static>) {
        lock(&self.shared.items)
            .resumers
            .push(Arc::downgrade(who) as Weak<dyn Resume>);
    }

    /// The queue's bound.
    pub fn capacity(&self) -> usize {
        lock(&self.shared.items).capacity
    }
}

/// A claimed slot in the notification queue.
///
/// Holding one is what makes delivery infallible, so a control message is
/// produced only once its room is already secured (D-33). Releasing it without
/// sending is fine -- the slot returns to the pool -- and is what happens when a
/// request is abandoned before it completes.
pub struct Reservation {
    sender: Sender,
    used: bool,
}

impl Reservation {
    /// Deliver the notification into the reserved slot.
    ///
    /// Cannot fail: the room was taken when this reservation was made.
    pub fn send(mut self, notification: Notification) {
        {
            let mut state = lock(&self.sender.shared.items);
            // Flushed while this reservation is still counted, so the latch can
            // only take genuinely free slots and never this one.
            state.flush_latched();
            state.reserved -= 1;
            state.queue.push_back(notification);
            refresh_doorbell(&self.sender.shared, &state);
        }
        self.used = true;
        self.sender.shared.arrived.notify_all();
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if !self.used {
            lock(&self.sender.shared.items).reserved -= 1;
        }
    }
}

impl std::fmt::Debug for Reservation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reservation").finish_non_exhaustive()
    }
}

impl Drop for Sender {
    fn drop(&mut self) {
        let mut state = lock(&self.shared.items);
        state.senders -= 1;
        let last = state.senders == 0;
        if last {
            // The end of the stream is something to observe, so the doorbell
            // reports it too -- otherwise a client waiting on it would wait for a
            // notification nothing can send.
            refresh_doorbell(&self.shared, &state);
        }
        drop(state);
        if last {
            // Wake anyone blocked in `recv`, so a queue that can never be filled
            // again does not hang its receiver.
            self.shared.arrived.notify_all();
        }
    }
}

impl std::fmt::Debug for Sender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sender").finish_non_exhaustive()
    }
}

/// The client-side half: the only way to observe notifications.
pub struct Receiver {
    shared: Arc<Shared>,
}

impl Receiver {
    /// Take the next notification if one is already available.
    #[must_use]
    pub fn try_recv(&self) -> Option<Notification> {
        let (item, resumers) = {
            let mut state = lock(&self.shared.items);
            let taken = take(&mut state);
            refresh_doorbell(&self.shared, &state);
            let resumers = freed_resumers(&mut state, taken.is_some());
            (taken, resumers)
        };
        prod(resumers);
        item
    }

    /// Block until a notification is available, or every sender is gone.
    ///
    /// Returns `None` only when nothing is queued, nothing is latched, *and* no
    /// sender remains, so a client loop terminates on teardown instead of
    /// hanging -- and never before it has been told about a loss.
    #[must_use]
    pub fn recv(&self) -> Option<Notification> {
        let mut state = lock(&self.shared.items);
        loop {
            if let Some(item) = take(&mut state) {
                refresh_doorbell(&self.shared, &state);
                let resumers = freed_resumers(&mut state, true);
                drop(state);
                prod(resumers);
                return Some(item);
            }
            if state.senders == 0 {
                return None;
            }
            state = self
                .shared
                .arrived
                .wait(state)
                .unwrap_or_else(|poison| poison.into_inner());
        }
    }

    /// Block for at most `timeout`.
    ///
    /// Returns `None` on timeout as well as on teardown; a caller that must tell
    /// them apart can check [`Receiver::is_disconnected`].
    #[must_use]
    pub fn recv_timeout(&self, timeout: Duration) -> Option<Notification> {
        let deadline = Instant::now() + timeout;
        let mut state = lock(&self.shared.items);
        loop {
            if let Some(item) = take(&mut state) {
                refresh_doorbell(&self.shared, &state);
                let resumers = freed_resumers(&mut state, true);
                drop(state);
                prod(resumers);
                return Some(item);
            }
            if state.senders == 0 {
                return None;
            }
            let remaining = deadline.checked_duration_since(Instant::now())?;
            let (next, _) = self
                .shared
                .arrived
                .wait_timeout(state, remaining)
                .unwrap_or_else(|poison| poison.into_inner());
            state = next;
        }
    }

    /// A manual-reset event that is signalled whenever this receiver has
    /// something to take.
    ///
    /// This is what lets a client integrate with its own thread pool instead of
    /// dedicating a thread to a blocking [`Receiver::recv`]: wait on the handle,
    /// then drain with [`Receiver::try_recv`] until it yields `None`.
    ///
    /// The event is created on the first call, so a client that never asks for it
    /// pays for no kernel object. It stays signalled while anything is
    /// outstanding, including once the stream has ended -- so a waiter learns
    /// about disconnection rather than waiting for a notification that can never
    /// arrive.
    ///
    /// The borrow is deliberate: the event belongs to this queue and must not be
    /// closed by a caller. Use [`Receiver::doorbell_owned`] where ownership is
    /// required, such as arming a `ThreadpoolWait`.
    ///
    /// # Errors
    ///
    /// Returns the error from `CreateEventW` on the first call.
    pub fn doorbell(&self) -> io::Result<BorrowedHandle<'_>> {
        self.ensure_doorbell()?;
        Ok(self
            .shared
            .doorbell
            .get()
            .expect("the doorbell was just created")
            .as_handle())
    }

    /// A duplicate of [`Receiver::doorbell`] that the caller owns.
    ///
    /// The duplicate refers to the same event, so signalling reaches both; the
    /// caller closes its own copy whenever it likes. This is the form a
    /// `ThreadpoolWait` needs, since arming one takes ownership of its target.
    ///
    /// # Errors
    ///
    /// Returns the error from `CreateEventW` or `DuplicateHandle`.
    pub fn doorbell_owned(&self) -> io::Result<OwnedHandle> {
        duplicate(self.doorbell()?)
    }

    /// Create the doorbell if it does not exist yet.
    fn ensure_doorbell(&self) -> io::Result<()> {
        if self.shared.doorbell.get().is_some() {
            return Ok(());
        }
        // Created under the queue lock so its initial state cannot disagree with
        // the queue it reports on: a client that asks for a doorbell after
        // notifications have already arrived must find it signalled.
        let state = lock(&self.shared.items);
        if self.shared.doorbell.get().is_none() {
            let event = create_event(state.pending())?;
            let _ = self.shared.doorbell.set(event);
        }
        Ok(())
    }

    /// Whether every sender has been dropped, so nothing further can arrive.
    #[must_use]
    pub fn is_disconnected(&self) -> bool {
        lock(&self.shared.items).senders == 0
    }

    /// How many notifications are queued right now.
    ///
    /// Excludes latched losses, which are not queued -- that is the point of the
    /// latch. [`Receiver::latched`] counts those.
    #[must_use]
    pub fn len(&self) -> usize {
        lock(&self.shared.items).queue.len()
    }

    /// Whether nothing is queued right now.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// How many subscriptions are owed a `Desync { QueueFull }`.
    #[must_use]
    pub fn latched(&self) -> usize {
        lock(&self.shared.items).latched.len()
    }

    /// The queue's bound.
    #[must_use]
    pub fn capacity(&self) -> usize {
        lock(&self.shared.items).capacity
    }
}

/// Take the next item: a queued notification, or -- once the queue is drained --
/// a synthesised report of a loss that never fitted.
///
/// The queue comes first because the latch records a loss that happened *after*
/// everything still queued: the queue was full at the time, so surfacing the
/// desync early would claim the hole is older than it is and break the ordering
/// D-12 promises.
fn take(state: &mut State) -> Option<Notification> {
    if let Some(item) = state.queue.pop_front() {
        return Some(item);
    }
    state.latched.pop_front().map(|watch| Notification::Desync {
        watch,
        cause: DesyncCause::QueueFull,
    })
}

impl std::fmt::Debug for Receiver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = lock(&self.shared.items);
        f.debug_struct("Receiver")
            .field("queued", &state.queue.len())
            .field("capacity", &state.capacity)
            .field("latched", &state.latched.len())
            .field("disconnected", &(state.senders == 0))
            .finish_non_exhaustive()
    }
}

/// Create a connected sender/receiver pair with the default bound.
pub fn channel() -> (Sender, Receiver) {
    channel_with_bound(DEFAULT_BOUND)
}

/// Create a connected sender/receiver pair holding at most `bound` notifications.
///
/// The bound is a [`NonZeroUsize`] rather than a checked `usize` because a zero
/// bound is not a runtime condition to report -- it is a queue that could not
/// carry even the desync announcing its own saturation, making the crate's
/// never-silently-lose guarantee vacuous. Making it unrepresentable rejects it at
/// construction, where D-11 asks for it, and leaves no error path to handle.
pub fn channel_with_bound(bound: NonZeroUsize) -> (Sender, Receiver) {
    let shared = Arc::new(Shared {
        items: Mutex::new(State {
            queue: VecDeque::new(),
            capacity: bound.get(),
            reserved: 0,
            latched: VecDeque::new(),
            senders: 1,
            resumers: Vec::new(),
        }),
        arrived: Condvar::new(),
        doorbell: OnceLock::new(),
    });
    (
        Sender {
            shared: Arc::clone(&shared),
        },
        Receiver { shared },
    )
}

/// The producers to prod, if taking an item has just made room.
///
/// Returns them rather than calling them, because a producer's `resume` must not
/// run under the queue lock: the obvious implementation re-arms a read, and a
/// failed arm reports itself by *sending*, which would take this very lock.
///
/// Dead producers are pruned here, which is the only place the list is walked.
fn freed_resumers(state: &mut State, took_one: bool) -> Vec<Arc<dyn Resume>> {
    // Only on the transition into having room: prodding on every take would put
    // a wake behind every single notification a client drains.
    if !took_one || state.resumers.is_empty() || state.free() != 1 {
        return Vec::new();
    }
    let mut live = Vec::with_capacity(state.resumers.len());
    let mut awake = Vec::new();
    for weak in state.resumers.drain(..) {
        if let Some(producer) = weak.upgrade() {
            live.push(Arc::downgrade(&producer));
            awake.push(producer);
        }
    }
    state.resumers = live;
    awake
}

/// Tell each producer there may be room now.
fn prod(resumers: Vec<Arc<dyn Resume>>) {
    for producer in resumers {
        producer.resume();
    }
}

/// Lock, recovering the guard if a previous holder panicked.
///
/// A poisoned lock here means some thread panicked while holding it; the queue is
/// a plain `VecDeque` plus a count, both of which are left structurally intact by
/// every path, so refusing to proceed would strand the receiver rather than
/// protect anything.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// Bring the doorbell into agreement with the state.
///
/// Every mutation ends here, under the same lock a receiver holds while deciding
/// there is nothing to take -- which is what makes a lost wakeup impossible
/// rather than merely unlikely: there is no window between the two decisions for
/// one to fall into.
fn refresh_doorbell(shared: &Shared, state: &State) {
    let Some(doorbell) = shared.doorbell.get() else {
        // Never asked for, so there is nothing to keep in agreement.
        return;
    };
    let raw = doorbell.as_raw_handle();
    // SAFETY: `raw` is a live manual-reset event owned by this queue for as long
    // as the queue exists, and neither call has any other precondition.
    unsafe {
        if state.pending() {
            SetEvent(raw);
        } else {
            ResetEvent(raw);
        }
    }
}

/// Create a manual-reset event in the given initial state.
fn create_event(signalled: bool) -> io::Result<OwnedHandle> {
    // SAFETY: creates an unnamed event with default security attributes; both
    // pointer arguments are null by design.
    let raw = unsafe {
        CreateEventW(
            ptr::null(),
            TRUE,
            if signalled { TRUE } else { FALSE },
            ptr::null(),
        )
    };
    if raw.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the call returned a fresh, exclusively owned event handle.
    Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
}

/// Duplicate a handle into this process, so the caller owns its own copy.
fn duplicate(handle: BorrowedHandle<'_>) -> io::Result<OwnedHandle> {
    let mut duplicated = ptr::null_mut();
    // SAFETY: duplicates a live handle within this process with the same access;
    // `duplicated` is a valid out-pointer for the call's duration.
    let ok = unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            handle.as_raw_handle(),
            GetCurrentProcess(),
            &raw mut duplicated,
            0,
            FALSE,
            DUPLICATE_SAME_ACCESS,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the call succeeded, so `duplicated` is a fresh owned handle.
    Ok(unsafe { OwnedHandle::from_raw_handle(duplicated) })
}

#[cfg(test)]
mod tests;
