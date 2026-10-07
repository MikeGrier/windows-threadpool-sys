// Copyright (c) 2026 Mike Grier
//! What dioring shares with `EventDelivery`'s callback: the entry queue, under dioring's own lock,
//! and the readiness event (DI-D-18, DI-D-28).
//!
//! The callback runs on pool threads with the ring's lock released, and two invocations may
//! overlap (`windows-ioring-sys`' D-83). It binds to exactly that: each completion is recorded
//! into the queue under dioring's lock, so the queue's order is the order completions were
//! recorded in, whichever thread recorded them. Nothing here assumes an order between two
//! callbacks.

use std::collections::VecDeque;
use std::io;
use std::sync::{Mutex, MutexGuard, PoisonError};

use win_sync_sys::{Event, ResetMode};
use windows_ioring_sys::Completion;

use super::Sidecar;
use crate::contract::EpochId;
use crate::ids::DioringIds;
use crate::types::{Entry, OpCompletion, Outcome};

/// An entry of a dioring instance's queue.
pub(crate) type DioringEntry<E, B, C> = Entry<DioringIds<E>, B, C>;

/// The state dioring's delivery callback reaches.
pub(crate) struct Relay<E: EpochId + 'static, B, C> {
    queue: Mutex<VecDeque<DioringEntry<E, B, C>>>,
    /// Auto-reset; the consumer's `readiness()` is a duplicate of it.
    readiness: Event,
}

impl<E: EpochId + 'static, B, C> Relay<E, B, C> {
    /// An empty queue and an unsignalled readiness event.
    pub(crate) fn new() -> io::Result<Self> {
        Ok(Self {
            queue: Mutex::new(VecDeque::new()),
            readiness: Event::new(ResetMode::Auto, false)?,
        })
    }

    /// A duplicate of the readiness event.
    pub(crate) fn readiness(&self) -> io::Result<Event> {
        self.readiness.try_clone()
    }

    /// The next entry, if any.
    pub(crate) fn pop(&self) -> Option<DioringEntry<E, B, C>> {
        self.lock().pop_front()
    }

    /// Record one ring completion. Called from `EventDelivery`'s callback, on a pool thread.
    ///
    /// Panicking here would abort the process, so nothing in this path panics on a value it
    /// receives. A completion with no sidecar, or a covering flush's, cannot reach it yet --
    /// dioring pushes nothing outside the inventory, and pushes no flush until seals exist
    /// (DI-3.2.3) -- so both are dropped, with a debug assertion.
    pub(crate) fn record(&self, completion: Completion, held: Option<(Option<B>, Sidecar<E, C>)>) {
        match held {
            Some((
                buffer,
                Sidecar::Consumer {
                    id, kind, context, ..
                },
            )) => {
                let outcome = match completion.result() {
                    // A transfer is never longer than the `u32` length it was pushed with.
                    Ok(transferred) => {
                        Outcome::Transferred(u32::try_from(transferred).unwrap_or(u32::MAX))
                    }
                    Err(error) => Outcome::Failed(error),
                };
                self.append(Entry::Op(OpCompletion {
                    id,
                    kind,
                    outcome,
                    buffer,
                    context,
                }));
            }
            Some((_, Sidecar::Commit { .. })) => {
                debug_assert!(false, "a covering flush completed before seals exist");
            }
            None => debug_assert!(false, "a completion for an operation dioring did not push"),
        }
    }

    /// Append an entry, and set the readiness event when the queue was empty: the entry is
    /// poppable before the event is set, and every empty-to-non-empty transition sets it
    /// (DI-D-28). A transition the consumer races -- popping the entry before the set lands --
    /// leaves it a wake with nothing to pop, which the contract calls normal.
    fn append(&self, entry: DioringEntry<E, B, C>) {
        let was_empty = {
            let mut queue = self.lock();
            let was_empty = queue.is_empty();
            queue.push_back(entry);
            was_empty
        };
        if was_empty {
            // Setting an event this instance created, with full access, does not fail.
            let set = self.readiness.set();
            debug_assert!(set.is_ok(), "setting the readiness event failed: {set:?}");
        }
    }

    fn lock(&self) -> MutexGuard<'_, VecDeque<DioringEntry<E, B, C>>> {
        // A panic while the queue was held leaves it a valid queue: every update is one push or
        // one pop.
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
