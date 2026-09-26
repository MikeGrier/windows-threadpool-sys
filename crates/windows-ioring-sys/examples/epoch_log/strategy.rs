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
//! On the machine this was written on: the spread in throughput and in total
//! commit cost **across** the three strategies is smaller than the spread one
//! strategy shows **between** consecutive runs. What does not vary between
//! runs is *where* each spends its commit -- `HostSequenced` in a host round
//! trip before the flush, the covering strategies inside the submit that
//! carries it. Fifteen runs, with the ranges beside the medians, are in
//! [measurements/2026-09-24-commit-decomposed/](../../measurements/2026-09-24-commit-decomposed/README.md).
//! Read the capture rather than this paragraph; nothing is quoted here that
//! would have to be kept true by hand.
//!
//! The distinction [D-24](../../DESIGN-NOTES.md#d-24) draws is real. Whether
//! it dominates a given workload is a question for that workload's own
//! numbers, which is why this sample prints its own instead of quoting ours.
//!
//! ## Two earlier explanations of that result were wrong (M20.6, M25.5)
//!
//! The spread relation above has held through every correction. The *reasons*
//! given for it did not, twice, and both are recorded because each looked
//! settled:
//!
//! **The first said the strategies differ only in things "two orders of
//! magnitude below" a dominant device flush** -- how long the flush waits, the
//! extra host round trip -- landing "in the tens" of microseconds. `M20.6`
//! found that none of those differences could occur at all: the handle carried
//! no `FILE_FLAG_OVERLAPPED`, so a ring operation completed inline during
//! `SubmitIoRing`, nothing was ever outstanding across a submit boundary, and
//! the comment further down claiming "a real log keeps appending while a commit
//! is outstanding" described something the program could not do. The three were
//! indistinguishable *because they were doing the same serialized work*. Both
//! readings gave the same ranking and only one was true.
//!
//! **The second was the figure itself.** The published commit latency was
//! entirely deferral -- how long the program went on appending before it got
//! around to asking -- so the strategy that defers furthest reported the worst
//! commit while being no slower.
//!
//! `M25.3` gave the handle the shape [the spike](../../design-sessions/spikes/write-pending-spike.rs)
//! measured as pending, `M25.4` split a commit's cost so the flush and the
//! deferral could not be read as each other, and `M25.5` re-ran the comparison
//! on those numbers. The capture linked above also records a correction it had
//! to make before it could answer: the commit clock started *after*
//! `HostSequenced`'s host round trip, which made that strategy look six times
//! cheaper than it is.
//!
//! One figure from the old explanation is now measured and is not what it said:
//! the strategies' differences land in the **hundreds** of microseconds, not
//! the tens. They remain smaller than the run-to-run spread. "Below the
//! run-to-run spread" and "two orders of magnitude below the flush" are
//! different claims, and the measurement supports only the first.
//!
//! **What this does not settle is whether `AlternatingRings` earns its place.**
//! This harness cannot show a blast-radius difference -- but that is a fact
//! about the harness, in which each lane's own arena is the limiter rather
//! than the ring topology, and not evidence that the strategy buys nothing.
//! See [`CommitStrategy::AlternatingRings`] for the conditions under which it
//! would, which include a ring shared with anything else, asymmetric arena
//! sizing, real overlap, and the per-CPU queue affinity
//! [D-27](../../DESIGN-NOTES.md#d-27) is built on. Removing it on the strength
//! of a measurement that could not have shown it working would be foreclosing
//! an option this crate exists to keep open. `M25` rebuilds the harness on a
//! pre-allocated unbuffered log where operations genuinely pend; `M25.5`
//! re-runs this comparison there.
//!
//! **And the point of the comparison is the instrument, not the verdict.** The
//! numbers below describe one machine, one device and one workload. A consumer
//! whose answer differs is not contradicting this sample -- they are the reason
//! it prints its numbers instead of quoting them.
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
//!   submit returns. That fix made the harness *able* to overlap while the
//!   handle still could not, so what the sentence above describes as the
//!   corrected state had never actually run.
//!
//!   **`M25.3` gave the handle the shape that can overlap, and `M25.5`
//!   measured what followed.** Every strategy's `block` reads zero at the
//!   median in every run -- so the harness still never waits for a flush. That
//!   is not evidence the operation completed inline: it defers by several
//!   milliseconds, and an operation that pended and finished during that window
//!   is indistinguishable from one that never pended. **The sentence above is
//!   now describable rather than demonstrated**, which is a smaller claim than
//!   it has ever carried before, and the honest one.
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
    RegisteredBuffers, RegisteredSpan, WriteCaching,
};

