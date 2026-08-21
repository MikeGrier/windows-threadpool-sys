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
//! object and the client's cadence. The M3.3.1 doorbell (D-25) exists to make
//! precisely that integration possible without dedicating a thread.
//!
//! This is the interim, entirely in-crate endpoint for M2. The session/receiver
//! split, the bounded overflow policy with its latched `Desync { QueueFull }`,
//! and the doorbell all land in M3.

// The queue is in-crate for M2 by design; M3.2 hands the receiver to a client
// through `Session` and exports these types for real. Until then they are
// reachable publicly only under `unstable-internals`, so under default features
// the receiving half reads as dead. Remove this then.
#![allow(dead_code)]

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::notify::{Change, DesyncCause};

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
    /// Set when every sender is gone, so a blocked receiver can stop waiting
    /// rather than hang forever on a queue nothing can fill.
    senders: usize,
}

/// The crate-side half: enqueues, never blocks, never fails.
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
    /// Enqueue one notification.
    ///
    /// Runs on the cadence path, so it must not block and must not fail. It is
    /// unbounded for M2; M3.3 adds the bound and the drop-with-latched-desync
    /// policy that replaces unbounded growth.
    pub fn send(&self, notification: Notification) {
        let mut state = lock(&self.shared.items);
        state.queue.push_back(notification);
        drop(state);
        self.shared.arrived.notify_all();
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
    /// Take the next notification if one is already queued.
    #[must_use]
    pub fn try_recv(&self) -> Option<Notification> {
        lock(&self.shared.items).queue.pop_front()
    }

    /// Block until a notification is available, or every sender is gone.
    ///
    /// Returns `None` only when the queue is empty *and* no sender remains, so a
    /// client loop terminates on teardown instead of hanging.
    #[must_use]
    pub fn recv(&self) -> Option<Notification> {
        let mut state = lock(&self.shared.items);
        loop {
            if let Some(item) = state.queue.pop_front() {
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
            if let Some(item) = state.queue.pop_front() {
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
    #[must_use]
    pub fn len(&self) -> usize {
        lock(&self.shared.items).queue.len()
    }

    /// Whether nothing is queued right now.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl std::fmt::Debug for Receiver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Receiver")
            .field("queued", &self.len())
            .field("disconnected", &self.is_disconnected())
            .finish_non_exhaustive()
    }
}

/// Create a connected sender/receiver pair.
pub fn channel() -> (Sender, Receiver) {
    let shared = Arc::new(Shared {
        items: Mutex::new(State {
            queue: VecDeque::new(),
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
