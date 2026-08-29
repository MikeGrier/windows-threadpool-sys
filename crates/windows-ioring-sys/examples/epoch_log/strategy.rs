// Copyright (c) 2026 Mike Grier
//! The three epoch-commit strategies (M14.3), behind one interface.
//!
//! [`crate::commit`] picks the simplest of the three and says so. This module
//! implements all three and lets a caller choose at run time, because
//! [D-24](../../DESIGN-NOTES.md) makes the choice a real fork with no free
//! answer, and a reader needs to see all three side by side to make it.
//!
//! # The fork
//!
//! An epoch's commit has to establish one thing: *every write in this epoch has
//! reached the device before the flush that reports it completes.* There are
//! exactly three places that ordering can be bought, and each charges
//! differently.
//!
//! ## 1. [`CommitStrategy::CoveringFlush`] -- buy it in the ring
//!
//! Push the flush with [`FlushCoverage::CoversPrecedingOperations`]. The ring
//! holds it until everything outstanding completes, and holds everything
//! pushed after it until *it* completes.
//!
//! Cheapest to write and easiest to see correct, and the reason is that the
//! ordering is a property of the submission rather than of any code that runs.
//! What it costs is that the barrier is **ring-wide** (D-24): appends for the
//! next epoch queue behind it, and their arena slots stay occupied. On a log
//! whose commits are rare relative to its appends, that stall is bounded and
//! fine. On one that commits often, it is the dominant cost.
//!
//! ## 2. [`CommitStrategy::HostSequenced`] -- buy it in userspace
//!
//! Submit the epoch's writes, wait in the host until every one of their
//! completions has been observed, and only then push an *unordered* flush.
//! The flush cannot race writes that have already completed, so the ordering
//! holds without asking the ring for anything.
//!
//! What it costs is a round trip that the ring would otherwise have absorbed:
//! the log thread must actually observe N completions before it may push the
//! flush, so the pipeline drains at every epoch boundary whether or not the
//! device needed it to. It also gives up overlap that the covering flush kept
//! -- with a barrier, the flush is *already queued* when the last write
//! completes; here it has not even been pushed yet.
//!
//! ## 3. [`CommitStrategy::AlternatingRings`] -- buy it in a second ring
//!
//! Two rings. Epoch *N* lives entirely on ring *N mod 2*, and its commit is a
//! covering flush on that ring -- so the barrier is real, but it stalls only
//! the ring being committed. Epoch *N+1*'s appends go to the other ring and
//! proceed while that barrier is held.
//!
//! Neither the ring-wide stall nor the host round trip. What it costs is
//! **doubled registration**: the arena is registered on both rings, and an
//! `IoRing` has no unregister call, so those registrations live for the rings'
//! whole lives. It also doubles the completion sources a wait must service,
//! which is not free for a program with a multiplexed loop.
//!
//! # What this module deliberately does not decide
//!
//! Which one is right. That depends on commit frequency, record size, arena
//! pressure, and the device -- none of which this crate knows. M14.4 measures
//! all three on the running machine, which is the only honest way to answer it.

use std::io;
use std::os::windows::io::RawHandle;

use windows_ioring_sys::{
    Batch, FlushCoverage, FlushMode, IoRing, PushOptions, RegisteredBuffers, RegisteredSpan,
    RegisteredUse, Token, WriteCaching,
};

use crate::commit::Epoch;
use crate::record::{self, Sequence};

/// Arena slots per ring. Small enough that a commit's stall is visible in
/// arena pressure, which is the cost strategy 1 is being charged for.
const SLOTS: u32 = 8;

/// Bytes per slot.
const SLOT_LEN: usize = 4096;

/// Bound on any wait, so a stuck strategy fails instead of hanging.
const WAIT_MS: u32 = 30_000;

/// How an epoch's commit establishes that its writes reached the device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommitStrategy {
    /// A covering flush: the ring holds the flush until everything
    /// outstanding completes. One ring, ring-wide stall.
    CoveringFlush,
    /// Wait in userspace for every write's completion, then push an unordered
    /// flush. One ring, no barrier, one host round trip per epoch.
    HostSequenced,
    /// Two rings, epochs alternating between them, each committed with a
    /// covering flush on its own ring. No ring-wide stall of the ring taking
    /// new appends, at the cost of registering the arena twice.
    AlternatingRings,
}

impl CommitStrategy {
    /// Every strategy, in the order the module documents them.
    pub const ALL: [Self; 3] = [
        Self::CoveringFlush,
        Self::HostSequenced,
        Self::AlternatingRings,
    ];

