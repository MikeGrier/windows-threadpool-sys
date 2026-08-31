// Copyright (c) Mike Grier.

//! Does it matter *where* the two ends of a queue run?
//!
//! # The question
//!
//! A machine with two efficiency classes and two L2 domains offers four kinds
//! of producer/consumer placement: same cache and same class, same cache and
//! different class, different cache and same class, different cache and
//! different class. This probe measures an SPSC handoff under each, with the
//! two threads pinned rather than left to the scheduler.
//!
//! # Why it was written, which is not the obvious reason
//!
//! [`crate::peer_index_cache`] found that caching the peer's index costs about
//! 1.8x on an x64 host and *wins* about 17x on an ARM64 one. That decided a
//! design question (`D-28`) in opposite directions on two machines, which is an
//! uncomfortable place to leave it.
//!
//! The explanation both hosts agree on is **batch depth**: caching trades
//! freshness for fewer shared reads, and that trade pays only when each read is
//! amortised over many items. What neither run explained is *why* the depth
//! differed by two orders of magnitude. The hypothesis this probe exists to
//! test is that depth is set by how evenly matched the two threads are:
//!
//! - Two threads of **equal** speed stay in lockstep. The ring hovers near
//!   empty, every operation finds it empty, and the batch is one item deep.
//! - Two threads of **unequal** speed decouple. The faster side runs ahead, a
//!   real backlog forms, and the batch is as deep as the backlog.
//!
//! If that is right, then the x64/ARM64 split is not about the architecture at
//! all. It is about **homogeneous versus heterogeneous cores** -- the x64 host
//! has one class of core, this one has two -- and the deciding factor is
//! whether the producer and consumer landed on cores of the same class. That
//! would be a far more useful thing to know than "ARM64 is different", because
//! it names a condition a caller could actually reason about.
//!
//! The probe is built to be able to **refute** that, which matters more than
//! its ability to confirm it. If placement turns out not to move batch depth,
//! the hypothesis is wrong and the host difference needs another explanation;
//! the numbers below say so either way.
//!
//! # What it controls for
//!
//! Cross-class pairs differ in two ways at once -- the cores run at different
//! speeds *and*, on this machine, they sit behind different L2 domains. Those
//! are separable only if same-class cross-cache pairs exist, which is why every
//! available combination is measured rather than only the interesting one. A
//! same-class pair spanning two caches isolates the cache effect; a
//! different-class pair sharing a cache, where the hardware provides one,
//! isolates the speed effect.
//!
//! # Reading it
//!
//! Absolute nanoseconds are host-specific and not the point. Two things are:
//! whether **batch depth** tracks class mismatch, and whether the **verdict on
//! caching** flips between placements on a single machine. The second is the
//! one with consequences, because a technique whose sign depends on where two
//! threads happen to be scheduled cannot be adopted or rejected by a fixed
//! decision at all.
//!
//! Run in **release**. A debug build's overhead buries coherence effects, which
//! is the same reason the two probes this one extends are absent from CI.

use std::collections::BTreeMap;

use crate::fingerprint::{ProcessorPlace, Slice, discover_places};
use crate::peer_index_cache::{ITEMS, Strategy, time_model_on};

/// Repetitions per placement; the median is reported.
///
/// Odd, so the median is an observation rather than an average of two.
const REPETITIONS: usize = 3;

/// How a producer and a consumer are placed relative to each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Placement {
    /// Two SMT siblings: the same physical core, sharing L1.
    ///
    /// Listed first because it is the tightest coupling a machine can offer,
    /// and kept distinct from `SameCacheSameClass` for a measured reason: on an
    /// SMT host a sibling pair and a two-core pair behind one cache would
    /// otherwise land in the same bucket, and the probe would report whichever
    /// it happened to select. That is precisely the distinction needed to
    /// explain why peer-index caching loses on an SMT x64 host and wins on a
    /// non-SMT ARM64 one -- siblings sharing L1 have every reason to stay in
    /// lockstep, which is the shallow-batch condition that makes caching lose.
    ///
    /// Absent on a machine without SMT, where it is reported inexpressible
    /// rather than merged into another category.
    SameCoreSiblings,
    /// Same cache domain, same efficiency class, but different physical cores.
    SameCacheSameClass,
    /// Same cache domain, different efficiency class.
    SameCacheCrossClass,
    /// Different cache domain, same efficiency class.
    CrossCacheSameClass,
    /// Different cache domain, different efficiency class.
    CrossCacheCrossClass,
    /// Different NUMA nodes.
    ///
    /// Listed last because it is the loosest coupling a machine can offer, and
    /// classified *first* for the same reason `SameCoreSiblings` is: crossing a
    /// node dominates any statement about the cache domain or the class, since
    /// two nodes necessarily have different last-level caches anyway.
    ///
    /// Kept as its own bucket rather than merged into `CrossCache*` because the
    /// merge is silent and the run that would expose it is the expensive one. A
    /// machine with real NUMA would otherwise report a node crossing under a
    /// cache label, with nothing in the output saying which had been measured.
    ///
    /// Absent on every host measured so far -- all three are VM slices that
    /// present a single node -- and reported inexpressible rather than merged.
    CrossNumaNode,
}

