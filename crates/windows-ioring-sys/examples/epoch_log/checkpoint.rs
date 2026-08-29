// Copyright (c) 2026 Mike Grier
//! Checkpointing on the thread pool (M14.2): the control plane, off the log's
//! own thread.
//!
//! # The hybrid this file exists to show
//!
//! The crate's design notes recommend Model B on a hot path and Model A for
//! everything else, and nothing in the crate showed both at once. This log now
//! does:
//!
//! - **Data path, Model B.** The pinned log thread owns its ring, submits to
//!   it, and drains it (see [`crate::event_loop`]). It never gives that up:
//!   append latency is what the log is judged on, and a hand-off to a pool
//!   thread would add a scheduling hop to every record.
//! - **Control plane, Model A.** Checkpointing is rare, off the critical path,
//!   and nobody is waiting on it. It gets its own ring, handed to
//!   [`EventDelivery`], so its completions arrive on pool threads and the log
//!   thread never blocks for them.
//!
//! Two rings, not one, and that is not an accident of implementation. A ring
//! handed to `EventDelivery` is *owned* by it: its completion event already has
//! a waiter, and D-21 says a second waiter on the same event cannot be made
//! correct, because the drain that re-arms the edge has to run to empty exactly
//! once. So "some completions to the pool, some to my own wait" is not
//! available on one ring. The choice is per ring, and a program that wants both
//! shapes buys both rings.
//!
//! # The ordering chain, and where it crosses threads
//!
//! 1. The **log thread** observes epoch *N* durable and submits a checkpoint:
//!    one write of the new watermark plus one covering flush, on the control
//!    ring.
//! 2. A **pool thread** observes that flush complete. The watermark is now
//!    durable, which is what makes the retired segment genuinely dead -- a
//!    reader recovering from the checkpoint will never look at it again.
//! 3. That same pool thread asks the **reclaim worker** to zero it (see
//!    [`crate::reclaim`]).
//! 4. The **log thread** learns the whole chain finished, through the one
//!    reclaim handle already in its multiplexed wait.
//!
//! Three execution contexts, one ordering chain, and the log thread blocks for
//! none of it. Note again what does *not* participate: `drain_preceding` orders
//! SQEs within a ring (D-24), so it cannot order step 3 against step 2, and it
//! cannot even order step 2 against step 1's ring, because they are different
//! rings. Every arrow above is enforced by this program.

use std::io;
use std::os::windows::io::RawHandle;
use std::sync::{Arc, Mutex};

use windows_ioring_sys::{
    Batch, EventDelivery, FlushCoverage, FlushMode, IoRing, PushOptions, Token, WriteCaching,
};

use crate::commit::Epoch;
use crate::reclaim::Reclaimer;

/// What a checkpoint record holds. Deliberately tiny: the watermark is the
/// whole point, and a recovering reader needs nothing else to know which
/// prefix it may skip.
const MAGIC: &[u8; 8] = b"EPLOGCKP";

/// Shared between the log thread (which submits) and the pool threads (which
/// complete).
struct State {
    /// `UserData` of an in-flight covering flush -> the watermark it makes
    /// durable, and how far the retired segment may then be reclaimed.
    in_flight: std::collections::HashMap<usize, (Epoch, u64)>,
    /// The buffers the writes are reading from, held in their tokens. A token
    /// that is dropped unclaimed deliberately leaks its buffer (that is what
    /// keeps the kernel's pointer valid), so claiming it on completion is not
    /// tidiness -- it is the only way the memory comes back.
    writes: std::collections::HashMap<usize, Token<Vec<u8>>>,
    /// The highest watermark whose checkpoint is durable.
    durable_through: Option<Epoch>,
    /// Anything that went wrong on a pool thread, kept for the log thread to
    /// report. A failure swallowed on a callback thread is a failure nobody
    /// ever hears about.
    failures: Vec<String>,
    /// How many checkpoints have completed, so a caller can wait for one
    /// without racing.
    completed: usize,
}

/// Owns the control ring and the thread-pool delivery over it.
pub struct Checkpointer {
    delivery: EventDelivery,
    state: Arc<Mutex<State>>,
    handle: RawHandle,
}

