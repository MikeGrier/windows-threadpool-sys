// Copyright (c) 2026 Mike Grier
//! The three epoch-commit strategies (M14.3), behind one interface.
//!
//! [`crate::commit`] picks the simplest of the three and says so. This module
//! implements all three and lets a caller choose at run time, because
//! [D-24](../../DESIGN-NOTES.md#d-24) makes the choice a real fork with no free
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
//! holds it until everything outstanding completes. It does **not** hold back
//! what is pushed after it (D-47); an earlier version of this section said it
//! did, and that was the false contract this example used to teach.
//!
//! Cheapest to write and easiest to see correct, and the reason is that the
//! ordering is a property of the submission rather than of any code that runs.
//! What it costs is that the wait is **ring-wide**: the flush waits on every
//! operation outstanding on the ring when it is reached, however unrelated to
//! this epoch, so it takes as long as the slowest of them. The epoch's own
//! arena slots stay occupied for that whole time. On a log whose commits are
//! rare relative to its appends, that is bounded and fine. On one that commits
//! often, it is the dominant cost.
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
//! covering flush on that ring. Epoch *N+1*'s appends go to the other ring, so
//! they are unambiguously outside the epoch being committed -- which is what
//! this buys, now that D-47 has established the ring does not hold later work
//! back anyway.
//!
//! Neither a long commit on the appending ring nor the host round trip. What it costs is
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
//!
//! # What the measurement found here, and why it is worth saying
//!
//! On the machine this was written on, the three are **indistinguishable**:
//! the throughput spread across strategies is the same size as the spread one
//! strategy shows between consecutive runs. The reason is visible in the
//! numbers the sample prints. Every strategy pays exactly one device flush per
//! epoch, that flush costs hundreds of microseconds, and everything the
//! strategies actually differ about -- how long the flush itself waits, the
//! extra host round trip -- lands in the tens.
//!
//! The distinction [D-24](../../DESIGN-NOTES.md#d-24) draws is real. It is
//! simply two orders of magnitude below the dominant term at this workload,
//! and a reader is better served by knowing that than by a ranking that would
//! not reproduce.
//!
//! ## The conclusion survives; the explanation above does not (M20.6)
//!
//! "Indistinguishable" still holds, and `M22.1` re-measured it after removing
//! a shared per-record cost that could have flattened it. What does not hold
//! is the *mechanism* this section gives for it. It says the strategies differ
//! about "how long the flush itself waits" and "the extra host round trip",
//! and that those land in the tens -- but on this sample's handle **none of
//! those differences can occur at all**.
//!
//! The handle carries no `FILE_FLAG_OVERLAPPED`, so a ring operation completes
//! inline during `SubmitIoRing`: the commit's submit takes hundreds of
//! microseconds and returns with every completion already queued. Nothing is
//! ever outstanding across a submit boundary, so there is no overlap for the
//! strategies to differ in, and the comment further down claiming "a real log
//! keeps appending while a commit is outstanding" describes something this
//! program cannot do.
//!
//! So the three are indistinguishable *because they are doing the same
//! serialized work*, not because a shared dominant term swamps real
//! differences. Both readings give the same ranking and only one is true.
//!
//! **What this does not settle is whether `AlternatingRings` earns its place.**
//! Its blast-radius justification is dead on structural grounds -- see
//! [`CommitStrategy::AlternatingRings`] -- but the other thing two rings could
//! buy is *overlap*, and overlap is precisely what this harness cannot
//! exhibit. Removing it now would be deciding against it using a measurement
//! that could not have shown it working. `M25` rebuilds the harness on a
//! pre-allocated unbuffered log where operations genuinely pend; `M25.5`
//! re-runs this comparison and answers the question on numbers that mean what
//! they say.
//!
//! Getting that result required fixing the harness three times, which is worth
//! recording because the mistakes are easy to make and none announces itself:
//!
//! - The first version awaited each commit before appending the next epoch.
//!   That serialises every strategy, so it measured a workload no real log
//!   runs. A real log keeps appending while a commit is outstanding, and the
//!   strategies differ in what that overlap costs -- registration, a host round
//!   trip, or nothing. (The original reason given here was that a covering
//!   flush *holds back* those appends. It does not; see D-47. Keeping the
//!   overlap is still right, but the comparison it produces should be re-read
//!   with that correction in mind -- see M20.6.)
//!
//!   **And `M20.6` found the deeper version of the same error.** Removing the
//!   serialisation was necessary and not sufficient: on a synchronous handle
//!   there is no overlap to restore, because the work is already done when
//!   submit returns. This fix made the harness *able* to overlap and the
//!   platform still does not, so what the sentence above describes as the
//!   corrected state has never actually run. `M25` is what makes it true.
//! - The second version keyed pending commits by `UserData` in one map across
//!   both rings. Each ring assigns its own sequence, so the two collided and
//!   half the samples vanished.
//! - The third submitted **one write per record**, which made the per-record
//!   submission cost a term every strategy paid equally -- exactly the shape of
//!   shared constant that can flatten a comparison into "indistinguishable"
//!   without the underlying claim being true. Both append paths now batch, and
//!   the confound was measured rather than argued away: twenty runs, ten each
//!   side, in
//!   [measurements/2026-09-22-append-batching/](../../measurements/2026-09-22-append-batching/).
//!   **The conclusion above survives it** -- removing the shared cost left the
//!   cross-strategy spread inside a single strategy's own run-to-run range.
//!   What batching did move is commit latency, which is the half of the system
//!   the device flush does not dominate.

