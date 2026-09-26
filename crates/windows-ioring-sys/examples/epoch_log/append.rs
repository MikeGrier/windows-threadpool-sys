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
//! 2. **Push** the write over exactly the bytes the record occupies. The ring
//!    holds the slot's registration lease until the completion is popped, and
//!    that pop is what releases the slot for step 1 again.
//!
//! Nothing here makes a record durable. An append that returns has been
//! *accepted into the open epoch*, which is all [`crate::contract`] promises;
//! durability arrives with the epoch's commit in M13.3.

use std::io;
use std::os::windows::io::RawHandle;

use windows_ioring_sys::contract::RingContract;
use windows_ioring_sys::{
    Batch, IoBufMut, IoRing, NumaBuffer, PushOptions, RegisteredBuffers, RegisteredSpan,
    WriteCaching,
};

use crate::commit::Epoch;
use crate::placement::Placement;
use crate::record::{self, Sequence};

/// How many slots the arena holds. More slots means more records can be in
/// flight before the appender has to wait for one to come back.
pub const SLOTS: u32 = 8;

/// Bytes per slot, and so the largest record this log accepts.
pub const SLOT_LEN: usize = 4096;

// The write covers a whole stride out of one slot, so a stride wider than a
// slot would read past the arena. The stride itself is the format's, and is
// defined in `record` beside the layout it describes.
const _: () = assert!(
    record::RECORD_STRIDE <= SLOT_LEN,
    "a slot must hold a whole stride: the write spans one stride of one buffer"
);

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

/// The ring an [`Appender`] and its log share.
///
/// The appender names the ring type rather than taking a generic one, and that
/// is the answer to the question `D-73` raised: a helper that pushes onto a
/// ring has always constrained what that ring may hold, and the parameters
/// only make the constraint visible. `u32` is the arena slot an append reads
/// from, which is what the pop hands back.
///
/// The log's other producer, [`crate::commit::Committer`], pushes raw flushes
/// that hold nothing -- so a popped completion with no sidecar is a commit and
/// one with a sidecar is an append. That is the dispatch, and it is the ring's
/// answer rather than a guess: the drain used to offer each completion to the
/// appender and then the committer, taking whichever accepted it.
pub type AppendRing = IoRing<(), u32>;

/// The append path: an arena of registered buffers, a monotonic sequence
/// counter, and the file offset the next record lands at.
pub struct Appender {
    arena: RegisteredBuffers<NumaBuffer>,
    /// How many appends this appender still owes a completion for.
    ///
    /// A count rather than a map: the ring holds each append's registration
    /// lease and the arena slot it names, and releases both at the pop. The
    /// failure this field's predecessor guarded -- an early return from
    /// [`Appender::claim`] skipping the claim and burning a slot permanently,
    /// unnoticed until the arena ran dry `SLOTS` failures later -- is no
    /// longer reachable, because releasing the slot is not something this code
    /// does.
    outstanding: usize,
    contract: RingContract,
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
    pub fn new(ring: &mut AppendRing, placement: &Placement) -> io::Result<Self> {
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
        let (completion, _held) = ring.pop_within(REGISTRATION_TIMEOUT)?.ok_or_else(|| {
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
            outstanding: 0,
            contract: RingContract::new(),
            next_sequence: 0,
            next_offset: 0,
        })
    }

    /// This appender's conservation record, for a caller to assert against at
    /// teardown.
    ///
    /// The field is back, and the hazard it once carried is worth restating
    /// rather than deleting. `Pending::checked()` used to drive this oracle,
    /// and the draft before that kept a `RingContract` field *beside* the map
    /// and never wrote to it again -- so `assert_quiescent()` passed
    /// **vacuously**, against an oracle that had observed nothing. With
    /// `Pending` retired the field is the only holder again, so the property
    /// that keeps it honest is local and checkable: every push in
    /// [`Appender::append_batch`] observes, and every completion in
    /// [`Appender::claim`] observes. `append/tests.rs` asserts the counts move,
    /// which is what makes a silently-unwritten oracle fail rather than pass.
    pub fn contract(&self) -> &RingContract {
        &self.contract
    }

    /// The sequence the next appended record will carry.
    pub fn next_sequence(&self) -> Sequence {
        Sequence(self.next_sequence)
    }

