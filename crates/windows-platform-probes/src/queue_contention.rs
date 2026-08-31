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
//! in `CHECKLIST-io-domains.md` as `M-inf.1`, gated on this measurement rather
//! than on taste. If N threads compare-and-swapping one tail does not collapse
//! at the producer counts a real system reaches, the bounded array queue is the
//! only MPSC the queue crate ever needs, and two speculative shapes never get
//! written.
//!
//! **2. Should `mpsc` and `reserving_mpsc` merge?** They ship as peers because
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
//!   consumer running. This is the *cleanest* measurement of tail-claim
//!   contention: nothing else touches the queue, so whatever curve appears
//!   against N is the compare-and-swap and nothing else.
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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::Instant;

use windows_waitable_queues::{Options, mpsc, reserving_mpsc};

/// How many pushes each producer thread performs in one timed run.
const PUSHES_PER_PRODUCER: usize = 50_000;

/// How many times each configuration is repeated; the median is reported.
///
/// Odd, so the median is an observed value rather than an average of two. Five
/// because these probes run on a virtual machine, where a single run can be
/// perturbed by something entirely outside the process.
const REPETITIONS: usize = 5;

/// The producer counts measured, in order.
///
/// Fixed rather than derived from the host's processor count, so two runs on
/// different machines produce comparable rows. The host's own count is reported
/// alongside, since the interesting region is around and beyond it.
pub const PRODUCER_COUNTS: &[usize] = &[1, 2, 4, 8, 16, 32];

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
        isolated.push(median_run("baseline_fetch_add", producers, |count| {
            time_contended_atomic(count)
        }));
        isolated.push(median_run("mpsc", producers, |count| {
            time_isolated_mpsc(count)
        }));
        isolated.push(median_run("reserving_mpsc", producers, |count| {
            time_isolated_reserving(count)
        }));

        drained.push(median_run("mpsc", producers, |count| {
            time_drained_mpsc(count)
        }));
        drained.push(median_run("reserving_mpsc", producers, |count| {
            time_drained_reserving(count)
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
    // One untimed pass first: the first touch of a fresh allocation faults
    // pages in, and that cost belongs to the allocator rather than the queue.
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
    let started = Instant::now();
    thread::scope(|scope| {
        for _ in 0..producers {
            let counter = Arc::clone(&counter);
            scope.spawn(move || {
                for _ in 0..PUSHES_PER_PRODUCER {
                    counter.fetch_add(1, Ordering::Relaxed);
                }
            });
        }
    });
    (started.elapsed().as_nanos() as f64, 0)
}

/// Capacity big enough that a whole run fits, so nothing is ever refused.
fn capacity_for(producers: usize) -> usize {
    (producers * PUSHES_PER_PRODUCER).next_power_of_two()
}

fn time_isolated_mpsc(producers: usize) -> Repetition {
    let (tx, rx) = mpsc::bounded::<u64>(capacity_for(producers)).expect("a valid capacity");
    let started = Instant::now();
    thread::scope(|scope| {
        for producer in 0..producers {
            let tx = tx.clone();
            scope.spawn(move || {
                for index in 0..PUSHES_PER_PRODUCER {
                    tx.push((producer * PUSHES_PER_PRODUCER + index) as u64)
                        .expect("the run fits in the capacity");
                }
            });
        }
    });
    let elapsed = started.elapsed().as_nanos() as f64;
    let refusals = tx.refused();
    // Drain before dropping: teardown would otherwise walk every slot, and that
    // is not part of what is being timed.
    while rx.pop().is_some() {}
    (elapsed, refusals)
}

fn time_isolated_reserving(producers: usize) -> Repetition {
    let (tx, rx) =
        reserving_mpsc::bounded::<u64>(capacity_for(producers)).expect("a valid capacity");
    let started = Instant::now();
    thread::scope(|scope| {
        for producer in 0..producers {
            let tx = tx.clone();
            scope.spawn(move || {
                for index in 0..PUSHES_PER_PRODUCER {
                    tx.push((producer * PUSHES_PER_PRODUCER + index) as u64)
                        .expect("the run fits in the capacity");
                }
            });
        }
    });
    let elapsed = started.elapsed().as_nanos() as f64;
    let refusals = tx.refused();
    while rx.pop().is_some() {}
    (elapsed, refusals)
}

/// A capacity a real system would choose, so the drained regime exercises
/// backpressure the way a real one would.
const DRAINED_CAPACITY: usize = 1024;

fn time_drained_mpsc(producers: usize) -> Repetition {
    let (tx, rx) = mpsc::bounded::<u64>(DRAINED_CAPACITY).expect("a valid capacity");
    let done = Arc::new(AtomicBool::new(false));
    let consumer_done = Arc::clone(&done);

    let consumer = thread::spawn(move || {
        // Spin rather than park: the doorbell's cost is `doorbell_cost`'s
        // question, and parking here would measure that instead of the claim.
        while !consumer_done.load(Ordering::Relaxed) {
            while rx.pop().is_some() {}
            std::hint::spin_loop();
        }
        while rx.pop().is_some() {}
        rx.refused()
    });

    let started = Instant::now();
    thread::scope(|scope| {
        for producer in 0..producers {
            let tx = tx.clone();
            scope.spawn(move || {
                for index in 0..PUSHES_PER_PRODUCER {
                    let mut item = (producer * PUSHES_PER_PRODUCER + index) as u64;
                    // Retry on a full queue, which is what a real producer
                    // does. The refusal count is what makes that visible.
                    while let Err(error) = tx.push(item) {
                        item = error.into_inner();
                        std::hint::spin_loop();
                    }
                }
            });
        }
    });
    let elapsed = started.elapsed().as_nanos() as f64;

    done.store(true, Ordering::Relaxed);
    drop(tx);
    let refusals = consumer.join().expect("the consumer must not panic");
    (elapsed, refusals)
}

fn time_drained_reserving(producers: usize) -> Repetition {
    let (tx, rx) = reserving_mpsc::bounded_with::<u64>(
        DRAINED_CAPACITY,
        // Tracking on, so this row also prices the switch M31.4 made opt-in.
        Options::new().tracking_high_water(),
    )
    .expect("a valid capacity");
    let done = Arc::new(AtomicBool::new(false));
    let consumer_done = Arc::clone(&done);

    let consumer = thread::spawn(move || {
        while !consumer_done.load(Ordering::Relaxed) {
            while rx.pop().is_some() {}
            std::hint::spin_loop();
        }
        while rx.pop().is_some() {}
        rx.refused()
    });

    let started = Instant::now();
    thread::scope(|scope| {
        for producer in 0..producers {
            let tx = tx.clone();
            scope.spawn(move || {
                for index in 0..PUSHES_PER_PRODUCER {
                    let mut item = (producer * PUSHES_PER_PRODUCER + index) as u64;
                    while let Err(error) = tx.push(item) {
                        item = error.into_inner();
                        std::hint::spin_loop();
                    }
                }
            });
        }
    });
    let elapsed = started.elapsed().as_nanos() as f64;

    done.store(true, Ordering::Relaxed);
    drop(tx);
    let refusals = consumer.join().expect("the consumer must not panic");
    (elapsed, refusals)
}