    /// A short name for reports.
    pub fn name(self) -> &'static str {
        match self {
            Self::CoveringFlush => "covering-flush",
            Self::HostSequenced => "host-sequenced",
            Self::AlternatingRings => "alternating-rings",
        }
    }

    /// What this strategy pays, in one line.
    pub fn cost(self) -> &'static str {
        match self {
            Self::CoveringFlush => "ring-wide stall at every commit",
            Self::HostSequenced => "a host round trip at every epoch boundary",
            Self::AlternatingRings => "the arena registered on both rings, permanently",
        }
    }

    /// How many rings this strategy needs.
    fn rings(self) -> usize {
        match self {
            Self::CoveringFlush | Self::HostSequenced => 1,
            Self::AlternatingRings => 2,
        }
    }
}

/// What one strategy run produced.
pub struct Outcome {
    pub strategy: CommitStrategy,
    /// Records appended, all of them inside committed epochs.
    pub records: usize,
    /// The highest epoch observed durable.
    pub durable_through: Epoch,
    /// Bytes the run wrote, so a reader can check the three wrote the same log.
    pub bytes: u64,
}

/// One ring plus the arena registered on it.
///
/// The arena is a separate registration per ring, which is the doubled cost
/// [`CommitStrategy::AlternatingRings`] is charged for -- and it is doubled
/// permanently, because `IoRing` has no unregister call.
struct Lane {
    ring: IoRing,
    arena: RegisteredBuffers<Vec<u8>>,
    /// `UserData` of an in-flight write -> its token and the slot it reads
    /// from. The token must be *claimed* on completion: dropping it unclaimed
    /// is treated as still-outstanding and leaks the slot forever.
    in_flight: std::collections::HashMap<usize, (Token<RegisteredUse>, u32)>,
    free: Vec<u32>,
    /// Completions that were not writes, kept by `UserData` for the commit
    /// path to match against.
    flushes: std::collections::HashMap<usize, io::Result<()>>,
}

impl Lane {
    fn new() -> io::Result<Self> {
        let mut ring = IoRing::new(64, 128)?;
        let buffers: Vec<Vec<u8>> = (0..SLOTS).map(|_| vec![0u8; SLOT_LEN]).collect();
        let mut batch = Batch::new(&mut ring);
        let pending = batch.register_buffers(buffers)?;
        batch.submit_and_wait(1, WAIT_MS)?;
        let completion = ring
            .try_pop()?
            .ok_or_else(|| io::Error::other("buffer registration produced no completion"))?;
        let arena = pending
            .claim_if(&completion)
            .map_err(|_| io::Error::other("registration token refused its own completion"))??;
        Ok(Self {
            ring,
            arena,
            in_flight: std::collections::HashMap::new(),
            free: (0..SLOTS).collect(),
            flushes: std::collections::HashMap::new(),
        })
    }

    #[expect(
        dead_code,
        reason = "M14.4 measures ring idle time, which is exactly this count reaching zero"
    )]
    fn outstanding(&self) -> usize {
        self.in_flight.len()
    }

    /// Compose one record into a free slot and push its write.
    ///
    /// Returns `None` when every slot is busy, which is the caller's cue to
    /// drain.
    fn append(
        &mut self,
        file: RawHandle,
        sequence: Sequence,
        epoch: Epoch,
        payload: &[u8],
        offset: u64,
    ) -> io::Result<Option<u64>> {
        let Some(slot) = self.free.pop() else {
            return Ok(None);
        };
        let total = {
            let bytes = self.arena.get_mut(slot)?;
            record::encode(bytes, sequence, epoch, payload)?
        };
        // Exactly the bytes the record occupies, not the whole slot: writing
        // the slot's unused tail would put stale bytes in the log and cost
        // real device bandwidth.
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
        let mut batch = Batch::new(&mut self.ring);
        // SAFETY: `file` outlives every operation pushed here -- the caller
        // drains to empty before closing it -- and the token is held in
        // `in_flight` until its completion is observed, so the slot cannot be
        // refilled underneath the kernel.
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
        self.in_flight.insert(token.id(), (token, slot));
        Ok(Some(total as u64))
    }

    /// Push this lane's commit flush and return its `UserData`.
    fn commit(&mut self, file: RawHandle, coverage: FlushCoverage) -> io::Result<usize> {
        let mut batch = Batch::new(&mut self.ring);
        // SAFETY: as `append`'s.
        let user_data = unsafe { batch.flush_raw(file, coverage, FlushMode::Default) }?;
        batch.submit()?;
        Ok(user_data)
    }

    /// Pop every completion currently available.
    ///
    /// Claiming an append's token is what returns its slot; a caller that
    /// drops completions runs the arena dry and never recovers.
    fn drain(&mut self) -> io::Result<usize> {
        let mut popped = 0;
        while let Some(completion) = self.ring.try_pop()? {
            popped += 1;
            if let Some((token, slot)) = self.in_flight.remove(&completion.user_data()) {
                // Claimed before the result is checked, for the reason
                // `Appender::claim` spells out: bailing out first would drop
                // the token unclaimed and burn the slot permanently.
                let released = token
                    .claim_if(&completion)
                    .map_err(|_| io::Error::other("a write token refused its own completion"))?;
                drop(released);
                self.free.push(slot);
                completion.result()?;
            } else {
                self.flushes
                    .insert(completion.user_data(), completion.result().map(|_| ()));
            }
        }
        Ok(popped)
    }

    /// Block until `user_data`'s flush has completed, draining as we go.
    fn await_flush(&mut self, user_data: usize) -> io::Result<()> {
        loop {
            if let Some(result) = self.flushes.remove(&user_data) {
                result?;
                return Ok(());
            }
            if self.drain()? == 0 {
                Batch::new(&mut self.ring).submit_and_wait(1, WAIT_MS)?;
            }
        }
    }

    /// Block until every write pushed on this lane has completed.
    ///
    /// This is [`CommitStrategy::HostSequenced`]'s whole mechanism, and the
    /// round trip it is charged for.
    fn await_writes(&mut self) -> io::Result<()> {
        while !self.in_flight.is_empty() {
            if self.drain()? == 0 {
                Batch::new(&mut self.ring).submit_and_wait(1, WAIT_MS)?;
            }
        }
        Ok(())
    }
}