impl Checkpointer {
    /// Hand `ring` to the thread pool and wire its completions to `reclaimer`.
    ///
    /// `handle` is the checkpoint file's handle; it must outlive this value.
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::Unsupported`] if the system does not report
    /// `IORING_FEATURE_SET_COMPLETION_EVENT`, or any error from
    /// `EventDelivery::new`.
    pub fn new(
        ring: IoRing,
        handle: RawHandle,
        reclaimer: Arc<Mutex<Reclaimer>>,
    ) -> io::Result<Self> {
        let state = Arc::new(Mutex::new(State {
            in_flight: std::collections::HashMap::new(),
            writes: std::collections::HashMap::new(),
            durable_through: None,
            failures: Vec::new(),
            completed: 0,
        }));
        let for_callback = Arc::clone(&state);

        let delivery = EventDelivery::new(
            ring,
            move |completion| {
                let mut state = for_callback
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let user_data = completion.user_data();
                if let Some(token) = state.writes.remove(&user_data) {
                    // The write's own completion. Claiming reclaims the
                    // buffer; the covering flush is what carries the meaning,
                    // so nothing else happens here. A failed write is still
                    // reported, because the flush that follows it would
                    // otherwise make a checkpoint out of bytes that never
                    // landed.
                    match token.claim_if(&completion) {
                        Ok(_buffer) => {
                            if let Err(error) = completion.result() {
                                state
                                    .failures
                                    .push(format!("checkpoint record write: {error}"));
                            }
                        }
                        Err(token) => {
                            // Cannot happen: the key *is* the token's id.
                            state.writes.insert(user_data, token);
                        }
                    }
                    return;
                }
                let Some((epoch, reclaim_to)) = state.in_flight.remove(&user_data) else {
                    return;
                };
                if let Err(error) = completion.result() {
                    state
                        .failures
                        .push(format!("checkpoint for epoch {}: {error}", epoch.0));
                    state.completed += 1;
                    return;
                }
                state.durable_through = Some(state.durable_through.map_or(epoch, |t| t.max(epoch)));
                state.completed += 1;

                // Step 3: the checkpoint is durable, so the retired segment is
                // dead and the reclaim may go. This is the ordering the ring
                // cannot express, issued from a pool thread -- the "background
                // path driven from the pool" in one line.
                let mut reclaimer = reclaimer
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Err(error) = reclaimer.request(epoch, reclaim_to) {
                    state
                        .failures
                        .push(format!("reclaim request for epoch {}: {error}", epoch.0));
                }
            },
            None,
        )?;

        Ok(Self {
            delivery,
            state,
            handle,
        })
    }

    /// Submit a checkpoint recording `watermark`, which -- once durable --
    /// authorises reclaiming the retired segment up to `reclaim_to`.
    ///
    /// Returns immediately: everything after the submit happens on pool
    /// threads.
    ///
    /// # Errors
    ///
    /// Any error from the pushes or the submit.
    pub fn submit(&self, watermark: Epoch, reclaim_to: u64) -> io::Result<()> {
        let mut record = Vec::with_capacity(MAGIC.len() + 8);
        record.extend_from_slice(MAGIC);
        record.extend_from_slice(&watermark.0.to_le_bytes());

        let mut ring = self
            .delivery
            .ring()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut batch = Batch::new(&mut ring);
        // SAFETY: `record` is kept alive in `State::buffers` until the flush
        // that covers it completes, and `self.handle` outlives this value by
        // the contract on `new`.
        let write = unsafe {
            batch.write_raw(
                self.handle,
                record,
                0,
                PushOptions::new(),
                WriteCaching::Cached,
            )
        }?;
        // Covering, for exactly the reason `Committer::commit` is: an
        // unordered flush is an ordinary operation racing the write it is
        // meant to cover (D-23), and a checkpoint that races its own record is
        // worse than no checkpoint -- it authorises a reclaim on the strength
        // of bytes that may not be there.
        // SAFETY: as above.
        let flush = unsafe {
            batch.flush_raw(
                self.handle,
                FlushCoverage::CoversPrecedingOperations,
                FlushMode::Default,
            )
        }?;

        // Registered before the submit, so the completion cannot arrive before
        // the callback knows what it means. The push assigns `UserData`; only
        // the submit makes the operation visible to the kernel.
        {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.in_flight.insert(flush, (watermark, reclaim_to));
            state.writes.insert(write.id(), write);
        }
        batch.submit()?;
        Ok(())
    }

    /// How many checkpoints have finished, successfully or otherwise.
    pub fn completed(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .completed
    }

    /// The highest watermark whose checkpoint is durable.
    pub fn durable_through(&self) -> Option<Epoch> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .durable_through
    }

    /// Everything that failed on a pool thread.
    pub fn failures(&self) -> Vec<String> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .failures
            .clone()
    }
}
