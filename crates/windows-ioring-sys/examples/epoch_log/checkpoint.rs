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
//! 2. A **pool thread** observes *both* of those complete, and both succeed.
//!    Only then is the watermark durable, which is what makes the retired
//!    segment genuinely dead -- a reader recovering from the checkpoint will
//!    never look at it again.
//! 3. That pool thread asks the **reclaim worker** to zero it (see
//!    [`crate::reclaim`]).
//! 4. The **log thread** learns the whole chain finished, through the one
//!    reclaim handle already in its multiplexed wait.
//!
//! Step 2 says "both, and both succeed" for a reason worth stating up front:
//! **a covering flush orders execution, it does not aggregate results.** The
//! flush is released when the operations before it *complete*, so a record
//! write that failed is followed by a flush that completes perfectly happily.
//! Acting on the flush alone would declare a watermark durable on the strength
//! of bytes that never landed, and then reclaim a segment on it. See
//! [`Pending`].
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
    EventDelivery, FlushCoverage, FlushMode, IoRing, PushOptions, Token, WriteCaching,
};

use crate::commit::Epoch;
use crate::reclaim::Reclaimer;

/// What a checkpoint record holds. Deliberately tiny: the watermark is the
/// whole point, and a recovering reader needs nothing else to know which
/// prefix it may skip.
const MAGIC: &[u8; 8] = b"EPLOGCKP";

/// One checkpoint's two operations, and what is known about them so far.
///
/// **A checkpoint is two operations, and both must succeed.** A covering flush
/// is released when the operations before it *complete*, not when they
/// *succeed* (D-24 orders execution, it does not aggregate results), so a
/// record write that fails with `ERROR_DISK_FULL` is followed by a flush that
/// completes perfectly happily. Treating that flush as the whole answer would
/// declare a watermark durable on the strength of bytes that never landed --
/// and then authorise reclaiming a segment on it.
///
/// So neither result is acted on alone: whichever completion arrives second
/// decides, and it authorises only if both succeeded.
struct Pending {
    epoch: Epoch,
    reclaim_to: u64,
    /// `None` until the corresponding completion has been observed.
    write_result: Option<Result<(), String>>,
    flush_result: Option<Result<(), String>>,
}

impl Pending {
    /// Both halves observed, and both succeeded.
    fn is_complete(&self) -> bool {
        self.write_result.is_some() && self.flush_result.is_some()
    }

    /// Every failure this checkpoint saw.
    fn failures(&self) -> Vec<String> {
        [self.write_result.as_ref(), self.flush_result.as_ref()]
            .into_iter()
            .flatten()
            .filter_map(|result| result.as_ref().err().cloned())
            .collect()
    }
}

/// Shared between the log thread (which submits) and the pool threads (which
/// complete).
struct State {
    /// Flush `UserData` -> the checkpoint that flush closes.
    pending: std::collections::HashMap<usize, Pending>,
    /// Write `UserData` -> (its checkpoint's flush `UserData`, the token
    /// holding the record).
    ///
    /// The token is what keeps the record buffer alive at a stable address
    /// while the kernel reads it. A token dropped unclaimed deliberately leaks
    /// that buffer (which is what keeps the kernel's pointer valid), so
    /// claiming it on completion is not tidiness -- it is the only way the
    /// memory comes back.
    writes: std::collections::HashMap<usize, (usize, Token<Vec<u8>>)>,
    /// The highest watermark whose checkpoint is durable.
    durable_through: Option<Epoch>,
    /// Anything that went wrong on a pool thread, kept for the log thread to
    /// report. A failure swallowed on a callback thread is a failure nobody
    /// ever hears about.
    failures: Vec<String>,
    /// How many checkpoints have finished, successfully or otherwise, so a
    /// caller can wait for one without racing.
    completed: usize,
}