/// Run `epochs` epochs of `records_per_epoch` records under `strategy`.
///
/// Every epoch is committed and its commit awaited, so the returned outcome's
/// `durable_through` is the truth rather than a hope. All three strategies
/// produce the same log: same records, same order, same bytes -- which is the
/// point. They differ only in what the commit costs.
///
/// # Errors
///
/// Any error from ring setup, a push, a submit, or an operation's result.
pub fn run(
    strategy: CommitStrategy,
    file: RawHandle,
    epochs: usize,
    records_per_epoch: usize,
    payload: &[u8],
) -> io::Result<Outcome> {
    let mut lanes = Vec::with_capacity(strategy.rings());
    for _ in 0..strategy.rings() {
        lanes.push(Lane::new()?);
    }

    let mut offset = 0u64;
    let mut sequence = 0u64;
    let mut records = 0usize;
    // The commit that is pushed but not yet awaited. Only
    // `AlternatingRings` ever carries one across an epoch boundary; that
    // deferral *is* the overlap it buys.
    let mut deferred: Option<(usize, usize)> = None;

    for epoch in 0..epochs as u64 {
        let lane_index = if strategy == CommitStrategy::AlternatingRings {
            (epoch as usize) % lanes.len()
        } else {
            0
        };

        // Before appending into this lane, settle any commit it still owes.
        // For the single-ring strategies there is never one outstanding; for
        // alternating rings this is where the previous use of *this* lane is
        // reconciled, one full epoch later than it was pushed.
        if let Some((owed_lane, user_data)) = deferred
            && owed_lane == lane_index
        {
            lanes[owed_lane].await_flush(user_data)?;
            deferred = None;
        }

        for _ in 0..records_per_epoch {
            loop {
                let lane = &mut lanes[lane_index];
                match lane.append(file, Sequence(sequence), Epoch(epoch), payload, offset)? {
                    Some(written) => {
                        offset += written;
                        sequence += 1;
                        records += 1;
                        break;
                    }
                    // Every slot is busy. Drain and retry -- the arena
                    // working as intended, and the pressure a ring-wide
                    // stall makes worse.
                    None => {
                        if lane.drain()? == 0 {
                            Batch::new(&mut lane.ring).submit_and_wait(1, WAIT_MS)?;
                        }
                    }
                }
            }
        }

        match strategy {
            CommitStrategy::CoveringFlush => {
                let user_data = lanes[0].commit(file, FlushCoverage::CoversPrecedingOperations)?;
                lanes[0].await_flush(user_data)?;
            }
            CommitStrategy::HostSequenced => {
                // The round trip: every write observed complete *before* the
                // flush is even pushed. That is what makes an unordered flush
                // sufficient here -- and what the pipeline pays for it.
                lanes[0].await_writes()?;
                let user_data = lanes[0].commit(file, FlushCoverage::Unordered)?;
                lanes[0].await_flush(user_data)?;
            }
            CommitStrategy::AlternatingRings => {
                // Push the barrier and walk away. The next epoch appends into
                // the *other* lane, so this stall overlaps with real work
                // instead of blocking it. The debt is settled at the top of
                // the next iteration that lands back on this lane.
                let user_data =
                    lanes[lane_index].commit(file, FlushCoverage::CoversPrecedingOperations)?;
                deferred = Some((lane_index, user_data));
            }
        }
    }

    // Settle the last outstanding commit, or `durable_through` below would be
    // a claim rather than an observation.
    if let Some((lane_index, user_data)) = deferred {
        lanes[lane_index].await_flush(user_data)?;
    }
    for lane in &mut lanes {
        lane.await_writes()?;
    }

    Ok(Outcome {
        strategy,
        records,
        durable_through: Epoch(epochs as u64 - 1),
        bytes: offset,
    })
}
