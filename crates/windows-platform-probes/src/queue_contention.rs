// Copyright (c) Mike Grier.

//! Does the array queue's tail claim contend at realistic producer counts?
//!
//! **An experiment, not a component.** These probes measure platform behaviour
//! and are not for production use. Do not call them from production code, and
//! do not lift a technique out of here. See this crate's DESIGN-NOTES.md.
//!
//! # The two decisions this exists to force
//!
//! **1. Are the linked and sharded MPSC shapes needed at all?** They are parked
//! in a plan that is not in this repository yet, gated on this measurement
//! rather than on taste. (The named checklist file arrives with the rest of the
//! queue work; naming a path that does not resolve is what this crate's own
//! link rule forbids, and an earlier draft did it here.)
//! If N threads compare-and-swapping one tail does not collapse
//! at the producer counts a real system reaches, the bounded array queue is the
//! only MPSC the queue crate ever needs, and two speculative shapes never get
//! written.
//!
//! **2. Should `slotwise_mpsc` and `reserving_mpsc` merge?** They ship as peers because
//! honouring a reservation costs the producer a read of the consumer's
//! position -- one line every thread touches -- and *how much* that costs was a
//! judgement rather than a measurement. If it is cheap, the two shapes merge and
//! the non-reserving one goes; if it is expensive, the split is vindicated.
//!
//! # Two regimes, because one of them cannot answer the second question
//!
//! Producers are timed twice, and the pair is the point.
//!
//! - **Isolated** -- capacity large enough that nothing is ever refused, and no
//!   consumer running. Nothing else touches the queue, so the curve against N
//!   is the producer side alone, with no consumer traffic in it.
//!
//!   **It is not the compare-and-swap alone, and an earlier draft said it
//!   was.** What is timed is each shape's whole push path: the tail claim, but
//!   also the slot-sequence load, the item write, the publication store, and
//!   the doorbell's fence. `permit_mpsc` takes two shared read-modify-writes
//!   where the others take one. So a difference between shapes here is a
//!   difference in PUSH COST, and attributing it to the claim alone would be
//!   reading more out of the number than is in it. Found by a review.
//!
//! - **Drained** -- a consumer popping continuously while the producers push.
//!   This is the one that can price `reserving_mpsc`, because its producer reads
//!   `head`, and `head` is only expensive to read when a consumer is *writing*
//!   it. Measured in isolation that read hits a clean, shared line and looks
//!   free -- which would be a confident wrong answer.
//!
//! # What is deliberately not claimed
//!
//! The drained regime has a **single** consumer, because that is what MPSC
//! means. At high producer counts it is therefore expected to become
//! consumer-bound, and a throughput plateau there says nothing about the tail
//! claim. The probe reports each run's refusal count -- from the queue's own
//! `Observable` counters -- so a backpressure-bound run is visible as a fact
//! rather than mistaken for contention. Read the isolated regime for the
//! contention question, and the drained one for the cost of `head`.

use std::sync::Arc;
use std::sync::Barrier;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::Instant;

use windows_waitable_queues::{permit_mpsc, reserving_mpsc, slotwise_mpsc};

use windows_waitable_queues::reserving_mpsc::{Balanced, ClaimLayout, Enduring, Perpetual};

/// The 128-bit layout exists only where a 128-bit exchange is native.
///
/// The condition is duplicated in this crate's `Cargo.toml`, which adds the
/// `dwcas` feature under the same `cfg`; see the comment there for why the
/// architectures are named rather than testing `target_has_atomic = "128"`, and
/// why enabling the feature unconditionally breaks the workspace's deliberately
/// supported `i686-pc-windows-msvc` build. Changing one without the other yields
/// either a missing type or an unused feature.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
use windows_waitable_queues::reserving_mpsc::Wide;

#[cfg(test)]
mod tests;

/// How many pushes each producer thread performs in one timed run.
pub const PUSHES_PER_PRODUCER: usize = 50_000;