use win_numa_sys::NumaNode;

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
    /// # What this harness can and cannot show about it (M20.6)
    ///
    /// The argument for two rings was that a covering flush reaches *every*
    /// operation outstanding on its ring ([D-47](../../DESIGN-NOTES.md#d-47)
    /// withdrew the hold-back half and kept this one), so alternating bounds
    /// what a commit's barrier can be dragged into.
    ///
    /// **This sample cannot exhibit that difference, and the reason is the
    /// sample's own shape rather than anything about the strategy.**
    /// `RegisteredBuffers::get_mut` refuses a slot with an operation
    /// outstanding, and there are `SLOTS` slots, so at most `SLOTS` appends
    /// are outstanding on a ring by construction -- and each alternating lane
    /// registers its own arena of the same size. Here the arena is the
    /// limiter, not the ring topology, so the covered count is identical
    /// either way. (Probed: 8 and 8.)
    ///
    /// That is a statement about this apparatus. It is **not** evidence that
    /// alternating rings buys nothing, and the conditions under which it would
    /// are ordinary rather than exotic:
    ///
    /// - **A ring shared with anything else.** This sample owns its ring
    ///   entirely. A consumer whose ring also carries another component's
    ///   traffic has a barrier whose reach is bounded by that traffic, not by
    ///   this arena.
    /// - **Arenas sized differently from the lanes.** One large arena on a
    ///   shared ring against two small ones is a different bound, and nothing
    ///   makes the sample's symmetric choice the general case.
    /// - **Real overlap.** With operations that genuinely pend, a shared
    ///   ring's flush can be reached *after* the next epoch's appends are
    ///   queued, so the same covered count is not the same wait. This could not
    ///   happen at all while the handle was synchronous; `M25.3` removed that
    ///   obstacle, and whether operations actually pend remains an observation
    ///   about a machine rather than something this crate may assume.
    /// - **Per-CPU queue affinity.** [D-27](../../DESIGN-NOTES.md#d-27) is
    ///   this crate's decision that one ring per thread is userspace's proxy
    ///   for one ring per CPU, and records that NVMe queue pairs are per-CPU
    ///   with their completion interrupt routed by their own vector. Two rings
    ///   on two pinned threads is that architecture; one ring is not.
    ///
    /// So this strategy stays, and the sample's job is to let a consumer find
    /// out **on their own hardware and workload** rather than to hand them a
    /// verdict from ours. `M25.5` re-runs the comparison on a harness where
    /// operations genuinely pend, which removes the third condition above and
    /// makes the answer here mean more than it currently can.
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
    /// Per epoch, what its commit cost, decomposed (M25.4).
    ///
    /// This was a single `Duration` measured from pushing the flush to
    /// observing its completion, and `M20.6` established that the number was
    /// **entirely deferral**: blocking p50 *and* p99 were 0 us for all three
    /// strategies, so the published figure reported how long the next epoch's
    /// appends took rather than anything about the commit. A strategy that
    /// deferred further reported a larger number while being no slower.
    ///
    /// Splitting it is the fix, and the split is the point: the three parts
    /// cannot be confused for one another the way one blended number invited.
    /// See [`CommitTiming`].
    pub commit_timings: Vec<CommitTiming>,
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