use std::io;
use std::os::windows::io::RawHandle;
use std::time::{Duration, Instant};

use windows_ioring_sys::{
    Batch, Completion, FlushCoverage, FlushMode, IoRing, NumaBuffer, PushOptions,
    RegisteredBuffers, RegisteredSpan, RegisteredUse, Token, WriteCaching,
};

use crate::append::free_slots;
use crate::commit::Epoch;
use crate::record::{self, Sequence};

/// Arena slots per ring. Small enough that a commit's own duration is visible
/// in arena pressure, which is the cost strategy 1 is being charged for.
const SLOTS: u32 = 8;

/// Bytes per slot.
const SLOT_LEN: usize = 4096;

/// Bound on any wait, so a stuck strategy fails instead of hanging.
const WAIT_MS: u32 = 30_000;

/// The same bound as a [`Duration`], derived from `WAIT_MS` rather than
/// written twice so the two cannot drift.
const WAIT: Duration = Duration::from_millis(WAIT_MS as u64);

/// How an epoch's commit establishes that its writes reached the device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommitStrategy {
    /// A covering flush: the flush waits until everything outstanding
    /// completes. One ring; the commit is long, but later appends are not
    /// blocked by the ring (D-47) -- this sample serialises them itself.
    CoveringFlush,
    /// Wait in userspace for every write's completion, then push an unordered
    /// flush. One ring, no barrier, one host round trip per epoch.
    HostSequenced,
    /// Two rings, epochs alternating between them, each committed with a
    /// covering flush on its own ring, so the appending ring is never the one
    /// waiting, at the cost of registering the arena twice.
    ///
    /// # Its blast-radius justification is dead, on structural grounds (M20.6)
    ///
    /// The argument for two rings was that a covering flush reaches *every*
    /// operation outstanding on its ring ([D-47](../../DESIGN-NOTES.md#d-47)
    /// withdrew the hold-back half and kept this one), so alternating bounds
    /// what a commit's barrier can be dragged into. On a shared ring, the
    /// reasoning went, commit latency is unbounded in unrelated traffic.
    ///
    /// Not here, and it needs no measurement to see why.
    /// `RegisteredBuffers::get_mut` refuses a slot with an operation
    /// outstanding, and there are `SLOTS` slots -- so at most `SLOTS` appends
    /// are outstanding on a ring **by construction**. Each alternating lane
    /// registers its own arena of the same size, so the per-ring bound is
    /// identical either way. The arena bounds the blast radius, not the ring
    /// topology. (Probing agreed: 8 and 8. The argument does not rest on that,
    /// and holds whatever the platform does about pending.)
    ///
    /// The argument would still apply against genuinely unrelated traffic from
    /// another component with its own buffers. This sample has none.
    ///
    /// # What is still open
    ///
    /// Overlap. Two rings let one ring's appends proceed while the other's
    /// flush is outstanding, and that is a real thing to buy -- but this
    /// harness cannot exhibit it, because its handle is synchronous and
    /// nothing is ever outstanding across a submit boundary. It is deliberately
    /// **not** removed on the strength of a measurement that could not have
    /// shown it working; `M25.5` answers that on a harness where operations
    /// genuinely pend.
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
            Self::CoveringFlush => "a long, ring-wide wait at every commit",
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
    /// Wall clock for the whole run, appends and commits together.
    pub elapsed: Duration,
    /// Per epoch, from pushing the commit to observing its completion.
    ///
    /// Read this carefully, because it is **not** device flush time. Every
    /// strategy here defers its await by design, so the figure includes time
    /// the program spent doing useful work before it got around to asking --
    /// and a strategy that defers *further* therefore reports a *larger*
    /// number while being no slower. Alternating rings shows this most
    /// plainly: it defers across two epochs and reports the highest latency of
    /// the three while matching them on throughput.
    ///
    /// So compare [`Outcome::elapsed`] across strategies, and read this as
    /// "how stale is a commit acknowledgement by the time this design collects
    /// it" -- which is a real property, just not the one its name suggests.
    ///
    /// # Measured, and it is worse than "includes" (M20.6)
    ///
    /// The paragraphs above were right about the shape and understated the
    /// extent. Decomposing the figure into deferral (flush pushed -> harness
    /// next looked) and blocking (time actually waiting) gives **blocking p50
    /// and p99 of 0 us for all three strategies**: the harness never waits for
    /// a flush at all, so this is not "inflated by" deferral, it *is*
    /// deferral. What it measures is how long the next epoch's appends took.
    ///
    /// The cause is underneath the harness. The commit's `SubmitIoRing` takes
    /// hundreds of microseconds and returns with every completion already
    /// queued, because the sample's handle carries no `FILE_FLAG_OVERLAPPED`
    /// and a synchronous handle completes a ring operation inline. So the
    /// commit is already durable before this clock starts -- and the overlap
    /// the comparison is built on does not exist.
    ///
    /// `M25` rebuilds the harness on a pre-allocated unbuffered log, where a
    /// commit genuinely pends and this becomes a real measurement. Until then
    /// the printed column is labelled "ack lag" rather than "commit", because
    /// a reader of the *output* deserves what a reader of this doc gets.
    pub commit_latencies: Vec<Duration>,
    /// Time appends spent blocked because every arena slot was busy.
    ///
    /// This is where a long commit shows up as a number: an arena slot is not
    /// reusable until the operation holding it completes, and a covering flush
    /// does not complete until everything outstanding on its ring has. The
    /// stall is the epoch's own operations retiring, not -- as an earlier
    /// revision claimed -- the flush holding back what was pushed after it,
    /// which D-47 established it does not do.
    pub append_stall: Duration,
}

