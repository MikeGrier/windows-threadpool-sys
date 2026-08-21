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
//! This is the interim, entirely in-crate endpoint for M2. The session/receiver
//! split, the bounded overflow policy with its latched `Desync { QueueFull }`,
//! and the doorbell all land in M3.

// The queue reaches a client through `Session`, but its reservation half has no
// production producer until M3.6 issues request completions and M5 issues fault
// reports, so part of this surface reads as dead under default features. Remove
// this when those land.
#![allow(dead_code)]

use std::collections::VecDeque;
use std::num::NonZeroUsize;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

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
}

impl Notification {
    /// The subscription this notification belongs to.
    #[must_use]
    pub fn watch(&self) -> WatchId {
        match self {
            Notification::Batch { watch, .. } | Notification::Desync { watch, .. } => *watch,
        }
    }
}

/// The shared queue storage.
struct Shared {
    items: Mutex<State>,
    arrived: Condvar,
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
}

impl State {
    /// Slots available to the best-effort path: neither occupied nor reserved.
    fn free(&self) -> usize {
        self.capacity - self.queue.len() - self.reserved
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
            if state.free() > 0 {
                state.queue.push_back(notification);
                Delivery::Queued
            } else {
                state.latch(watch);
                Delivery::Latched
            }
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
        take(&mut lock(&self.shared.items))
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
        }),
        arrived: Condvar::new(),
    });
    (
        Sender {
            shared: Arc::clone(&shared),
        },
        Receiver { shared },
    )
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

#[cfg(test)]
mod tests;
