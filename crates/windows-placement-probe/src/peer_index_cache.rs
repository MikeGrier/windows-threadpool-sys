// Copyright (c) Mike Grier.

//! What does caching the peer's index buy an SPSC ring?
//!
//! **An experiment, not a component.** These probes measure platform behaviour
//! and are not for production use. Do not call them from production code, and
//! do not lift a technique out of here. See this crate's DESIGN-NOTES.md.
//!
//! # The technique under test
//!
//! In the ring this workspace ships, each side reads the *other's* index on
//! every single operation: the producer acquire-loads `head` to find room, the
//! consumer acquire-loads `tail` to find work. Both of those lines are written
//! by the opposite core, so every operation is a guaranteed cross-core
//! coherence miss and the line ping-pongs at interconnect speed.
//!
//! **Peer-index caching** removes almost all of them. Each side keeps a plain,
//! non-atomic copy of the peer's index and consults the shared line *only* when
//! its cached copy says the queue looks full (producer) or empty (consumer).
//! In the common case -- neither full nor empty -- there is no shared read at
//! all, so one acquire load is amortised across a whole batch. It is the
//! central trick in Erik Rigtorp's "Optimizing a ring buffer for throughput",
//! and the same idea appears in Boost's `spsc_queue` and DPDK's rings.
//!
//! It is safe because both indices are **monotonic**. A stale cached `head`
//! under-reports free space and a stale cached `tail` under-reports available
//! items, so the error is always conservative: a spurious "full" or "empty",
//! never a wrong write or a double read.
//!
//! # And a control, because the alternative memory deserves testing
//!
//! A second reading of the same technique is that the extra load is *only*
//! there to warm the cache line -- that its value cannot be used, and the
//! authoritative load still has to happen. That is a different mechanism with a
//! different ceiling, so it is measured rather than argued about:
//! [`Strategy::Warmed`] issues a discarded relaxed load of the peer index and
//! then does exactly the work the baseline does.
//!
//! # Why this measures a model rather than the shipping queue
//!
//! This crate's rule is that a probe measures the real API, because a stand-in
//! only measures itself. That rule cannot apply here: the variants do not exist
//! in the shipping crate, and the whole question is whether one of them should.
//!
//! So the model is kept structurally identical to `spsc` -- same split indices,
//! same padding, same orderings, same release/acquire pairing -- and the
//! **baseline strategy is calibrated against the real queue** in the same run.
//! If the model's baseline does not reproduce the shipping queue's number, the
//! model is wrong and the variant comparison means nothing. That calibration
//! row is printed first for exactly that reason.

use std::cell::UnsafeCell;
use std::hint::black_box;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Instant;

use core::ptr;

use windows_sys::Win32::System::SystemInformation::GROUP_AFFINITY;
use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadGroupAffinity};
use windows_waitable_queues::spsc;

/// Items handed across the ring in one timed run.
pub const ITEMS: usize = 2_000_000;

/// Ring capacity, in items. Deep enough that a consumer keeping up leaves the
/// producer's cached index valid for long stretches, which is the regime the
/// technique is for.
pub const CAPACITY: usize = 1024;

/// Repetitions per configuration; the median is reported.
const REPETITIONS: usize = 5;

/// Which peer-index strategy a run uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    /// Read the peer's index on every operation. What `spsc` does today.
    Baseline,
    /// Keep a local copy and consult the shared line only when it says the
    /// queue looks full or empty.
    Cached,
    /// Issue a discarded load of the peer's index, then do exactly what the
    /// baseline does. Tests whether the benefit is cache warming rather than
    /// the avoided load.
    Warmed,
}

impl Strategy {
    fn label(self) -> &'static str {
        match self {
            Self::Baseline => "model: baseline",
            Self::Cached => "model: cached index",
            Self::Warmed => "model: warming load",
        }
    }
}