impl Outcome {
    /// Records per second over the whole run.
    pub fn throughput(&self) -> f64 {
        if self.elapsed.is_zero() {
            return 0.0;
        }
        self.records as f64 / self.elapsed.as_secs_f64()
    }

    /// Commit latency at `fraction` through the sorted distribution.
    ///
    /// Nearest-rank rather than interpolated: with tens of samples an
    /// interpolated quantile invents precision the data does not have.
    pub fn commit_quantile(&self, fraction: f64) -> Duration {
        if self.commit_latencies.is_empty() {
            return Duration::ZERO;
        }
        let mut sorted = self.commit_latencies.clone();
        sorted.sort_unstable();
        let rank = ((sorted.len() as f64 - 1.0) * fraction).round() as usize;
        sorted[rank.min(sorted.len() - 1)]
    }
}

/// One ring plus the arena registered on it.
///
/// The arena is a separate registration per ring, which is the doubled cost
/// [`CommitStrategy::AlternatingRings`] is charged for -- and it is doubled
/// permanently, because `IoRing` has no unregister call.
struct Lane {
    ring: IoRing,
    arena: RegisteredBuffers<NumaBuffer>,
    /// `UserData` of an in-flight write -> its token and the slot it reads
    /// from. The token must be *claimed* on completion: dropping it unclaimed
    /// is treated as still-outstanding and leaks the slot forever.
    in_flight: std::collections::HashMap<usize, (Token<RegisteredUse>, u32)>,
    /// Completions that were not writes, kept by `UserData` for the commit
    /// path to match against.
    flushes: std::collections::HashMap<usize, io::Result<()>>,
}