impl Placement {
    /// A short label for a table.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::SameCoreSiblings => "SMT siblings (one core)",
            Self::SameCacheSameClass => "same cache, same class",
            Self::SameCacheCrossClass => "same cache, cross class",
            Self::CrossCacheSameClass => "cross cache, same class",
            Self::CrossCacheCrossClass => "cross cache, cross class",
            Self::CrossNumaNode => "cross NUMA node",
        }
    }
}

/// One placement measured under one strategy.
#[derive(Debug, Clone)]
pub struct Measurement {
    /// Exactly which processors this number came from.
    ///
    /// Carried on the measurement rather than printed once in a banner: a
    /// table of numbers from different slices is the thing that misleads, and
    /// the only defence is for each row to know its own provenance.
    pub slice: Slice,
    /// Which processor produced.
    pub producer: ProcessorPlace,
    /// Which processor consumed.
    pub consumer: ProcessorPlace,
    /// How the two are related.
    pub placement: Placement,
    /// Which strategy was measured.
    pub strategy: Strategy,
    /// Median nanoseconds per item.
    pub nanos_per_item: f64,
    /// How many items each consumer-side shared read was amortised over.
    ///
    /// **This is the number the hypothesis is about.** Near 1 means the two
    /// sides stayed in lockstep; large means they decoupled and a real backlog
    /// formed.
    pub consumer_batch: f64,
    /// The same for the producer side.
    pub producer_batch: f64,
}

/// Everything one invocation measured.
#[derive(Debug, Clone)]
pub struct Observation {
    /// Every logical processor, as discovered.
    pub processors: Vec<ProcessorPlace>,
    /// One within-class, within-cache pair per efficiency class.
    ///
    /// Separate from [`Self::measurements`] because the placement categories
    /// collapse every same-class pair into one row, which would answer "does
    /// separation cost" while silently skipping "are the fast cores faster".
    /// On a machine whose classes are named for their speed, that second
    /// question is the one a reader asks first.
    pub by_class: Vec<Measurement>,
    /// Which placements this machine can actually express.
    ///
    /// Not every machine offers all four: one with a single cache domain cannot
    /// produce a cross-cache pair, and a homogeneous one cannot produce a
    /// cross-class pair. A placement that is absent is reported as absent
    /// rather than silently skipped, because "this host cannot test that" and
    /// "that made no difference" are opposite findings.
    pub measurements: Vec<Measurement>,
    /// One measurement per *distinct pair of NUMA nodes*.
    ///
    /// Separate from [`Self::measurements`] for the same reason [`Self::by_class`]
    /// is: the placement categories collapse every node crossing into a single
    /// `CrossNumaNode` row, which answers "does leaving the node cost" while
    /// silently assuming every node is equidistant. On real multi-node hardware
    /// they are not -- two nodes on one package are far closer than two across a
    /// socket link -- so a single row would report whichever hop the enumeration
    /// happened to reach first.
    ///
    /// Empty on a single-node machine, and a single entry on a two-node one,
    /// where it restates the `CrossNumaNode` row rather than adding to it.
    pub by_node_pair: Vec<Measurement>,
}