/// What one epoch's commit cost, split so the parts cannot be read as each
/// other (M25.4).
///
/// The harness published a single blended number until `M20.6` decomposed it
/// and found it was entirely deferral. These three add up to that old number
/// and are reported separately for that reason.
///
/// # Every part stays meaningful whether or not the operation pends
///
/// `M25`'s standing constraint forbids anything here depending on an operation
/// pending, because Windows specifies nothing about when a ring operation
/// completes relative to `SubmitIoRing`. This split satisfies it by
/// construction rather than by assumption:
///
/// - if the flush completes **inline**, the device round trip lands in
///   [`submit`](Self::submit) and [`blocking`](Self::blocking) is zero;
/// - if it **pends**, `submit` is short and the wait shows up in `blocking`;
/// - either way [`deferral`](Self::deferral) is the program's own choice and
///   belongs to neither.
///
/// So the harness reports what it observed and never has to know which case it
/// got.
#[derive(Clone, Copy, Debug)]
pub struct CommitTiming {
    /// Wall time the strategy spent preparing before the flush could be
    /// pushed at all.
    ///
    /// Zero for the covering strategies, which push the flush immediately and
    /// let its coverage do the ordering. For `HostSequenced` it is the host
    /// round trip -- waiting for every write's completion in userspace, which
    /// is what makes an unordered flush sufficient for it.
    ///
    /// **This part exists because leaving it out inverted the comparison.**
    /// `M25.4` started the clock at the submit, which put that round trip
    /// outside every measured part; `HostSequenced` then reported a flush
    /// roughly six times cheaper than the other two while doing the same work
    /// in a place nothing was looking. The cost had not gone anywhere, and a
    /// reader comparing the published numbers would have concluded the
    /// opposite of the truth. Found by `M25.5` while reading the very figures
    /// `M25.4` produced.
    pub prepare: Duration,
    /// Wall time inside the call that builds and submits the flush.
    ///
    /// On a handle where the operation completes inline, this **is** the
    /// flush: the device round trip happens inside `SubmitIoRing`.
    pub submit: Duration,
    /// From the submit returning to the harness asking for the completion.
    ///
    /// **Not a cost of the flush.** It is work the program chose to do first
    /// -- here, pushing the next epoch's appends -- and a design that defers
    /// further grows this number while being no slower. It is kept because the
    /// figure this harness used to publish as commit latency was exactly this,
    /// and a number that was once mistaken for another is worth showing beside
    /// the one it was mistaken for.
    pub deferral: Duration,
    /// Wall time actually spent waiting for the flush's completion.
    ///
    /// Zero when the completion was already queued by the time the harness
    /// looked. **That happens for two different reasons and this number cannot
    /// tell them apart**: the operation may have completed inline during the
    /// submit, or it may have pended and then finished during the deferral.
    /// Reading a zero here as evidence of inline completion is the same error
    /// in the opposite direction as the one `M20.6` found, which is why this
    /// is never reported without `deferral` beside it.
    pub blocking: Duration,
}

impl CommitTiming {
    /// What the commit itself cost: preparing for it, submitting it, and
    /// waiting for it.
    ///
    /// Excludes [`deferral`](Self::deferral), which is the program's and not
    /// the commit's. This is the figure `M25.4` asked for, with the
    /// [`prepare`](Self::prepare) term `M25.5` found it was missing.
    pub fn flush(&self) -> Duration {
        self.prepare + self.submit + self.blocking
    }
}

impl Outcome {
    /// Records per second over the whole run.
    pub fn throughput(&self) -> f64 {
        if self.elapsed.is_zero() {
            return 0.0;
        }
        self.records as f64 / self.elapsed.as_secs_f64()
    }