impl Lane {
    fn new(node: Option<u32>) -> io::Result<Self> {
        let mut ring = IoRing::new(64, 128)?;
        let buffers = (0..SLOTS)
            .map(|_| NumaBuffer::new(SLOT_LEN, node))
            .collect::<io::Result<Vec<_>>>()?;
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

    /// Compose as many records as there are free slots and push them all in
    /// **one** submission, starting at `sequence` and `offset`.
    ///
    /// Returns how many were accepted and how many bytes they occupy. Zero
    /// accepted is the caller's cue to drain, not an error.
    ///
    /// Batched for the reason [`crate::append::Appender::append_batch`] is:
    /// one `SubmitIoRing` per record made the per-record submission cost a
    /// term every strategy paid equally, which is exactly the kind of shared
    /// constant that flattens a comparison.
    ///
    /// Which slots are free is asked of [`free_slots`] -- the sample's single
    /// definition -- rather than tracked here. This lane used to keep its own
    /// free list; see that function for why the copy was not merely redundant.
    fn append_batch(
        &mut self,
        file: RawHandle,
        first_sequence: u64,
        epoch: Epoch,
        payload: &[u8],
        first_offset: u64,
        want: usize,
    ) -> io::Result<(usize, u64)> {
        let slots = free_slots(&self.arena, want);
        if slots.is_empty() {
            return Ok((0, 0));
        }

        let mut batch = Batch::new(&mut self.ring);
        let mut written = 0_u64;
        let mut accepted = 0;
        for (index, &slot) in slots.iter().enumerate() {
            let sequence = Sequence(first_sequence + index as u64);
            let total = {
                let bytes = self.arena.get_mut(slot)?;
                record::encode(bytes, sequence, epoch, payload)?
            };
            // Exactly the bytes the record occupies, not the whole slot:
            // writing the slot's unused tail would put stale bytes in the log
            // and cost real device bandwidth.
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
            // SAFETY: `file` outlives every operation pushed here -- the
            // caller drains to empty before closing it -- and the token is
            // held in `in_flight` until its completion is observed, so the
            // slot cannot be refilled underneath the kernel.
            let token = unsafe {
                batch.write_registered_raw(
                    file,
                    &self.arena,
                    span,
                    first_offset + written,
                    PushOptions::new(),
                    WriteCaching::Cached,
                )
            }?;
            self.in_flight.insert(token.id(), (token, slot));
            written += total as u64;
            accepted += 1;
        }
        batch.submit()?;
        Ok((accepted, written))
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
            self.classify(completion)?;
        }
        Ok(popped)
    }

    /// File one popped completion: an append returns its slot, anything else
    /// is a flush the commit path will match against.
    ///
    /// Factored out of [`Lane::drain`] so the bounded waits below can file a
    /// completion they blocked for without a second copy of this logic.
    fn classify(&mut self, completion: Completion) -> io::Result<()> {
        if let Some((token, slot)) = self.in_flight.remove(&completion.user_data()) {
            // Claimed before the result is checked, for the reason
            // `Appender::claim` spells out: bailing out first would drop
            // the token unclaimed and burn the slot permanently.
            let released = token
                .claim_if(&completion)
                .map_err(|_| io::Error::other("a write token refused its own completion"))?;
            // Dropping the marker is what decrements the slot's outstanding
            // count, and that count *is* the free list now -- so this drop,
            // not a push to a side table, is what returns the slot.
            drop(released);
            debug_assert!(
                self.arena.outstanding(slot) == Some(0),
                "claiming the token must release the slot"
            );
            completion.result()?;
        } else {
            self.flushes
                .insert(completion.user_data(), completion.result().map(|_| ()));
        }
        Ok(())
    }