/// Classify a pair.
#[must_use]
pub fn classify(producer: ProcessorPlace, consumer: ProcessorPlace) -> Placement {
    // Tested first: two processors on one core share L1, which dominates any
    // statement about the cache domain or the class they also share.
    if producer.core == consumer.core {
        return Placement::SameCoreSiblings;
    }
    // Tested before cache and class for the mirror-image reason: crossing a
    // NUMA node dominates both, since separate nodes have separate last-level
    // caches. Without this the pair would be reported under a cache label and
    // the node crossing would be invisible.
    if producer.numa_node != consumer.numa_node {
        return Placement::CrossNumaNode;
    }
    let same_cache = producer.cache_domain == consumer.cache_domain;
    let same_class = producer.efficiency_class == consumer.efficiency_class;
    match (same_cache, same_class) {
        (true, true) => Placement::SameCacheSameClass,
        (true, false) => Placement::SameCacheCrossClass,
        (false, true) => Placement::CrossCacheSameClass,
        (false, false) => Placement::CrossCacheCrossClass,
    }
}

/// Choose one representative processor pair for each placement this machine can
/// express.
///
/// One pair per placement rather than an exhaustive sweep: the question is
/// whether the *category* moves the result, and 12 processors would otherwise
/// mean 132 ordered pairs times three strategies times three repetitions.
#[must_use]
pub fn representative_pairs(
    places: &[ProcessorPlace],
) -> BTreeMap<Placement, (ProcessorPlace, ProcessorPlace)> {
    let mut chosen = BTreeMap::new();
    for producer in places {
        for consumer in places {
            if producer.number == consumer.number {
                // One processor cannot be both ends: that measures the
                // scheduler time-slicing a thread against itself. Two
                // processors on one *core* are a different matter entirely and
                // are measured, as `Placement::SameCoreSiblings`.
                continue;
            }
            chosen
                .entry(classify(*producer, *consumer))
                .or_insert((*producer, *consumer));
        }
    }
    chosen
}

/// Choose one representative processor pair for each *distinct pair of NUMA
/// nodes*.
///
/// Keyed by `(low, high)` node id, so a node pair appears once rather than once
/// per direction: this measures the link, and `0 -> 1` and `1 -> 0` traverse the
/// same one. The producer is always on the lower-numbered node, which makes a
/// run reproducible rather than dependent on enumeration order.
///
/// # Why this exists separately from [`representative_pairs`]
///
/// Windows exposes no NUMA distance table -- there is no Win32 equivalent of
/// reading ACPI SLIT -- so the only way to learn that two nodes are further
/// apart than another two is to measure the handoff between them. A single
/// `CrossNumaNode` row cannot express that, because it reports one hop and
/// implies every hop is like it.
///
/// Empty on a single-node machine: there is no node crossing to represent, and
/// an empty result says so more honestly than a fabricated self-pair.
#[must_use]
pub fn node_pairs(
    places: &[ProcessorPlace],
) -> BTreeMap<(u32, u32), (ProcessorPlace, ProcessorPlace)> {
    let mut chosen = BTreeMap::new();
    for producer in places {
        for consumer in places {
            if producer.numa_node >= consumer.numa_node {
                // `>=` rather than `!=` collapses the two directions onto the
                // canonical `(low, high)` key and drops same-node pairs, which
                // are not a crossing at all.
                continue;
            }
            chosen
                .entry((producer.numa_node, consumer.numa_node))
                .or_insert((*producer, *consumer));
        }
    }
    chosen
}

