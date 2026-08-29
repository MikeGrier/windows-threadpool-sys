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

use std::collections::HashMap;
use std::io;
use std::os::windows::io::RawHandle;

use windows_ioring_sys::{
    Batch, IoRing, PushOptions, RegisteredBuffers, RegisteredSpan, RegisteredUse, Token,
    WriteCaching,
};

use crate::record::{self, Sequence};

/// How many slots the arena holds. More slots means more records can be in
/// flight before the appender has to wait for one to come back.
pub const SLOTS: u32 = 8;

/// Bytes per slot, and so the largest record this log accepts.
pub const SLOT_LEN: usize = 4096;

/// One in-flight append: the token that holds the arena slot, and where the
/// record was written.
struct InFlight {
    token: Token<RegisteredUse>,
    slot: u32,
}

/// The append path: an arena of registered buffers, a monotonic sequence
/// counter, and the file offset the next record lands at.
pub struct Appender {
    arena: RegisteredBuffers<Vec<u8>>,
    in_flight: HashMap<usize, InFlight>,
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
    /// # Errors
    ///
    /// Any error from the registration push, the submit, or the registration
    /// operation itself.
    pub fn new(ring: &mut IoRing) -> io::Result<Self> {
        let buffers = (0..SLOTS).map(|_| vec![0_u8; SLOT_LEN]).collect::<Vec<_>>();
        let mut batch = Batch::new(ring);
        let pending = batch.register_buffers(buffers)?;
        batch.submit_and_wait(1, 30_000)?;

        let completion = loop {
            if let Some(completion) = ring.try_pop()? {
                break completion;
            }
        };
        let arena = pending
            .claim_if(&completion)
            .map_err(|_| {
                io::Error::other("the first completion was not the buffer registration's")
            })?
            .map_err(|error| io::Error::other(format!("buffer registration failed: {error}")))?;

        Ok(Self {
            arena,
            in_flight: HashMap::new(),
            next_sequence: 0,
            next_offset: 0,
        })
    }

    /// The sequence the next appended record will carry.
    pub fn next_sequence(&self) -> Sequence {
        Sequence(self.next_sequence)
    }

    /// How many appends are pushed but not yet observed complete.
    pub fn in_flight(&self) -> usize {
        self.in_flight.len()
    }

    /// A slot with no operation outstanding against it, or `None` if every
    /// slot is busy.
    ///
    /// Asked rather than assumed: the arena is the reason an append can block
    /// at all, and a caller that gets `None` should drain a completion and try
    /// again rather than grow the arena.
    fn free_slot(&self) -> Option<u32> {
        (0..self.arena.len()).find(|&slot| self.arena.outstanding(slot) == Some(0))
    }

    /// Compose `payload` into a free arena slot and push the write.
    ///
    /// Returns the record's [`Sequence`], which is its identity for the rest
    /// of its life -- the epoch bookkeeping in M13.3 keys off it.
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::WouldBlock`] if every arena slot is still in flight:
    /// the caller must drain a completion before appending again. Otherwise
    /// any error from encoding the record (notably if it does not fit a slot)
    /// or from the push.
    pub fn append(
        &mut self,
        ring: &mut IoRing,
        file: RawHandle,
        payload: &[u8],
    ) -> io::Result<Sequence> {
        let slot = self.free_slot().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::WouldBlock,
                "every arena slot has an append in flight; drain a completion first",
            )
        })?;
        let sequence = Sequence(self.next_sequence);

        // Step 1: compose. `get_mut` is what makes this possible at all, and
        // it is also the check that the kernel is not reading this slot.
        let total = record::encode(self.arena.get_mut(slot)?, sequence, payload)?;

        // Step 2: push, over exactly the bytes the record occupies rather than
        // the whole slot -- writing the slot's unused tail would put stale
        // bytes in the log and cost real device bandwidth.
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
        let mut batch = Batch::new(ring);
        // SAFETY: `file` is the log's own handle and outlives every operation
        // pushed here -- the log drains to empty before it closes. The token
        // is held in `in_flight` until its completion is observed, so the
        // arena slot it names cannot be refilled underneath the kernel.
        //
        // `PushOptions::new()` deliberately carries no barrier: records stream
        // unordered within an epoch, exactly as the contract says, and the
        // ordering that matters is bought once by the epoch's covering flush.
        // `WriteCaching::Cached` for the same reason -- write-through here
        // would shape latency without changing what is durable.
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
        batch.submit()?;

        self.in_flight.insert(token.id(), InFlight { token, slot });
        self.next_sequence += 1;
        self.next_offset += total as u64;
        Ok(sequence)
    }

    /// Account for one popped completion that belongs to an append.
    ///
    /// Returns `true` if `completion` was one of ours. Claiming the token is
    /// what returns its arena slot to the free pool, so a caller that drops
    /// completions on the floor will run the arena dry and never recover --
    /// which is the same drain-to-empty discipline the ring itself demands.
    pub fn claim(&mut self, completion: &windows_ioring_sys::Completion) -> io::Result<bool> {
        let Some(in_flight) = self.in_flight.remove(&completion.user_data()) else {
            return Ok(false);
        };
        let written = completion.result()?;
        let released = in_flight
            .token
            .claim_if(completion)
            .map_err(|_| io::Error::other("an append token refused its own completion"))?;
        // Dropping the marker is what decrements the slot's count, so it has
        // to happen before the check below rather than at end of scope.
        drop(released);
        debug_assert!(
            self.arena.outstanding(in_flight.slot) == Some(0),
            "claiming the token must release the slot"
        );
        let _ = written;
        Ok(true)
    }
}
