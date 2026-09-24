// Copyright (c) 2026 Mike Grier
//! The append path (M13.2): records composed into a registered buffer arena
//! and pushed through a `Batch`.
//!
//! # The arena
//!
//! Buffers are registered once, up front, and reused for the log's whole life.
//! That is the shape a real consumer has -- memory it sized and placed
//! deliberately -- and it is why this sample uses the registered form rather
//! than handing an owned `Vec` to every push.
//!
//! **Placed** is meant literally as of M22.3: the arena is `NumaBuffer`, not
//! `Vec<u8>`, on the node [`crate::placement`] decides. That module is also
//! where the limits of the decision are written down -- notably that this
//! workload is far too flush-bound for the placement to pay, so the sample
//! demonstrates how the choice is made rather than that it was worth making.
//!
//! Appending therefore has two halves that must not be confused:
//!
//! 1. **Compose** the record into a slot the kernel is not currently reading.
//!    `RegisteredBuffers::get_mut` enforces that: it refuses a slot with an
//!    operation still outstanding against it, and per-buffer accounting means
//!    a busy slot does not block its neighbours.
//! 2. **Push** the write over exactly the bytes the record occupies, and hold
//!    the returned token until its completion is popped -- which is what
//!    releases the slot for step 1 again.
//!
//! Nothing here makes a record durable. An append that returns has been
//! *accepted into the open epoch*, which is all [`crate::contract`] promises;
//! durability arrives with the epoch's commit in M13.3.

use std::io;
use std::os::windows::io::RawHandle;

use windows_ioring_sys::contract::RingContract;
use windows_ioring_sys::{
    Batch, IoBufMut, IoRing, NumaBuffer, Pending, PushOptions, RegisteredBuffers, RegisteredSpan,
    RegisteredUse, WriteCaching,
};

use crate::commit::Epoch;
use crate::placement::Placement;
use crate::record::{self, Sequence};

/// How many slots the arena holds. More slots means more records can be in
/// flight before the appender has to wait for one to come back.
pub const SLOTS: u32 = 8;

/// Bytes per slot, and so the largest record this log accepts.
pub const SLOT_LEN: usize = 4096;

/// Hang bound on the one blocking step this appender has: waiting for the
/// arena's registration to complete at startup.
const REGISTRATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Slots of `arena` with no operation outstanding against them, at most `want`
/// of them, lowest index first.
///
/// This is the sample's **only** definition of "which slots are free", and
/// both append paths bind to it: this module's [`Appender`] and the
/// measurement harness's `Lane` in [`crate::strategy`]. Each used to carry its
/// own, which is the defect that collapsed them -- not the cost, which is nil
/// at eight slots, but that two definitions of one fact can drift apart while
/// each looks locally correct.
///
/// # Why this is derived and not tracked
///
/// The obvious alternative is a `Vec<u32>` free list: pop a slot when an
/// append takes it, push it back when the completion is claimed. `Lane` did
/// exactly that. It is a *second copy* of a fact
/// [`RegisteredBuffers`] already owns and maintains -- its per-buffer
/// outstanding count, the same one that makes
/// [`RegisteredBuffers::get_mut`] refuse a busy slot. Asking is therefore
/// always right by construction, where a copy is right only as long as every
/// path that changes the truth remembers to change the copy too.
///
/// The free list had already stopped remembering, in a way nothing reported.
/// It popped a slot before composing into it, so any error between the pop and
/// the push -- a record too long for a slot is the reachable one -- returned
/// early with the slot removed from the free list and no operation ever
/// issued. The arena considered that slot quiet forever; the free list never
/// offered it again. `SLOTS` such errors and the harness wedges, blaming an
/// arena that is in fact entirely idle. The derived form cannot express that
/// bug: a slot nothing was pushed against never stopped being free.
///
/// That is measured rather than argued: re-injecting the free list and failing
/// eight appends left the lane reporting **0** of 8 slots free while the arena
/// held nothing. See [`crate::strategy::tests`], which also says plainly what
/// those tests can and cannot catch.
pub fn free_slots<B: IoBufMut>(arena: &RegisteredBuffers<B>, want: usize) -> Vec<u32> {
    (0..arena.len())
        .filter(|&slot| arena.outstanding(slot) == Some(0))
        .take(want)
        .collect()
}