/// Measure every expressible placement under baseline and cached strategies.
///
/// # Errors
///
/// Returns whatever [`discover_places`] failed with.
pub fn measure() -> std::io::Result<Observation> {
    let processors = discover_places()?;
    let pairs = representative_pairs(&processors);
    let mut measurements = Vec::new();

    for (placement, (producer, consumer)) in pairs {
        for strategy in [Strategy::Baseline, Strategy::Cached] {
            let mut samples: Vec<_> = (0..REPETITIONS)
                .map(|_| time_model_on(strategy, Some(producer.number), Some(consumer.number)))
                .collect();
            samples.sort_by(|a, b| a.nanos.total_cmp(&b.nanos));
            let median = samples[samples.len() / 2];

            measurements.push(Measurement {
                slice: Slice::pair(producer, consumer),
                producer,
                consumer,
                placement,
                strategy,
                nanos_per_item: median.nanos / ITEMS as f64,
                consumer_batch: ITEMS as f64 / median.consumer_refreshes.max(1) as f64,
                producer_batch: ITEMS as f64 / median.producer_refreshes.max(1) as f64,
            });
        }
    }

    // One same-class, same-cache pair per efficiency class, so "are the fast
    // cores faster at this" is answerable and not folded into a single
    // same-class row.
    let mut by_class = Vec::new();
    let mut classes: Vec<u8> = processors.iter().map(|p| p.efficiency_class).collect();
    classes.sort_unstable();
    classes.dedup();
    for class in classes {
        let members: Vec<_> = processors
            .iter()
            .filter(|p| p.efficiency_class == class)
            .collect();
        let Some((producer, consumer)) = members
            .iter()
            .flat_map(|a| members.iter().map(move |b| (**a, **b)))
            // Different cores, not merely different processors: on an SMT host
            // one class might otherwise be measured as siblings and the other
            // as two cores, and the comparison between classes would be
            // measuring the placement difference instead.
            .find(|(a, b)| a.core != b.core && a.cache_domain == b.cache_domain)
        else {
            continue;
        };
        for strategy in [Strategy::Baseline, Strategy::Cached] {
            let mut samples: Vec<_> = (0..REPETITIONS)
                .map(|_| time_model_on(strategy, Some(producer.number), Some(consumer.number)))
                .collect();
            samples.sort_by(|a, b| a.nanos.total_cmp(&b.nanos));
            let median = samples[samples.len() / 2];
            by_class.push(Measurement {
                slice: Slice::pair(producer, consumer),
                producer,
                consumer,
                placement: classify(producer, consumer),
                strategy,
                nanos_per_item: median.nanos / ITEMS as f64,
                consumer_batch: ITEMS as f64 / median.consumer_refreshes.max(1) as f64,
                producer_batch: ITEMS as f64 / median.producer_refreshes.max(1) as f64,
            });
        }
    }

    let mut by_node_pair = Vec::new();
    for ((left, right), (producer, consumer)) in node_pairs(&processors) {
        debug_assert_eq!((producer.numa_node, consumer.numa_node), (left, right));
        for strategy in [Strategy::Baseline, Strategy::Cached] {
            let mut samples: Vec<_> = (0..REPETITIONS)
                .map(|_| time_model_on(strategy, Some(producer.number), Some(consumer.number)))
                .collect();
            samples.sort_by(|a, b| a.nanos.total_cmp(&b.nanos));
            let median = samples[samples.len() / 2];
            by_node_pair.push(Measurement {
                slice: Slice::pair(producer, consumer),
                producer,
                consumer,
                placement: classify(producer, consumer),
                strategy,
                nanos_per_item: median.nanos / ITEMS as f64,
                consumer_batch: ITEMS as f64 / median.consumer_refreshes.max(1) as f64,
                producer_batch: ITEMS as f64 / median.producer_refreshes.max(1) as f64,
            });
        }
    }

    Ok(Observation {
        processors,
        by_class,
        measurements,
        by_node_pair,
    })
}

impl Observation {
    /// The measurement for one placement and strategy, if it was taken.
    #[must_use]
    pub fn get(&self, placement: Placement, strategy: Strategy) -> Option<Measurement> {
        self.measurements
            .iter()
            .find(|m| m.placement == placement && m.strategy == strategy)
            .cloned()
    }

    /// Every node pair measured, in canonical `(low, high)` order.
    #[must_use]
    pub fn node_pairs_measured(&self) -> Vec<(u32, u32)> {
        let mut seen: Vec<(u32, u32)> = self
            .by_node_pair
            .iter()
            .map(|m| (m.producer.numa_node, m.consumer.numa_node))
            .collect();
        seen.sort_unstable();
        seen.dedup();
        seen
    }

    /// The measurement for one node pair and strategy, if it was taken.
    #[must_use]
    pub fn node_pair(&self, pair: (u32, u32), strategy: Strategy) -> Option<Measurement> {
        self.by_node_pair
            .iter()
            .find(|m| {
                (m.producer.numa_node, m.consumer.numa_node) == pair && m.strategy == strategy
            })
            .cloned()
    }

    /// Which placements this machine could express.
    #[must_use]
    pub fn placements(&self) -> Vec<Placement> {
        let mut seen: Vec<Placement> = self.measurements.iter().map(|m| m.placement).collect();
        seen.sort_unstable();
        seen.dedup();
        seen
    }
}

#[cfg(test)]
mod tests;