    /// Block until `user_data`'s flush has completed, draining as we go.
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::TimedOut`] if the flush does not complete within
    /// [`WAIT`]. This loop used to have no bound at all: it blocked in
    /// `submit_and_wait` for `WAIT_MS`, ignored the fact that the call had
    /// returned without a completion, and went round again forever. A
    /// measurement harness that hangs reports nothing, which is strictly worse
    /// than one that fails -- the same reason
    /// [`EventLoop::pump`](crate::event_loop::EventLoop::pump) raises
    /// `TimedOut` rather than spinning.
    fn await_flush(&mut self, user_data: usize) -> io::Result<()> {
        let deadline = Instant::now() + WAIT;
        loop {
            if let Some(result) = self.flushes.remove(&user_data) {
                result?;
                return Ok(());
            }
            if self.drain()? == 0 {
                match self.ring.pop_within(remaining(deadline))? {
                    Some(completion) => self.classify(completion)?,
                    None => return Err(timed_out("a commit's flush")),
                }
            }
        }
    }

    /// Block until every write pushed on this lane has completed.
    ///
    /// This is [`CommitStrategy::HostSequenced`]'s whole mechanism, and the
    /// round trip it is charged for.
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::TimedOut`] if they do not all complete within
    /// [`WAIT`], for the reason given on [`Lane::await_flush`].
    fn await_writes(&mut self) -> io::Result<()> {
        let deadline = Instant::now() + WAIT;
        while !self.in_flight.is_empty() {
            if self.drain()? == 0 {
                match self.ring.pop_within(remaining(deadline))? {
                    Some(completion) => self.classify(completion)?,
                    None => return Err(timed_out("this lane's outstanding writes")),
                }
            }
        }
        Ok(())
    }
}

/// How long is left before `deadline`, saturating at zero.
fn remaining(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}

/// The one spelling of this harness's timeout failure, so the two waits above
/// cannot describe the same condition differently.
fn timed_out(what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::TimedOut,
        format!("timed out after {WAIT:?} waiting for {what}"),
    )
}