/// Decide a checkpoint once both of its completions have been observed.
///
/// Called from whichever pool thread observes the second one. The two can
/// arrive in either order and on different threads -- `EventDelivery` re-arms
/// *inside* its callback, so two pool threads can be draining at once and
/// completion-queue order is not a processing order -- which is exactly why
/// this is keyed on "both present" rather than on the flush alone.
fn settle(state: &mut State, flush: usize, reclaimer: &Mutex<Reclaimer>) {
    let Some(pending) = state.pending.get(&flush) else {
        return;
    };
    if !pending.is_complete() {
        return;
    }
    let pending = state
        .pending
        .remove(&flush)
        .expect("just observed to be present");
    state.completed += 1;

    let failures = pending.failures();
    if !failures.is_empty() {
        // Not durable, so the watermark does not advance and no reclaim is
        // authorised. Saying "this epoch is not durable" is the truthful
        // answer, and it is the whole reason the failure is checked here
        // rather than merely logged.
        for failure in failures {
            state.failures.push(format!(
                "checkpoint for epoch {}: {failure}",
                pending.epoch.0
            ));
        }
        return;
    }

    state.durable_through = Some(
        state
            .durable_through
            .map_or(pending.epoch, |t| t.max(pending.epoch)),
    );

    // Step 3: the checkpoint is durable, so the retired segment is dead and
    // the reclaim may go. This is the ordering the ring cannot express, issued
    // from a pool thread -- the "background path driven from the pool" in one
    // line.
    let mut reclaimer = reclaimer
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Err(error) = reclaimer.request(pending.epoch, pending.reclaim_to) {
        state.failures.push(format!(
            "reclaim request for epoch {}: {error}",
            pending.epoch.0
        ));
    }
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
            pending: std::collections::HashMap::new(),
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

                if let Some((flush, token)) = state.writes.remove(&user_data) {
                    // The record write. Claiming the token is what returns its
                    // buffer; the result is recorded against the checkpoint
                    // rather than acted on here, because the flush may not have
                    // been observed yet.
                    let expected = match token.claim_if(&completion) {
                        Ok(record) => record.len(),
                        Err(token) => {
                            // Cannot happen: the key *is* the token's id.
                            state.writes.insert(user_data, (flush, token));
                            return;
                        }
                    };
                    let result = match completion.result() {
                        Err(error) => Err(format!("record write: {error}")),
                        // A short write is a failure too. The record is fixed
                        // size and tiny, so a partial one is not a "retry the
                        // tail" case -- it means what is on disk is not the
                        // record we composed.
                        Ok(written) if written != expected => Err(format!(
                            "record write was short: {written} of {expected} bytes"
                        )),
                        Ok(_) => Ok(()),
                    };
                    if let Some(pending) = state.pending.get_mut(&flush) {
                        pending.write_result = Some(result);
                    }
                    settle(&mut state, flush, &reclaimer);
                    return;
                }

                if let Some(pending) = state.pending.get_mut(&user_data) {
                    pending.flush_result = Some(
                        completion
                            .result()
                            .map(|_| ())
                            .map_err(|error| format!("covering flush: {error}")),
                    );
                    settle(&mut state, user_data, &reclaimer);
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

        let mut scope = self.delivery.scope();
        let mut batch = scope.batch();
        // SAFETY: `record` is moved into the token, which is kept alive in
        // `State::writes` until its own completion is observed and claimed --
        // so the buffer stays at a stable address for as long as the kernel
        // may read it. `self.handle` outlives this value by the contract on
        // `new`.
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
        //
        // Coverage orders execution; it does not aggregate results. So this
        // flush completing says nothing about whether the write *succeeded* --
        // which is why both are tracked against one `Pending` and neither is
        // acted on alone.
        // SAFETY: as above.
        let flush = unsafe {
            batch.flush_raw(
                self.handle,
                FlushCoverage::CoversPrecedingOperations,
                FlushMode::Default,
            )
        }?;

        // Registered before the submit, so a completion cannot arrive before
        // the callback knows what it means. The push assigns `UserData`; only
        // the submit makes the operation visible to the kernel, and the ring
        // lock is held across both -- so no pool thread can pop either
        // completion until this registration is visible.
        {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.pending.insert(
                flush,
                Pending {
                    epoch: watermark,
                    reclaim_to,
                    write_result: None,
                    flush_result: None,
                },
            );
            state.writes.insert(write.id(), (flush, write));
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