/// How many times each configuration is repeated; the median is reported.
///
/// Odd, so the median is an observed value rather than an average of two. Five
/// because these probes run on a virtual machine, where a single run can be
/// perturbed by something entirely outside the process.
pub const REPETITIONS: usize = 5;

/// The producer counts measured, in order.
///
/// Fixed rather than derived from the host's processor count, so two runs on
/// different machines produce comparable rows. The host's own count is reported
/// alongside, since the interesting region is around and beyond it.
pub const PRODUCER_COUNTS: &[usize] = &[1, 2, 4, 8, 16, 32];

/// The names a run is filed under.
///
/// **Named once because a lookup by string literal is a rename waiting to
/// fail, and this one already did.** The `mpsc` -> `slotwise_mpsc` rename
/// updated the recording side and not the reporting binary, which went on
/// asking for `"mpsc"`; every lookup returned `None` and two entire columns of
/// the report rendered as `--` without anything erroring. A wrong shape name is
/// not a compile error, so the only defence is that both sides read the same
/// definition.
pub mod shapes {
    /// The bounded-array MPSC.
    pub const SLOTWISE_MPSC: &str = "slotwise_mpsc";
    /// The reservation-based MPSC.
    pub const RESERVING_MPSC: &str = "reserving_mpsc";
    /// The experimental permit-claiming MPSC, measured against
    /// [`RESERVING_MPSC`] because it is a candidate replacement for it.
    pub const PERMIT_MPSC: &str = "permit_mpsc";
    /// The uncontended-atomic floor the queues are measured against.
    pub const BASELINE_FETCH_ADD: &str = "baseline_fetch_add";
    /// `reserving_mpsc` on its default layout: a `u64` split 32 / 32.
    ///
    /// The same configuration as [`RESERVING_MPSC`], run again under its own
    /// name so the layout comparison reads without a reader having to know
    /// which layout the default is.
    pub const CLAIM_NARROW: &str = "reserving(32/32)";
    /// `reserving_mpsc` on `Enduring`: a `u64` split 16 / 48.
    pub const CLAIM_DEEP: &str = "reserving(16/48)";
    /// `reserving_mpsc` on `Perpetual`: a `u64` split 8 / 56.
    pub const CLAIM_PERPETUAL: &str = "reserving(8/56)";
    /// `reserving_mpsc` on `Wide`: a `u128` split 64 / 64.
    pub const CLAIM_WIDE: &str = "reserving(64/64)";
}
/// One configuration's result.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Run {
    /// Which queue shape, or the baseline.
    pub shape: &'static str,
    /// How many producer threads pushed concurrently.
    pub producers: usize,
    /// Median nanoseconds per successful push, across all producers.
    pub nanos_per_push: f64,
    /// Successful pushes per second, summed across producers.
    pub pushes_per_second: f64,
    /// Pushes refused for want of room during the median run.
    ///
    /// Non-zero means the run was at least partly bounded by the consumer
    /// rather than by the claim, which is a fact about the measurement and not
    /// about the queue.
    pub refusals: u64,
}

/// Everything one invocation measured.
#[derive(Debug, Clone)]
pub struct Observation {
    /// Producers timed with no consumer and no possibility of refusal.
    pub isolated: Vec<Run>,
    /// Producers timed against a continuously draining consumer.
    pub drained: Vec<Run>,
    /// Logical processors the host reports.
    pub logical_processors: usize,
}

impl Observation {
    /// Look one run up.
    #[must_use]
    pub fn find(&self, regime: &[Run], shape: &str, producers: usize) -> Option<Run> {
        regime
            .iter()
            .find(|run| run.shape == shape && run.producers == producers)
            .copied()
    }

    /// How far throughput scaled from one producer to `producers`.
    ///
    /// 1.0 means N producers together push no faster than one did, which is
    /// what a badly contended claim looks like. Perfect scaling would be N,
    /// which no shared-tail queue can reach.
    #[must_use]
    pub fn scaling(&self, regime: &[Run], shape: &str, producers: usize) -> Option<f64> {
        let one = self.find(regime, shape, 1)?;
        let many = self.find(regime, shape, producers)?;
        Some(many.pushes_per_second / one.pushes_per_second)
    }
}