/// One configuration's result.
#[derive(Debug, Clone, Copy)]
pub struct Run {
    /// What was measured.
    pub label: &'static str,
    /// Median nanoseconds per item handed across the ring.
    pub nanos_per_item: f64,
    /// Items per second.
    pub items_per_second: f64,
    /// How many times the consumer actually read the shared `tail`.
    ///
    /// **This is the number that says whether the technique was even
    /// exercised.** Peer-index caching only avoids a shared read when the
    /// cached copy says there is work; a consumer that finds the ring empty
    /// every time refreshes every time, and caching can do nothing for it. A
    /// count near [`ITEMS`] means the ring never had a backlog to batch over,
    /// and any comparison drawn from that run is a comparison of branches
    /// rather than of cache traffic.
    pub consumer_refreshes: u64,
    /// How many times the producer actually read the shared `head`.
    pub producer_refreshes: u64,
}

/// Everything one invocation measured.
#[derive(Debug, Clone)]
pub struct Observation {
    /// The shipping `spsc`, so the model can be checked against it.
    pub calibration: Run,
    /// The model under each strategy.
    pub strategies: Vec<Run>,
}

impl Observation {
    /// Look one strategy's run up.
    #[must_use]
    pub fn get(&self, strategy: Strategy) -> Option<Run> {
        self.strategies
            .iter()
            .find(|run| run.label == strategy.label())
            .copied()
    }
}

/// Time the shipping queue and every model strategy.
#[must_use]
pub fn measure() -> Observation {
    Observation {
        calibration: median("shipping spsc", time_real_spsc),
        strategies: [Strategy::Baseline, Strategy::Cached, Strategy::Warmed]
            .into_iter()
            .map(|strategy| median(strategy.label(), || time_model(strategy)))
            .collect(),
    }
}

/// One timed pass, with the shared-read counts that pass performed.
#[derive(Debug, Clone, Copy)]
pub struct Sample {
    /// Wall-clock nanoseconds for the whole pass.
    pub nanos: f64,
    /// How many times the consumer read the producer's shared position.
    pub consumer_refreshes: u64,
    /// How many times the producer read the consumer's shared position.
    pub producer_refreshes: u64,
}

fn median(label: &'static str, mut timer: impl FnMut() -> Sample) -> Run {
    // One untimed pass: first touch of a fresh allocation faults pages in, and
    // that belongs to the allocator rather than to the ring.
    let _ = timer();

    let mut samples: Vec<Sample> = (0..REPETITIONS).map(|_| timer()).collect();
    samples.sort_by(|left, right| left.nanos.total_cmp(&right.nanos));
    let sample = samples[REPETITIONS / 2];

    Run {
        label,
        nanos_per_item: sample.nanos / ITEMS as f64,
        items_per_second: ITEMS as f64 / (sample.nanos / 1e9),
        consumer_refreshes: sample.consumer_refreshes,
        producer_refreshes: sample.producer_refreshes,
    }
}

/// The shipping queue, driven the same way the model is.
fn time_real_spsc() -> Sample {
    let (tx, rx) = spsc::bounded::<u64>(CAPACITY).expect("a valid capacity");
    let started = Instant::now();
    // The producer handle is deliberately not `Sync`, so it cannot be borrowed
    // into a scoped thread -- it has to move. That is the single-producer
    // guarantee being enforced by the compiler, and it is why this reads
    // differently from the model below.
    let producer = thread::spawn(move || {
        for item in 0..ITEMS as u64 {
            let mut item = item;
            while let Err(error) = tx.push(item) {
                item = error.into_inner();
                std::hint::spin_loop();
            }
        }
    });
    let mut taken = 0;
    while taken < ITEMS {
        if rx.pop().is_some() {
            taken += 1;
        } else {
            std::hint::spin_loop();
        }
    }
    let elapsed = started.elapsed().as_nanos() as f64;
    producer.join().expect("the producer must not panic");
    Sample {
        nanos: elapsed,
        // The shipping queue caches nothing, so both sides read the shared line
        // at least once per item. These are floors, not exact counts: `push`
        // also consults `reserved` and the depth metric, and `pop` re-reads on
        // a retry.
        consumer_refreshes: ITEMS as u64,
        producer_refreshes: ITEMS as u64,
    }
}