/// The append path: an arena of registered buffers, a monotonic sequence
/// counter, and the file offset the next record lands at.
pub struct Appender {
    arena: RegisteredBuffers<NumaBuffer>,
    /// Unclaimed tokens, with the arena slot each holds.
    ///
    /// Checked, so the conservation oracle is driven by the same call that
    /// updates the map (M16.2's accounting, now wired rather than hand-driven).
    /// That matters for the specific failure it guards: an early return from
    /// [`Appender::claim`] that skips the token claim leaks the arena slot
    /// permanently, and nothing else in this program notices until the arena
    /// runs dry `SLOTS` failures later -- somewhere else entirely, with no
    /// trace of the cause.
    pending: Pending<RegisteredUse, u32>,
    next_sequence: u64,
    next_offset: u64,
}

impl Appender {
    /// Register the arena and build an appender over it.
    ///
    /// Registration is itself a ring operation, so this submits and waits for
    /// its completion -- one blocking step at startup, before the log has any
    /// work to pipeline against.
    ///
    /// # Placement
    ///
    /// `placement` decides which NUMA node the arena's pages prefer. It is
    /// taken as an argument rather than decided here because it is a *policy*
    /// question about a caller's storage layout, and the library deliberately
    /// answers none -- [`crate::placement`] is where this sample makes its own
    /// choice, and says what that choice is and is not worth.
    ///
    /// # Errors
    ///
    /// Any error from allocating the arena, the registration push, the submit,
    /// or the registration operation itself.
    pub fn new(ring: &mut IoRing, placement: &Placement) -> io::Result<Self> {
        let node = placement.node();
        let buffers = (0..SLOTS)
            .map(|_| NumaBuffer::new(SLOT_LEN, node))
            .collect::<io::Result<Vec<_>>>()?;
        let mut batch = Batch::new(ring);
        let pending = batch.register_buffers(buffers)?;
        batch.submit()?;

        // One bounded wait, not a spin: `pop_within` is the crate's join
        // between `try_pop`'s "empty right now" and a submit-side wait whose
        // return promises nothing about poppability. The bare `loop` that
        // used to be here turned a slow registration into a hung process.
        let completion = ring.pop_within(REGISTRATION_TIMEOUT)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                "the buffer registration never completed",
            )
        })?;
        let arena = pending
            .claim_if(&completion)
            .map_err(|_| {
                io::Error::other("the first completion was not the buffer registration's")
            })?
            .map_err(|error| io::Error::other(format!("buffer registration failed: {error}")))?;

        Ok(Self {
            arena,
            pending: Pending::checked(),
            next_sequence: 0,
            next_offset: 0,
        })
    }

    /// This appender's conservation record, for a caller to assert against at
    /// teardown.
    ///
    /// Reads through to the map's own oracle rather than a separate one. An
    /// earlier draft of this conversion kept the `RingContract` field beside
    /// `Pending::checked()`, which compiled, ran, and made
    /// `assert_quiescent()` pass **vacuously** -- the field was never written
    /// to again, so a caller's teardown check was asserting against an oracle
    /// that had observed nothing.
    pub fn contract(&self) -> &RingContract {
        self.pending
            .contract()
            .expect("the appender's map is always checked")
    }

    /// The sequence the next appended record will carry.
    pub fn next_sequence(&self) -> Sequence {
        Sequence(self.next_sequence)
    }

    /// How many appends are pushed but not yet observed complete.
    pub fn in_flight(&self) -> usize {
        self.pending.len()
    }

    /// Compose as many of `payloads` as there are free arena slots, and push
    /// them all in **one** submission.
    ///
    /// Returns how many were accepted. **Zero is not an error**: it means
    /// every slot still has an append in flight, and the caller must drain a
    /// completion before trying again.
    ///
    /// # Why this is a batch, and why that is the point of the sample
    ///
    /// This used to push one write and submit it, per record. That is a
    /// working log and a misleading example: `Batch` exists so that many
    /// submission-queue entries cost one `SubmitIoRing`, and a sample whose
    /// job is to teach `Batch` should not pay that call per record.
    ///
    /// The batching is bounded by the arena rather than by the caller's list,
    /// which is what keeps the two halves of an append honest: a slot is
    /// composed into only while the kernel is not reading it, and it stays
    /// spoken for until its completion is observed. So the natural batch is
    /// "everything that fits right now", not "everything the caller has".
    ///
    /// # Errors
    ///
    /// Any error from encoding a record (notably if it does not fit a slot) or
    /// from a push. A failure partway leaves the records already pushed
    /// accounted for and in flight -- `Batch` submits what it queued when it
    /// drops (D-5), so they are real operations, not a rollback.
    pub fn append_batch(
        &mut self,
        ring: &mut IoRing,
        file: RawHandle,
        epoch: Epoch,
        payloads: &[Vec<u8>],
    ) -> io::Result<usize> {
        let slots = free_slots(&self.arena, payloads.len());
        if slots.is_empty() {
            return Ok(0);
        }

        let mut batch = Batch::new(ring);
        let mut accepted = 0;
        for (&slot, payload) in slots.iter().zip(payloads) {
            let sequence = Sequence(self.next_sequence);

            // Compose. `get_mut` is what makes this possible at all, and it is
            // also the check that the kernel is not reading this slot.
            //
            // The epoch is stamped in here, at the moment the append is
            // accepted -- which is exactly when the contract says a record's
            // epoch is decided, and never changes afterwards.
            let total = record::encode(self.arena.get_mut(slot)?, sequence, epoch, payload)?;

            // Push over exactly the bytes the record occupies rather than the
            // whole slot: writing the slot's unused tail would put stale bytes
            // in the log and cost real device bandwidth.
            let span = RegisteredSpan {
                buffer_index: slot,
                offset: 0,
                len: u32::try_from(total).map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "record length exceeds u32::MAX",
                    )
                })?,
            };
            let offset = self.next_offset;
            // SAFETY: `file` is the log's own handle and outlives every
            // operation pushed here -- the log drains to empty before it
            // closes. The token is held in `in_flight` until its completion is
            // observed, so the arena slot it names cannot be refilled
            // underneath the kernel.
            //
            // `PushOptions::new()` deliberately carries no barrier: records
            // stream unordered within an epoch, exactly as the contract says,
            // and the ordering that matters is bought once by the epoch's
            // covering flush. `WriteCaching::Cached` for the same reason --
            // write-through here would shape latency without changing what is
            // durable.
            let token = unsafe {
                batch.write_registered_raw(
                    file,
                    &self.arena,
                    span,
                    offset,
                    PushOptions::new(),
                    WriteCaching::Cached,
                )
            }?;

            // One call updates the map and its oracle, where this previously
            // updated them separately and could drift.
            self.pending.push(token, slot);
            self.next_sequence += 1;
            self.next_offset += total as u64;
            accepted += 1;
        }

        // One submission for the whole batch. This is the line the item
        // existed for.
        batch.submit()?;
        Ok(accepted)
    }

    /// Account for one popped completion that belongs to an append.
    ///
    /// Returns `true` if `completion` was one of ours. Claiming the token is
    /// what returns its arena slot to the free pool, so a caller that drops
    /// completions on the floor will run the arena dry and never recover --
    /// which is the same drain-to-empty discipline the ring itself demands.
    pub fn claim(&mut self, completion: &windows_ioring_sys::Completion) -> io::Result<bool> {
        // Claiming happens here, before the write's result is inspected, and
        // that ordering still matters: bailing out on a failed write without
        // claiming drops the token unclaimed, which `Token` deliberately
        // treats as "still outstanding" and leaks -- burning this arena slot
        // permanently, so `free_slots` never offers it again and after `SLOTS`
        // failures every append returns `WouldBlock` forever. `M22.2` found
        // exactly that bug here.
        //
        // `Pending` does not make the inverted order unrepresentable -- it
        // compiles -- but it is no longer silent either way: the token stays in
        // the map, and `append/tests.rs` drives a failed write through the
        // injection seam so the inversion is caught by an assertion rather than
        // waiting for a production arena to run dry.
        let Some((released, slot)) = self.pending.claim(completion) else {
            return Ok(false);
        };
        // Dropping the marker is what decrements the slot's count, so it has
        // to happen before the check below rather than at end of scope.
        drop(released);
        debug_assert!(
            self.arena.outstanding(slot) == Some(0),
            "claiming the token must release the slot"
        );

        let _written = completion.result()?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests;