/// Time every configuration.
#[must_use]
pub fn measure() -> Observation {
    let mut isolated = Vec::new();
    let mut drained = Vec::new();

    for &producers in PRODUCER_COUNTS {
        isolated.push(median_run(shapes::BASELINE_FETCH_ADD, producers, |count| {
            time_contended_atomic(count)
        }));
        isolated.push(median_run(shapes::SLOTWISE_MPSC, producers, |count| {
            time_isolated_mpsc(count)
        }));
        isolated.push(median_run(shapes::RESERVING_MPSC, producers, |count| {
            time_isolated_reserving(count)
        }));
        isolated.push(median_run(shapes::PERMIT_MPSC, producers, |count| {
            time_isolated_permit(count)
        }));

        drained.push(median_run(shapes::SLOTWISE_MPSC, producers, |count| {
            time_drained_mpsc(count)
        }));
        drained.push(median_run(shapes::RESERVING_MPSC, producers, |count| {
            time_drained_reserving(count)
        }));
        drained.push(median_run(shapes::PERMIT_MPSC, producers, |count| {
            time_drained_permit(count)
        }));

        isolated.push(median_run(shapes::CLAIM_NARROW, producers, |count| {
            time_isolated_layout::<Balanced>(count)
        }));
        isolated.push(median_run(shapes::CLAIM_DEEP, producers, |count| {
            time_isolated_layout::<Enduring>(count)
        }));
        isolated.push(median_run(shapes::CLAIM_PERPETUAL, producers, |count| {
            time_isolated_layout::<Perpetual>(count)
        }));
        // Gated on the architectures where a 128-bit exchange is native; see the
        // `Wide` import above. `#[cfg]` governs only the statement that follows
        // it, so each of the two pushes carries its own.
        #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
        isolated.push(median_run(shapes::CLAIM_WIDE, producers, |count| {
            time_isolated_layout::<Wide>(count)
        }));

        drained.push(median_run(shapes::CLAIM_NARROW, producers, |count| {
            time_drained_layout::<Balanced>(count)
        }));
        drained.push(median_run(shapes::CLAIM_DEEP, producers, |count| {
            time_drained_layout::<Enduring>(count)
        }));
        drained.push(median_run(shapes::CLAIM_PERPETUAL, producers, |count| {
            time_drained_layout::<Perpetual>(count)
        }));
        #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
        drained.push(median_run(shapes::CLAIM_WIDE, producers, |count| {
            time_drained_layout::<Wide>(count)
        }));
    }

    Observation {
        isolated,
        drained,
        logical_processors: thread::available_parallelism().map_or(0, std::num::NonZeroUsize::get),
    }
}

/// Raw result of one timed repetition: elapsed nanoseconds and refusals.
type Repetition = (f64, u64);

/// Run one configuration [`REPETITIONS`] times and keep the median.
///
/// The median rather than the mean, because on a virtual machine the failure
/// mode is one run being hugely slower rather than a spread around a centre,
/// and a mean would carry that outlier into the reported number.
fn median_run(
    shape: &'static str,
    producers: usize,
    mut timer: impl FnMut(usize) -> Repetition,
) -> Run {
    // One untimed pass first. Be exact about what this does and does not warm:
    // every call to `timer` builds and drops its OWN queue, so this does not
    // pre-touch the allocation any timed repetition will use. What it does warm
    // is the process -- the allocator's size class, the OS page cache, the
    // instruction cache, and the branch predictors -- which is why the first
    // timed repetition is no longer an outlier. An earlier comment here claimed
    // it faulted in "the" allocation, which is not true of an allocation made
    // fresh each pass. Found by a review.
    let _ = timer(producers);

    let mut results: Vec<Repetition> = (0..REPETITIONS).map(|_| timer(producers)).collect();
    results.sort_by(|left, right| left.0.total_cmp(&right.0));
    let (elapsed_nanos, refusals) = results[REPETITIONS / 2];

    let pushes = (producers * PUSHES_PER_PRODUCER) as f64;
    Run {
        shape,
        producers,
        nanos_per_push: elapsed_nanos / pushes,
        pushes_per_second: pushes / (elapsed_nanos / 1e9),
        refusals,
    }
}