/// Run `epochs` epochs of `records_per_epoch` records under `strategy`.
///
/// Every epoch is committed and its commit awaited, so the returned outcome's
/// `durable_through` is the truth rather than a hope. All three strategies
/// produce the same log: same records, same order, same bytes -- which is the
/// point. They differ only in what the commit costs.
///
/// Every lane's arena is placed on `node`, the same node the log's own arena
/// uses, so placement is held constant across the comparison rather than being
/// one more thing the strategies differ in.
///
/// # Errors
///
/// Any error from ring setup, arena allocation, a push, a submit, or an
/// operation's result.
pub fn run(
    strategy: CommitStrategy,
    file: RawHandle,
    epochs: usize,
    records_per_epoch: usize,
    payload: &[u8],
    node: Option<u32>,
) -> io::Result<Outcome> {
    let mut lanes = Vec::with_capacity(strategy.rings());
    for _ in 0..strategy.rings() {
        lanes.push(Lane::new(node)?);
    }

    let mut offset = 0u64;
    let mut sequence = 0u64;
    let mut records = 0usize;
    let mut commit_latencies = Vec::with_capacity(epochs);
    let mut append_stall = Duration::ZERO;
    // Registration is deliberately outside the clock: it happens once, and
    // charging a strategy's throughput for it would say more about setup than
    // about the commit path this is comparing.
    let started = Instant::now();
    // Per lane, the commit that is pushed but not yet awaited. Only
    // `AlternatingRings` ever carries one across an epoch boundary; that
    // deferral *is* the overlap it buys.
    //
    // One slot **per lane**, not one slot: with a single slot, lane 0's
    // outstanding commit is overwritten the moment lane 1 pushes its own, and
    // is then never awaited -- so `durable_through` becomes a claim rather
    // than an observation. That was the shape of this code until M14.4's
    // measurement exposed it.
    //
    // Do not rely on the commit-latency count below to catch a regression
    // here: in this loop's current shape it stays at `epochs` even with a
    // shared slot, because every iteration still settles exactly one commit --
    // just not always the right one. Verified by re-running that sabotage. The
    // guard that does catch it belongs to the crate: an un-awaited commit is
    // still referencing the lane's registered arena when `Lane` drops, and the
    // drop panics.
    //
    // The push instant lives here too, rather than in a map keyed by
    // `UserData`. Each ring assigns its own `UserData` sequence, so the two
    // lanes hand out colliding values and a single map silently loses half the
    // samples -- which is exactly what the second attempt at this measured.
    let mut deferred: Vec<Option<(usize, Instant)>> = vec![None; lanes.len()];

    for epoch in 0..epochs as u64 {
        let lane_index = if strategy == CommitStrategy::AlternatingRings {
            (epoch as usize) % lanes.len()
        } else {
            0
        };

        let mut pushed_this_epoch = 0;
        while pushed_this_epoch < records_per_epoch {
            let lane = &mut lanes[lane_index];
            let (accepted, written) = lane.append_batch(
                file,
                sequence,
                Epoch(epoch),
                payload,
                offset,
                records_per_epoch - pushed_this_epoch,
            )?;
            if accepted == 0 {
                // Every slot is busy. Drain and retry -- the arena working as
                // intended, and the pressure a long commit makes worse by
                // holding its slots for the whole of its own duration. Timed,
                // because this is the cost a covering flush imposes on the
                // append path.
                let blocked = Instant::now();
                if lane.drain()? == 0 {
                    Batch::new(&mut lane.ring).submit_and_wait(1, WAIT_MS)?;
                }
                append_stall += blocked.elapsed();
                continue;
            }
            offset += written;
            sequence += accepted as u64;
            records += accepted;
            pushed_this_epoch += accepted;
        }

        // Settle this lane's previous commit *here*, after its next epoch's
        // appends have already been pushed, rather than before them.
        //
        // That placement is what makes the comparison mean anything. Settling
        // first would make every strategy serialise -- commit, wait, append,
        // commit -- which is a workload no real log runs. A real log keeps
        // appending while a commit is outstanding (the one in `main.rs` does).
        // An earlier revision of this harness settled first and measured a
        // difference indistinguishable from run-to-run noise.
        //
        // The original justification said those overlapping appends are held
        // back by a covering flush. They are not (D-47), so what the overlap
        // exposes is the strategies' other costs rather than a stall. The
        // placement stays; the conclusion drawn from the numbers needs
        // re-reading, which is `M20.6`.
        if let Some((user_data, pushed)) = deferred[lane_index].take() {
            lanes[lane_index].await_flush(user_data)?;
            commit_latencies.push(pushed.elapsed());
        }

        let coverage = match strategy {
            // The round trip: every write observed complete *before* the flush
            // is even pushed. That is what makes an unordered flush sufficient
            // here -- and what the pipeline pays for it.
            CommitStrategy::HostSequenced => {
                lanes[lane_index].await_writes()?;
                FlushCoverage::Unordered
            }
            CommitStrategy::CoveringFlush | CommitStrategy::AlternatingRings => {
                FlushCoverage::CoversPrecedingOperations
            }
        };
        let user_data = lanes[lane_index].commit(file, coverage)?;
        deferred[lane_index] = Some((user_data, Instant::now()));
    }

    // Settle the last outstanding commit, or `durable_through` below would be
    // a claim rather than an observation.
    for lane_index in 0..lanes.len() {
        if let Some((user_data, pushed)) = deferred[lane_index].take() {
            lanes[lane_index].await_flush(user_data)?;
            commit_latencies.push(pushed.elapsed());
        }
    }
    // One observation per epoch. Cheap, and it binds a real invariant -- but be
    // precise about which one, because this comment previously overclaimed.
    //
    // It fires when a settle is *skipped altogether*, which is how the original
    // per-lane bug showed up (16 of 32) in an earlier shape where the settle sat
    // at the top of the loop and a lane mismatch could pass it by. It does
    // **not** catch that bug in the current shape: with the settle immediately
    // before the push, every iteration settles exactly one commit, so restoring
    // the single shared `deferred` slot keeps this count at `epochs` while
    // settling the *wrong* commits -- some twice, some never. Measured, not
    // assumed: that sabotage was re-run against this code and this assertion
    // passed.
    //
    // What does catch it is the crate's own guard. A commit left un-awaited is
    // still referencing the lane's registered arena at drop, and `Lane`'s drop
    // panics with "RegisteredBuffers dropped while an operation still
    // references it". That is the load-bearing detector here, and it lives in
    // the layer that owns the invariant rather than in this harness.
    //
    // An earlier revision also carried a per-lane
    // `debug_assert!(deferred[lane].is_none())` just before the push. That one
    // could never fire at all -- the settle above it calls `Option::take`, which
    // empties the slot whether or not the `if let` binds -- so it was removed
    // rather than left to look like a guard.
    assert_eq!(
        commit_latencies.len(),
        epochs,
        "{} observed {} of {epochs} commits",
        strategy.name(),
        commit_latencies.len()
    );
    for lane in &mut lanes {
        lane.await_writes()?;
    }
    let elapsed = started.elapsed();

    Ok(Outcome {
        strategy,
        records,
        durable_through: Epoch(epochs as u64 - 1),
        bytes: offset,
        elapsed,
        commit_latencies,
        append_stall,
    })
}

#[cfg(test)]
mod tests;
