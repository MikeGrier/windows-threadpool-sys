// Copyright (c) 2026 Mike Grier
//! The servicing path: a request queue drained by one `ThreadpoolWork`, so every
//! resident-state mutation happens under a single logical authority (D-2).
//!
//! This is the SQ half of the two-queue model. Requests arrive from any number of
//! client threads and are serviced one at a time, in order, on a pool thread the
//! crate never lends to client code.
//!
//! # The doorbell rings on idle -> scheduled, not on empty -> non-empty
//!
//! [`ThreadpoolWork::submit`] *is* the SQ doorbell (D-25) -- on this side of the
//! design the crate is the waiter, so queuing work beats an event and something
//! to wait on it. But each `submit` queues an **independent** invocation and they
//! do not coalesce, so ringing on every enqueue would queue 500 drains to service
//! 500 subscriptions, 499 of which would find the queue already emptied by the
//! first.
//!
//! The obvious fix -- ring only when the queue was empty -- coalesces but does
//! **not** serialise, and serialising is the part D-2 actually needs. A drain that
//! has emptied the queue and is still running its last handler leaves the queue
//! observably empty, so the next enqueue would ring and a second drain would
//! execute concurrently with the first. The condition is therefore whether a drain
//! is *outstanding*, not whether the queue is empty: [`Drain::Scheduled`] is set
//! under the queue lock by whoever rings, and cleared only by a drain that has
//! found nothing left to do. A drain loops until the queue is empty rather than
//! servicing one item per ring, which is what makes the two readings coincide in
//! the steady state.
//!
//! Clearing the flag is deliberately the drain's **last** touch of shared state: a
//! producer may ring the instant it is cleared, so anything done afterwards would
//! be racing the drain that ring starts.
//!
//! # Rundown
//!
//! Teardown closes the queue *before* waiting (the ordering lesson of D-23/D-34):
//! the other order lets a drain accept work after the wait began. Pending requests
//! are discarded rather than serviced, because every watcher they could act on is
//! being torn down in the same breath.

// The servicing path is exercised by this crate's own tests and, from M3.5, by
// requests submitted through a `Session`; until then some of its surface has no
// production caller. Remove this when M3.5 gives it one.
#![allow(dead_code)]

use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Mutex};

use windows_threadpool_sys::work::ThreadpoolWork;

/// Whether a drain is outstanding.
///
/// This is the doorbell's edge, and it is not the same question as "is the queue
/// empty" -- see the module docs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Drain {
    /// No drain is queued or running, so the next enqueue must ring.
    Idle,
    /// A drain is queued or running. It will pick up anything enqueued now, so
    /// ringing again would only queue a drain that finds nothing to do.
    Scheduled,
}

/// A request that could not be accepted, handed back to its submitter.
///
/// The only rejection is a shut-down monitor. Returning the request rather than
/// dropping it is what lets M3.6 report the outcome as a completion (D-30)
/// instead of leaving a client waiting for something that will never happen.
#[derive(Debug)]
pub struct Rejected<T>(pub(crate) T);

/// The queue and the state that governs who drains it.
struct Queue<T> {
    items: VecDeque<T>,
    drain: Drain,
    /// Set by shutdown. One reason only: unlike the watcher's arm gate (D-34),
    /// nothing else ever stops the servicing path, because D-33's reservation
    /// means request draining is never throttled for backpressure.
    closed: bool,
    /// How many times the doorbell has been rung, so coalescing is measurable
    /// rather than merely asserted.
    rings: u64,
    /// How many requests shutdown discarded.
    discarded: u64,
}

struct Shared<T> {
    queue: Mutex<Queue<T>>,
}

/// A request queue with exactly one servicing authority.
///
/// Generic over the request type so the mechanism can be tested on its own terms:
/// what it guarantees -- order, single-servicer, coalesced wakes, bounded rundown
/// -- is a property of the machinery, not of what flows through it.
pub(crate) struct Servicer<T: Send + 'static> {
    shared: Arc<Shared<T>>,
    work: ThreadpoolWork,
}