    /// How many appends are pushed but not yet observed complete.
    pub fn in_flight(&self) -> usize {
        self.outstanding
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
        ring: &mut AppendRing,
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
            let buffer = self.arena.get_mut(slot)?;
            record::encode_block(buffer, sequence, epoch, payload)?;

            // Push over the whole stride rather than the record's own length.
            //
            // This reverses the decision this line used to carry -- "writing
            // the slot's unused tail would put stale bytes in the log and cost
            // real device bandwidth". The stale-bytes half is handled by
            // `encode_block`, which zeroes the remainder of the block. The
            // bandwidth half was correct and is simply the price: see
            // `record::RECORD_STRIDE` for what it costs and why a log pays it.
            let span = RegisteredSpan {
                buffer_index: slot,
                offset: 0,
                len: u32::try_from(record::RECORD_STRIDE).map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "record stride exceeds u32::MAX",
                    )
                })?,
            };
            let offset = self.next_offset;
            // SAFETY: `file` is the log's own handle and outlives every
            // operation pushed here -- the log drains to empty before it
            // closes. The ring holds the slot's registration lease until its
            // completion is popped, so the slot cannot be refilled underneath
            // the kernel.
            //
            // `PushOptions::new()` deliberately carries no barrier: records
            // stream unordered within an epoch, exactly as the contract says,
            // and the ordering that matters is bought once by the epoch's
            // covering flush. `WriteCaching::Cached` for the same reason --
            // write-through here would shape latency without changing what is
            // durable.
            let id = unsafe {
                batch.write_registered_raw_owned(
                    file,
                    &self.arena,
                    span,
                    slot,
                    offset,
                    PushOptions::new(),
                    WriteCaching::Cached,
                )
            }?;

            self.contract.observe_push(id.user_data());
            self.outstanding += 1;
            self.next_sequence += 1;
            self.next_offset += record::RECORD_STRIDE as u64;
            accepted += 1;
        }

        // One submission for the whole batch. This is the line the item
        // existed for.
        batch.submit()?;
        Ok(accepted)
    }

    /// Account for one popped completion that belongs to an append.
    ///
    /// `slot` is the sidecar the ring returned with the completion, which is
    /// also what identifies the completion as an append -- the caller reads it
    /// from the pop rather than offering the completion to each claimant in
    /// turn.
    ///
    /// The ordering hazard this used to carry is gone, and it is worth saying
    /// what it was: claiming had to happen *before* the write's result was
    /// inspected, because an early return on a failed write dropped the token
    /// unclaimed, which `Token` treated as still-outstanding. That burned the
    /// arena slot permanently -- `free_slots` never offered it again, and
    /// after `SLOTS` failures every append returned `WouldBlock` forever.
    /// `M22.2` found exactly that bug here, and `M25.1` made it loud rather
    /// than silent. The pop releases the slot now, before this runs, so there
    /// is no order left to invert.
    pub fn claim(
        &mut self,
        completion: &windows_ioring_sys::Completion,
        slot: u32,
    ) -> io::Result<()> {
        self.contract.observe_completion(completion.user_data());
        self.outstanding -= 1;
        debug_assert!(
            self.arena.outstanding(slot) == Some(0),
            "the pop must release the slot"
        );

        // The contract requires that a successful write of N bytes transferred
        // N bytes (see `crate::contract`, `Clause::Requires`), and this is
        // where that requirement is checked rather than assumed. Every record
        // write asks for exactly `RECORD_STRIDE`, so the comparison needs no
        // per-slot bookkeeping.
        //
        // The ring itself permits a short count -- `RS-P-8` in the crate's
        // RESPONSE-SPACE.md -- because it never asks what kind of handle it was
        // given. This log narrows that by requiring a handle which does not do
        // it, so a short count here is a violation of the contract's
        // requirement rather than a case to absorb, and is reported as one. The
        // count was previously bound to `_written` and discarded, which left
        // the requirement stated nowhere and checked nowhere.
        let written = completion.result()?;
        if written != record::RECORD_STRIDE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "a successful write transferred {written} of {} bytes; this log requires a \
                     handle whose successful writes are complete (see the contract's Requires \
                     clause), and this handle does not meet that requirement",
                    record::RECORD_STRIDE
                ),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