    /// The quantile at `fraction` of whichever part of a commit `part`
    /// selects.
    ///
    /// Nearest-rank rather than interpolated: with tens of samples an
    /// interpolated quantile invents precision the data does not have.
    ///
    /// Taking a projection rather than offering one method per part is what
    /// keeps the four call sites from drifting -- each asks the same question
    /// of a different field instead of each carrying its own copy of the
    /// sort-and-rank.
    pub fn commit_quantile(
        &self,
        part: impl Fn(&CommitTiming) -> Duration,
        fraction: f64,
    ) -> Duration {
        if self.commit_timings.is_empty() {
            return Duration::ZERO;
        }
        let mut sorted: Vec<Duration> = self.commit_timings.iter().map(part).collect();
        sorted.sort_unstable();
        let rank = ((sorted.len() as f64 - 1.0) * fraction).round() as usize;
        sorted[rank.min(sorted.len() - 1)]
    }
}

/// Wait for one lane's outstanding commit and record what it cost (M25.4).
///
/// One function rather than the same four lines at each of the two settle
/// sites -- the loop's, and the drain after it -- because the rule being
/// applied is *where the deferral ends and the blocking begins*, and a rule
/// stated twice is a rule that can be half-corrected. The split has to happen
/// here, at the moment the harness decides to ask, which is precisely what a
/// single `elapsed()` at the end could not distinguish.
///
/// A lane with nothing outstanding is not an error: the first epoch on each
/// lane has no previous commit to settle.
fn settle(
    lane: &mut Lane,
    deferred: &mut Option<(usize, Duration, Duration, Instant)>,
    timings: &mut Vec<CommitTiming>,
) -> io::Result<()> {
    let Some((user_data, prepare, submit, submitted_at)) = deferred.take() else {
        return Ok(());
    };
    let deferral = submitted_at.elapsed();
    let blocked = Instant::now();
    lane.await_flush(user_data)?;
    timings.push(CommitTiming {
        prepare,
        submit,
        deferral,
        blocking: blocked.elapsed(),
    });
    Ok(())
}

/// One ring plus the arena registered on it.
///
/// The arena is a separate registration per ring, which is the doubled cost
/// [`CommitStrategy::AlternatingRings`] is charged for -- and it is doubled
/// permanently, because `IoRing` has no unregister call.
/// A lane's ring.
///
/// Writes carry the slot they read from as the sidecar; flushes are pushed
/// raw and hold nothing. That difference is what tells the two apart in
/// [`Lane::classify`], which is most of what the `in_flight` map this struct
/// used to keep was for.
type LaneRing = IoRing<(), u32>;

/// What a pop on a [`LaneRing`] hands back.
type LaneHeld = Option<(Option<()>, u32)>;

struct Lane {
    ring: LaneRing,
    arena: RegisteredBuffers<NumaBuffer>,
    /// How many writes this lane still owes a completion for. The ring holds
    /// each one's registration lease and releases it at the pop, so this is a
    /// count rather than a table of things the caller must not lose.
    outstanding_writes: usize,
    /// Completions that were not writes, kept by `UserData` for the commit
    /// path to match against.
    flushes: std::collections::HashMap<usize, io::Result<()>>,
}

impl Lane {
    fn new(node: Option<NumaNode>) -> io::Result<Self> {
        let mut ring = LaneRing::with_inventory(64, 128)?;
        let buffers = (0..SLOTS)
            .map(|_| NumaBuffer::new(SLOT_LEN, node))
            .collect::<io::Result<Vec<_>>>()?;
        let mut batch = Batch::new(&mut ring);
        let pending = batch.register_buffers(buffers)?;
        batch.submit_and_wait(1, WAIT_MS)?;
        let (completion, _held) = ring
            .try_pop()?
            .ok_or_else(|| io::Error::other("buffer registration produced no completion"))?;
        let arena = pending
            .claim_if(&completion)
            .map_err(|_| io::Error::other("registration token refused its own completion"))??;
        Ok(Self {
            ring,
            arena,
            outstanding_writes: 0,
            flushes: std::collections::HashMap::new(),
        })
    }