/// The floor: N threads incrementing one shared counter.
///
/// Not a queue, and not trying to be. It is the cheapest possible operation on
/// a contended line, so it says how much of a queue's scaling curve is the
/// queue and how much is simply what this processor does when N cores fight
/// over one cache line.
fn time_contended_atomic(producers: usize) -> Repetition {
    let counter = Arc::new(AtomicU64::new(0));
    // One party per worker plus this thread. Every worker is created, then waits
    // here; the clock starts as the barrier releases, so neither thread creation
    // nor a solo head start by an early worker is inside the measurement. See
    // `start_barrier`'s note for why that matters at these producer counts.
    let gate = Arc::new(Barrier::new(producers + 1));
    let spans = thread::scope(|scope| {
        let mut workers = Vec::with_capacity(producers);
        for _ in 0..producers {
            let counter = Arc::clone(&counter);
            let gate = Arc::clone(&gate);
            workers.push(scope.spawn(move || {
                gate.wait();
                let began = Instant::now();
                for _ in 0..PUSHES_PER_PRODUCER {
                    counter.fetch_add(1, Ordering::Relaxed);
                }
                (began, Instant::now())
            }));
        }
        gate.wait();
        workers
            .into_iter()
            .map(|worker| worker.join().expect("a producer must not panic"))
            .collect::<Vec<_>>()
    });
    (measured_span(&spans), 0)
}

/// Capacity big enough that a whole run fits, so nothing is ever refused.
fn capacity_for(producers: usize) -> usize {
    (producers * PUSHES_PER_PRODUCER).next_power_of_two()
}

/// A gate holding every participant until all of them exist.
///
/// **Without this the row labelled N producers need not have measured N of
/// them.** Spawning is not instant, and each worker used to start pushing the
/// moment it was created, so at 50,000 pushes an early producer could complete
/// a long uncontended prefix -- or finish entirely -- before the last thread was
/// spawned. The reported interval also began before any worker existed, folding
/// thread-creation cost into a per-push number. The curve against N is the whole
/// output of this probe, and both effects bend it downward exactly where it is
/// steepest.
///
/// The count includes this thread, so no worker can start before the last one
/// exists. It does NOT start the clock -- see [`measured_span`] for why that is
/// a separate job.
fn start_barrier(participants: usize) -> Arc<Barrier> {
    Arc::new(Barrier::new(participants + 1))
}

/// The wall-clock window the producers were actually inside: from the first to
/// begin to the last to finish.
///
/// **Each worker times itself, because this thread cannot time them.** The
/// obvious arrangement -- release the barrier, call `Instant::now()` here, and
/// read `elapsed()` after the scope ends -- is wrong at both ends, and a review
/// caught it:
///
/// - `Barrier::wait` releases every party together, and this thread is just
///   another party. A worker can return from `wait` and run an arbitrary prefix
///   of its pushes before this thread is scheduled again to read the clock, so
///   the start could land after work had already happened. That understates the
///   interval, which OVERSTATES throughput.
/// - `thread::scope` joins every worker before it returns, so an `elapsed()`
///   read after it includes thread exit and join. That overstates the interval,
///   which understates throughput.
///
/// Neither error is bounded by anything this probe controls, and both bite
/// hardest on the fast low-producer rows where a run is only hundreds of
/// microseconds. Taking the earliest start and the latest finish measures the
/// span the producers were contending over and nothing else.
fn measured_span(spans: &[(Instant, Instant)]) -> f64 {
    let began = spans
        .iter()
        .map(|(began, _)| *began)
        .min()
        .expect("a run has at least one producer");
    let ended = spans
        .iter()
        .map(|(_, ended)| *ended)
        .max()
        .expect("a run has at least one producer");

    ended.duration_since(began).as_nanos() as f64
}