/// Pads a value onto its own cache line, as `spsc` does.
#[repr(align(128))]
struct CacheAligned<T>(T);

/// A minimal SPSC ring, structurally identical to `spsc`'s.
struct Ring {
    slots: Box<[UnsafeCell<u64>]>,
    mask: usize,
    capacity: usize,
    head: CacheAligned<AtomicUsize>,
    tail: CacheAligned<AtomicUsize>,
}

// SAFETY: the two positions partition the slots between the threads exactly as
// `spsc` does -- a slot in `[head, tail)` belongs to the consumer, one outside
// it to the producer -- and each side publishes its position with a release
// store the other acquires.
unsafe impl Sync for Ring {}

impl Ring {
    fn new(capacity: usize) -> Self {
        let mut slots = Vec::with_capacity(capacity);
        slots.resize_with(capacity, || UnsafeCell::new(0));
        Self {
            slots: slots.into_boxed_slice(),
            mask: capacity - 1,
            capacity,
            head: CacheAligned(AtomicUsize::new(0)),
            tail: CacheAligned(AtomicUsize::new(0)),
        }
    }
}

fn time_model(strategy: Strategy) -> Sample {
    time_model_on(strategy, None, None)
}

/// One run of the model, optionally with each side pinned to a chosen
/// processor.
///
/// The affinity arguments exist so a caller can ask where the two threads run
/// rather than accept wherever the scheduler puts them. That turned out to
/// matter: this probe's headline result inverted between two hosts, and the
/// mechanism -- how deeply the two sides batch -- is a property of how they are
/// placed relative to each other, which an unpinned run leaves to chance and
/// cannot report.
///
/// `None` leaves a side unconstrained, which is what the unpinned entry points
/// pass and is deliberately not the same thing as pinning it to every
/// processor: an unconstrained thread can migrate mid-run.
pub fn time_model_on(
    strategy: Strategy,
    producer_cpu: Option<(u16, u8)>,
    consumer_cpu: Option<(u16, u8)>,
) -> Sample {
    let ring = Ring::new(CAPACITY);
    let started = Instant::now();
    let (consumer_refreshes, producer_refreshes) = thread::scope(|scope| {
        let shared = &ring;
        let producer = scope.spawn(move || {
            pin_current_thread(producer_cpu);
            produce(shared, strategy)
        });
        pin_current_thread(consumer_cpu);
        let consumer_refreshes = consume(&ring, strategy);
        let producer_refreshes = producer.join().expect("the producer must not panic");
        (consumer_refreshes, producer_refreshes)
    });
    Sample {
        nanos: started.elapsed().as_nanos() as f64,
        consumer_refreshes,
        producer_refreshes,
    }
}

/// Confine the calling thread to one logical processor, named by group.
///
/// Panics rather than warns on failure. A silently unpinned thread would turn
/// a placement experiment into a measurement of the scheduler's preferences,
/// and the run would still print a confident number -- the same failure mode as
/// a probe that asserts its conclusion.
///
/// # Why not `SetThreadAffinityMask`
///
/// Its mask is interpreted **within the caller's current group**, so it cannot
/// name a processor in another one. On a machine with more than 64 logical
/// processors that is not a matter of widening the mask; the call has no way to
/// express the target at all. `SetThreadGroupAffinity` takes the group
/// explicitly, and is the only way to pin across the whole machine.
fn pin_current_thread(cpu: Option<(u16, u8)>) {
    let Some((group, number)) = cpu else {
        return;
    };
    assert!(
        u32::from(number) < usize::BITS,
        "processor number {number} does not fit a group affinity mask"
    );

    let affinity = GROUP_AFFINITY {
        Mask: 1_usize << number,
        Group: group,
        Reserved: [0; 3],
    };
    // SAFETY: `affinity` is a fully initialised `GROUP_AFFINITY` naming one
    // processor the caller took from the discovered topology, and the previous
    // affinity is not wanted, so a null pointer is passed for it.
    let ok = unsafe { SetThreadGroupAffinity(GetCurrentThread(), &affinity, ptr::null_mut()) };
    assert!(
        ok != 0,
        "SetThreadGroupAffinity failed for group {group} processor {number}: {}",
        std::io::Error::last_os_error()
    );
}