impl<T: Send + 'static> Servicer<T> {
    /// Build a servicing path that applies `handle` to each request in turn.
    ///
    /// `handle` runs on a pool thread, one call at a time, in submission order.
    /// Since M18 removed panic containment from the thread pool, a handler that
    /// unwinds aborts the process; it is crate-internal code, so that is a bug to
    /// fix rather than a condition to tolerate.
    ///
    /// # Errors
    ///
    /// Returns the error from `CreateThreadpoolWork`.
    pub(crate) fn new<H>(handle: H) -> io::Result<Self>
    where
        H: Fn(T) + Send + Sync + 'static,
    {
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue {
                items: VecDeque::new(),
                drain: Drain::Idle,
                closed: false,
                rings: 0,
                discarded: 0,
            }),
        });

        // The callback holds a strong reference, not a `Weak`: unlike the
        // watcher's completion callback it is not reached *through* the object it
        // would point back at, so there is no cycle -- the work object owns the
        // closure, the closure owns the queue, and nothing owns the work object
        // but the `Servicer`.
        let queue = Arc::clone(&shared);
        let work = ThreadpoolWork::new(move || drain(&queue, &handle), None)?;

        Ok(Self { shared, work })
    }

    /// Enqueue a request, ringing the doorbell only if no drain is outstanding.
    ///
    /// Never blocks on servicing: the lock is held for the enqueue alone, and the
    /// ring happens outside it.
    ///
    /// # Errors
    ///
    /// Returns the request unserviced if the servicing path has shut down.
    pub(crate) fn submit(&self, item: T) -> Result<(), Rejected<T>> {
        let ring = {
            let mut queue = lock(&self.shared.queue);
            if queue.closed {
                return Err(Rejected(item));
            }
            queue.items.push_back(item);
            let ring = queue.drain == Drain::Idle;
            if ring {
                queue.drain = Drain::Scheduled;
                queue.rings += 1;
            }
            ring
        };

        if ring {
            self.work.submit();
        }
        Ok(())
    }

    /// Refuse further requests, discard those pending, and wait for the drain.
    ///
    /// Idempotent, so a caller may shut down explicitly and still let `Drop` run
    /// (the teardown shape of D-34). After it returns, no handler is executing or
    /// can start.
    ///
    /// Must not be called from inside a handler: it waits for that handler to
    /// finish, so it would wait on itself.
    pub(crate) fn shut_down(&self) {
        {
            let mut queue = lock(&self.shared.queue);
            queue.closed = true;
            queue.discarded += queue.items.len() as u64;
            queue.items.clear();
        }

        // With the queue closed and emptied, a drain that has already been queued
        // has nothing left to do, so cancelling the ones that have not started
        // costs nothing and returns sooner than letting each run to discover it.
        self.work.cancel_pending();
    }

    /// Whether the servicing path is still accepting requests.
    pub(crate) fn is_open(&self) -> bool {
        !lock(&self.shared.queue).closed
    }

    /// How many requests are waiting to be serviced.
    pub(crate) fn pending(&self) -> usize {
        lock(&self.shared.queue).items.len()
    }

    /// How many times the doorbell has been rung.
    ///
    /// The measure of coalescing: this is the number of drains queued, which
    /// should stay far below the number of requests submitted.
    pub(crate) fn rings(&self) -> u64 {
        lock(&self.shared.queue).rings
    }

    /// How many requests shutdown discarded unserviced.
    pub(crate) fn discarded(&self) -> u64 {
        lock(&self.shared.queue).discarded
    }
}

impl<T: Send + 'static> Drop for Servicer<T> {
    fn drop(&mut self) {
        // One teardown implementation with two triggers; `shut_down` is
        // idempotent, so an explicit call beforehand costs nothing.
        self.shut_down();
    }
}

impl<T: Send + 'static> std::fmt::Debug for Servicer<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let queue = lock(&self.shared.queue);
        f.debug_struct("Servicer")
            .field("pending", &queue.items.len())
            .field("drain", &queue.drain)
            .field("closed", &queue.closed)
            .field("rings", &queue.rings)
            .finish_non_exhaustive()
    }
}

/// Service requests until none are left.
///
/// Draining to empty rather than one-per-ring is what makes the doorbell's
/// idle/scheduled edge coincide with the queue's empty/non-empty edge in the
/// steady state, and it is why 500 submissions do not need 500 rings.
fn drain<T, H>(shared: &Shared<T>, handle: &H)
where
    H: Fn(T),
{
    loop {
        let next = {
            let mut queue = lock(&shared.queue);
            match queue.items.pop_front() {
                Some(item) => item,
                None => {
                    // Deliberately the last touch of shared state in this
                    // callback: a producer may ring the instant this is cleared,
                    // and the drain that ring starts must not overlap this one.
                    queue.drain = Drain::Idle;
                    return;
                }
            }
        };

        // Outside the lock: servicing a request can be slow (opening a directory,
        // arming a read), and producers must not queue behind it.
        handle(next);
    }
}

/// Lock, recovering the guard if a previous holder panicked.
///
/// Panic containment was removed from the thread pool in M18, so a handler that
/// unwinds aborts rather than poisoning this lock; recovery here covers only a
/// panic on a client thread inside `submit`, which leaves the queue structurally
/// intact.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

#[cfg(test)]
mod tests;