fn time_isolated_mpsc(producers: usize) -> Repetition {
    let (tx, rx) =
        slotwise_mpsc::bounded::<u64>(capacity_for(producers)).expect("a valid capacity");
    let gate = start_barrier(producers);
    let spans = thread::scope(|scope| {
        let mut workers = Vec::with_capacity(producers);
        for producer in 0..producers {
            let tx = tx.clone();
            let gate = Arc::clone(&gate);
            workers.push(scope.spawn(move || {
                gate.wait();
                let began = Instant::now();
                for index in 0..PUSHES_PER_PRODUCER {
                    tx.push((producer * PUSHES_PER_PRODUCER + index) as u64)
                        .expect("the run fits in the capacity");
                }
                (began, Instant::now())
            }));
        }
        gate.wait();
        workers
            .into_iter()
            .map(|worker| worker.join().expect("a producer must not panic"))
            .collect::<Vec<_>>()
    });
    let elapsed = measured_span(&spans);
    let refusals = tx.refused();
    // Drain before dropping: teardown would otherwise walk every slot, and that
    // is not part of what is being timed.
    while rx.pop().is_ok() {}
    (elapsed, refusals)
}

fn time_isolated_reserving(producers: usize) -> Repetition {
    let (tx, rx) =
        reserving_mpsc::bounded::<u64>(capacity_for(producers)).expect("a valid capacity");
    let gate = start_barrier(producers);
    let spans = thread::scope(|scope| {
        let mut workers = Vec::with_capacity(producers);
        for producer in 0..producers {
            let tx = tx.clone();
            let gate = Arc::clone(&gate);
            workers.push(scope.spawn(move || {
                gate.wait();
                let began = Instant::now();
                for index in 0..PUSHES_PER_PRODUCER {
                    tx.push((producer * PUSHES_PER_PRODUCER + index) as u64)
                        .expect("the run fits in the capacity");
                }
                (began, Instant::now())
            }));
        }
        gate.wait();
        workers
            .into_iter()
            .map(|worker| worker.join().expect("a producer must not panic"))
            .collect::<Vec<_>>()
    });
    let elapsed = measured_span(&spans);
    let refusals = tx.refused();
    while rx.pop().is_ok() {}
    (elapsed, refusals)
}

/// The experimental permit claim, in the regime that isolates the claim itself.
///
/// A line-for-line twin of [`time_isolated_reserving`] with one shape
/// substituted. Deliberately not factored into a generic over the two, which
/// would need a trait both implement and would put a dynamic or monomorphised
/// indirection inside the timed region -- in a measurement whose whole output is
/// a difference of a few nanoseconds per push.
fn time_isolated_permit(producers: usize) -> Repetition {
    let (tx, rx) = permit_mpsc::bounded::<u64>(capacity_for(producers)).expect("a valid capacity");
    let gate = start_barrier(producers);
    let spans = thread::scope(|scope| {
        let mut workers = Vec::with_capacity(producers);
        for producer in 0..producers {
            let tx = tx.clone();
            let gate = Arc::clone(&gate);
            workers.push(scope.spawn(move || {
                gate.wait();
                let began = Instant::now();
                for index in 0..PUSHES_PER_PRODUCER {
                    tx.push((producer * PUSHES_PER_PRODUCER + index) as u64)
                        .expect("the run fits in the capacity");
                }
                (began, Instant::now())
            }));
        }
        gate.wait();
        workers
            .into_iter()
            .map(|worker| worker.join().expect("a producer must not panic"))
            .collect::<Vec<_>>()
    });
    let elapsed = measured_span(&spans);
    let refusals = tx.refused();
    while rx.pop().is_ok() {}
    (elapsed, refusals)
}

/// A capacity a real system would choose, so the drained regime exercises
/// backpressure the way a real one would.
const DRAINED_CAPACITY: usize = 1024;