/// Fills the ring, returning how many times it read the consumer's position.
fn produce(ring: &Ring, strategy: Strategy) -> u64 {
    // The producer's local copy of the consumer's position. Plain, not atomic:
    // it is this thread's alone, and it is only ever a conservative
    // under-estimate of how much room there is.
    let mut cached_head = 0_usize;
    let mut refreshes = 0_u64;

    for item in 0..ITEMS as u64 {
        loop {
            // The producer owns `tail`, so this never leaves its own core.
            let tail = ring.tail.0.load(Ordering::Relaxed);

            let full = match strategy {
                Strategy::Baseline => {
                    refreshes += 1;
                    tail.wrapping_sub(ring.head.0.load(Ordering::Acquire)) == ring.capacity
                }
                Strategy::Warmed => {
                    refreshes += 1;
                    // Discarded: warms the line, and then the authoritative
                    // load happens anyway. `black_box` stops the compiler from
                    // noticing the first load is dead and removing it.
                    black_box(ring.head.0.load(Ordering::Relaxed));
                    tail.wrapping_sub(ring.head.0.load(Ordering::Acquire)) == ring.capacity
                }
                Strategy::Cached => {
                    // The shared line is touched only when the cached copy says
                    // there is no room, which is the whole optimisation.
                    if tail.wrapping_sub(cached_head) == ring.capacity {
                        refreshes += 1;
                        cached_head = ring.head.0.load(Ordering::Acquire);
                    }
                    tail.wrapping_sub(cached_head) == ring.capacity
                }
            };

            if full {
                std::hint::spin_loop();
                continue;
            }

            // SAFETY: `tail` is outside `[head, tail)`, so this slot belongs to
            // the producer and no other thread reads it before the release
            // store below publishes it.
            unsafe {
                *ring.slots[tail & ring.mask].get() = item;
            }
            ring.tail.0.store(tail.wrapping_add(1), Ordering::Release);
            break;
        }
    }

    refreshes
}

/// Drains the ring, returning how many times it read the producer's position.
fn consume(ring: &Ring, strategy: Strategy) -> u64 {
    // The consumer's local copy of the producer's position; see `produce`.
    let mut cached_tail = 0_usize;
    let mut taken = 0_usize;
    let mut refreshes = 0_u64;

    while taken < ITEMS {
        let head = ring.head.0.load(Ordering::Relaxed);

        let empty = match strategy {
            Strategy::Baseline => {
                refreshes += 1;
                head == ring.tail.0.load(Ordering::Acquire)
            }
            Strategy::Warmed => {
                refreshes += 1;
                black_box(ring.tail.0.load(Ordering::Relaxed));
                head == ring.tail.0.load(Ordering::Acquire)
            }
            Strategy::Cached => {
                // One acquire load of `tail` per *batch*: everything that
                // snapshot made visible is then drained with no shared reads at
                // all. This is the load the technique is named for.
                if head == cached_tail {
                    refreshes += 1;
                    cached_tail = ring.tail.0.load(Ordering::Acquire);
                }
                head == cached_tail
            }
        };

        if empty {
            std::hint::spin_loop();
            continue;
        }

        // SAFETY: `head` is in `[head, tail)`, so the producer wrote this slot
        // and released it; the acquire above makes that write visible here.
        let item = unsafe { *ring.slots[head & ring.mask].get() };
        black_box(item);
        ring.head.0.store(head.wrapping_add(1), Ordering::Release);
        taken += 1;
    }

    refreshes
}