    #[expect(
        dead_code,
        reason = "M14.4 measures ring idle time, which is exactly this count reaching zero"
    )]
    fn outstanding(&self) -> usize {
        self.outstanding_writes
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
            {
                let bytes = self.arena.get_mut(slot)?;
                // The same composition the log's own appender uses, from one
                // definition: encode, then zero the rest of the block. This
                // lane is a second writer over the same format, and it
                // previously carried its own packed layout *and* its own copy
                // of the justification for it.
                record::encode_block(bytes, sequence, epoch, payload)?;
            }
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
            // SAFETY: `file` outlives every operation pushed here -- the
            // caller drains to empty before closing it -- and the ring holds
            // the slot's registration lease until its completion is popped,
            // so the slot cannot be refilled underneath the kernel.
            unsafe {
                batch.write_registered_raw_owned(
                    file,
                    &self.arena,
                    span,
                    slot,
                    first_offset + written,
                    PushOptions::new(),
                    WriteCaching::Cached,
                )
            }?;
            self.outstanding_writes += 1;
            written += record::RECORD_STRIDE as u64;
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
        while let Some((completion, held)) = self.ring.try_pop()? {
            popped += 1;
            self.classify(completion, held)?;
        }
        Ok(popped)
    }

    /// File one popped completion: an append returns its slot, anything else
    /// is a flush the commit path will match against.
    ///
    /// Factored out of [`Lane::drain`] so the bounded waits below can file a
    /// completion they blocked for without a second copy of this logic.
    fn classify(&mut self, completion: Completion, held: LaneHeld) -> io::Result<()> {
        if let Some((_payload, slot)) = held {
            // The pop already released the slot: the ring held the
            // registration lease and dropped it as the entry retired, so the
            // count is back to zero before this line runs. That used to
            // depend on claiming *before* checking the result, because an
            // early return would have dropped the token unclaimed and burnt
            // the slot permanently. There is no ordering left to get wrong.
            self.outstanding_writes -= 1;
            debug_assert!(
                self.arena.outstanding(slot) == Some(0),
                "the pop must release the slot"
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
                    Some((completion, held)) => self.classify(completion, held)?,
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
        while self.outstanding_writes > 0 {
            if self.drain()? == 0 {
                match self.ring.pop_within(remaining(deadline))? {
                    Some((completion, held)) => self.classify(completion, held)?,
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
    node: Option<NumaNode>,
) -> io::Result<Outcome> {
    let mut lanes = Vec::with_capacity(strategy.rings());
    for _ in 0..strategy.rings() {
        lanes.push(Lane::new(node)?);
    }

    let mut offset = 0u64;
    let mut sequence = 0u64;
    let mut records = 0usize;
    let mut commit_timings = Vec::with_capacity(epochs);
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
    let mut deferred: Vec<Option<(usize, Duration, Duration, Instant)>> = vec![None; lanes.len()];

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
        settle(
            &mut lanes[lane_index],
            &mut deferred[lane_index],
            &mut commit_timings,
        )?;

        // Timed from here, not from the submit. What a strategy must do
        // *before* it can push its flush is part of what committing costs it
        // -- and leaving it out is what made `HostSequenced` look six times
        // cheaper than the others in M25.4's first numbers.
        let preparing = Instant::now();
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
        let prepare = preparing.elapsed();
        // The submit itself, which is the part of a commit's cost that exists
        // whether or not the operation pends: on a handle that completes
        // inline, the device round trip happens inside this call.
        let submitting = Instant::now();
        let user_data = lanes[lane_index].commit(file, coverage)?;
        deferred[lane_index] = Some((user_data, prepare, submitting.elapsed(), Instant::now()));
    }

    // Settle the last outstanding commit, or `durable_through` below would be
    // a claim rather than an observation.
    for lane_index in 0..lanes.len() {
        settle(
            &mut lanes[lane_index],
            &mut deferred[lane_index],
            &mut commit_timings,
        )?;
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
        commit_timings.len(),
        epochs,
        "{} observed {} of {epochs} commits",
        strategy.name(),
        commit_timings.len()
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
        commit_timings,
        append_stall,
    })
}

#[cfg(test)]
mod tests;