fn time_drained_mpsc(producers: usize) -> Repetition {
    let (tx, rx) = slotwise_mpsc::bounded::<u64>(DRAINED_CAPACITY).expect("a valid capacity");
    let done = Arc::new(AtomicBool::new(false));
    let consumer_done = Arc::clone(&done);
    // The consumer is a barrier participant, not merely spawned: spawning is not
    // readiness, and a consumer still in thread start-up while producers push
    // turns the opening of the run into an undrained regime.
    //
    // Be precise about what this buys, because it is less than it looks. The
    // barrier guarantees the consumer has ARRIVED -- it exists, is scheduled, and
    // is past start-up -- not that it reaches its first `pop` before a producer
    // reaches its first `push`. A release wakes every party at once, so a short
    // undrained window remains. It is bounded by a scheduling quantum rather than
    // by thread creation, which is the improvement; it is not zero. Closing it
    // needs a readiness flag the producers spin on, which would change the
    // measurement and so obsolete every figure already published against it --
    // queued as M4.3 rather than taken mid-branch.
    let gate = start_barrier(producers + 1);
    let consumer_gate = Arc::clone(&gate);

    let consumer = thread::spawn(move || {
        consumer_gate.wait();
        // Spin rather than park: the doorbell's cost is `doorbell_cost`'s
        // question, and parking here would measure that instead of the claim.
        while !consumer_done.load(Ordering::Relaxed) {
            while rx.pop().is_ok() {}
            std::hint::spin_loop();
        }
        while rx.pop().is_ok() {}
        rx.refused()
    });

    let spans = thread::scope(|scope| {
        let mut workers = Vec::with_capacity(producers);
        for producer in 0..producers {
            let tx = tx.clone();
            let gate = Arc::clone(&gate);
            workers.push(scope.spawn(move || {
                gate.wait();
                let began = Instant::now();
                for index in 0..PUSHES_PER_PRODUCER {
                    let mut item = (producer * PUSHES_PER_PRODUCER + index) as u64;
                    // Retry a FULL queue, which is what a real producer does;
                    // the refusal count is what makes that visible. Anything
                    // else is not retryable -- a disconnected queue never
                    // drains -- and retrying it is an infinite spin that
                    // presents as a hung probe rather than as the consumer
                    // failure it actually is. The queue crate says so itself:
                    // "retrying the first is sensible and retrying the second
                    // is a spin".
                    while let Err(error) = tx.push(item) {
                        assert!(
                            error.is_retryable(),
                            "the consumer is gone, so this push can never \
                             succeed: {error}"
                        );
                        item = error.into_inner();
                        std::hint::spin_loop();
                    }
                }
                (began, Instant::now())
            }));
        }
        gate.wait();
        workers
            .into_iter()
            .map(|worker| worker.join().expect("a producer must not panic"))
            .collect::<Vec<_>>()
    });
    let elapsed = measured_span(&spans);

    done.store(true, Ordering::Relaxed);
    drop(tx);
    let refusals = consumer.join().expect("the consumer must not panic");
    (elapsed, refusals)
}

fn time_drained_reserving(producers: usize) -> Repetition {
    // **Defaults on both sides, and that is a correction.** This row previously
    // enabled high-water tracking here and nowhere else, to "also price the
    // switch M31.4 made opt-in". But the number it feeds is presented as the
    // cost of *reservation*, and tracking adds an unrelated operation to this
    // shape's push path alone -- a load of the consumer's position, which is
    // exactly the shared line the other shape's push is built to avoid
    // touching. The ratio therefore measured reservation plus a handicap, with
    // no way for a reader to separate them.
    //
    // Nothing consumes the high-water figure here either, so the tracking was
    // paying a cost to produce a number nobody read. Pricing that switch is a
    // worthwhile measurement and needs its own row, with both shapes tracking,
    // rather than being folded into this comparison.
    let (tx, rx) = reserving_mpsc::bounded::<u64>(DRAINED_CAPACITY).expect("a valid capacity");
    let done = Arc::new(AtomicBool::new(false));
    let consumer_done = Arc::clone(&done);
    // The consumer joins the gate here for the reason it does in the slotwise
    // twin: a run whose opening is undrained is not the regime being measured.
    let gate = start_barrier(producers + 1);
    let consumer_gate = Arc::clone(&gate);

    let consumer = thread::spawn(move || {
        consumer_gate.wait();
        while !consumer_done.load(Ordering::Relaxed) {
            while rx.pop().is_ok() {}
            std::hint::spin_loop();
        }
        while rx.pop().is_ok() {}
        rx.refused()
    });

    let spans = thread::scope(|scope| {
        let mut workers = Vec::with_capacity(producers);
        for producer in 0..producers {
            let tx = tx.clone();
            let gate = Arc::clone(&gate);
            workers.push(scope.spawn(move || {
                gate.wait();
                let began = Instant::now();
                for index in 0..PUSHES_PER_PRODUCER {
                    let mut item = (producer * PUSHES_PER_PRODUCER + index) as u64;
                    while let Err(error) = tx.push(item) {
                        // Only a FULL queue is retryable; see the note on the
                        // first of these loops.
                        assert!(
                            error.is_retryable(),
                            "the consumer is gone, so this push can never \
                             succeed: {error}"
                        );
                        item = error.into_inner();
                        std::hint::spin_loop();
                    }
                }
                (began, Instant::now())
            }));
        }
        gate.wait();
        workers
            .into_iter()
            .map(|worker| worker.join().expect("a producer must not panic"))
            .collect::<Vec<_>>()
    });
    let elapsed = measured_span(&spans);

    done.store(true, Ordering::Relaxed);
    drop(tx);
    let refusals = consumer.join().expect("the consumer must not panic");
    (elapsed, refusals)
}

/// The experimental permit claim, against a continuously draining consumer.
///
/// The regime that can price the claim honestly, for the same reason the
/// reserving twin needs it: the shared line a producer touches is only
/// expensive when a consumer is writing it. Measured in isolation, an
/// uncontended line looks free -- which would be a confident wrong answer, and
/// this shape has more riding on that answer than the others, because it trades
/// `reserving_mpsc`'s *load* of the consumer's position for a read-modify-write
/// on a count the consumer also writes.
fn time_drained_permit(producers: usize) -> Repetition {
    let (tx, rx) = permit_mpsc::bounded::<u64>(DRAINED_CAPACITY).expect("a valid capacity");
    let done = Arc::new(AtomicBool::new(false));
    let consumer_done = Arc::clone(&done);
    let gate = start_barrier(producers + 1);
    let consumer_gate = Arc::clone(&gate);

    let consumer = thread::spawn(move || {
        consumer_gate.wait();
        while !consumer_done.load(Ordering::Relaxed) {
            while rx.pop().is_ok() {}
            std::hint::spin_loop();
        }
        while rx.pop().is_ok() {}
        rx.refused()
    });

    let spans = thread::scope(|scope| {
        let mut workers = Vec::with_capacity(producers);
        for producer in 0..producers {
            let tx = tx.clone();
            let gate = Arc::clone(&gate);
            workers.push(scope.spawn(move || {
                gate.wait();
                let began = Instant::now();
                for index in 0..PUSHES_PER_PRODUCER {
                    let mut item = (producer * PUSHES_PER_PRODUCER + index) as u64;
                    while let Err(error) = tx.push(item) {
                        // Only a FULL queue is retryable; see the note on the
                        // first of these loops.
                        assert!(
                            error.is_retryable(),
                            "the consumer is gone, so this push can never \
                             succeed: {error}"
                        );
                        item = error.into_inner();
                        std::hint::spin_loop();
                    }
                }
                (began, Instant::now())
            }));
        }
        gate.wait();
        workers
            .into_iter()
            .map(|worker| worker.join().expect("a producer must not panic"))
            .collect::<Vec<_>>()
    });
    let elapsed = measured_span(&spans);

    done.store(true, Ordering::Relaxed);
    drop(tx);
    let refusals = consumer.join().expect("the consumer must not panic");
    (elapsed, refusals)
}

/// One claim-word layout, in the regime that isolates the claim.
///
/// **Generic over the layout, where [`time_isolated_permit`] is deliberately
/// duplicated, and the difference is the point.** That twin compares two
/// *different types*, which a generic could only unify behind a trait, putting
/// an indirection that might not inline identically inside the timed region.
/// These are the *same type* at different layout parameters, so this
/// monomorphises to exactly the code a hand-written copy would produce -- there
/// is nothing left to dispatch.
///
/// Measures `reserving_mpsc` itself rather than a stand-in. An earlier form of
/// this probe carried its own duplicated implementation of the claim protocol,
/// built so the layouts could be compared before the shipping crate had them;
/// it drifted from the original twice while doing so. The shipping type takes
/// the layout as a parameter now, so the duplicate is gone.
fn time_isolated_layout<L: ClaimLayout>(producers: usize) -> Repetition {
    let (tx, rx) =
        reserving_mpsc::bounded_as::<u64, L>(capacity_for(producers)).expect("a valid capacity");
    let gate = start_barrier(producers);
    let spans = thread::scope(|scope| {
        let mut workers = Vec::with_capacity(producers);
        for producer in 0..producers {
            let tx = tx.clone();
            let gate = Arc::clone(&gate);
            workers.push(scope.spawn(move || {
                gate.wait();
                let began = Instant::now();
                for index in 0..PUSHES_PER_PRODUCER {
                    tx.push((producer * PUSHES_PER_PRODUCER + index) as u64)
                        .expect("the run fits in the capacity");
                }
                (began, Instant::now())
            }));
        }
        gate.wait();
        workers
            .into_iter()
            .map(|worker| worker.join().expect("a producer must not panic"))
            .collect::<Vec<_>>()
    });
    let elapsed = measured_span(&spans);
    let refusals = tx.refused();
    while rx.pop().is_ok() {}
    (elapsed, refusals)
}

/// One claim-word layout, against a continuously draining consumer.
///
/// Generic for [`time_isolated_layout`]'s reason.
fn time_drained_layout<L: ClaimLayout + 'static>(producers: usize) -> Repetition {
    let (tx, rx) =
        reserving_mpsc::bounded_as::<u64, L>(DRAINED_CAPACITY).expect("a valid capacity");
    let done = Arc::new(AtomicBool::new(false));
    let consumer_done = Arc::clone(&done);
    // The consumer joins the gate for the reason its twins do: a run whose
    // opening is undrained is not the regime being measured.
    let gate = start_barrier(producers + 1);
    let consumer_gate = Arc::clone(&gate);

    let consumer = thread::spawn(move || {
        consumer_gate.wait();
        while !consumer_done.load(Ordering::Relaxed) {
            while rx.pop().is_ok() {}
            std::hint::spin_loop();
        }
        while rx.pop().is_ok() {}
        rx.refused()
    });

    let spans = thread::scope(|scope| {
        let mut workers = Vec::with_capacity(producers);
        for producer in 0..producers {
            let tx = tx.clone();
            let gate = Arc::clone(&gate);
            workers.push(scope.spawn(move || {
                gate.wait();
                let began = Instant::now();
                for index in 0..PUSHES_PER_PRODUCER {
                    let mut item = (producer * PUSHES_PER_PRODUCER + index) as u64;
                    while let Err(error) = tx.push(item) {
                        // Only a FULL queue is retryable; see the note on the
                        // first of these loops.
                        assert!(
                            error.is_retryable(),
                            "the consumer is gone, so this push can never \
                             succeed: {error}"
                        );
                        item = error.into_inner();
                        std::hint::spin_loop();
                    }
                }
                (began, Instant::now())
            }));
        }
        gate.wait();
        workers
            .into_iter()
            .map(|worker| worker.join().expect("a producer must not panic"))
            .collect::<Vec<_>>()
    });
    let elapsed = measured_span(&spans);
    done.store(true, Ordering::Relaxed);
    let refusals = consumer.join().expect("the consumer must not panic");
    (elapsed, refusals)
}
