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
//! # It did refute it, and that is the finding
//!
//! Read the above as the question, not the answer. The x64 host has answered
//! it, and the equal-speed half of the hypothesis is **wrong in the direction
//! nobody predicted**: SMT siblings -- as evenly matched as two threads can be,
//! sharing L1 -- produce the *deepest* batches on that host, not the shallowest,
//! and caching wins on them by 1.8x. The shallow batches are between cores.
//!
//! What survives is the part that matters: batch depth is set by **placement**,
//! the verdict on caching flips inside a single machine, and no rule keyed to
//! the instruction set can be right. The mechanism proposed for it did not
//! survive. See
//! [DESIGN-NOTES.md](../../windows-waitable-queues/DESIGN-NOTES.md) `D-28`.
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
use crate::peer_index_cache::{ITEMS, Strategy, time_model_on, time_model_placed};

/// Repetitions per placement; the median is reported.
///
/// Odd, so the median is an observation rather than an average of two.
pub const REPETITIONS: usize = 3;

/// The within-class pair a run would measure for one efficiency class, if any.
///
/// **Extracted so the plan and the run bind to one definition.** Restating this
/// predicate in [`RunPlan`] would be a second copy of a rule, and the two would
/// drift -- the plan would promise a comparison the run then skipped, or the
/// reverse. Here the plan asks the same function the run uses.
///
/// Requires different *cores*, not merely different processors: on an SMT host
/// one class might otherwise be measured as siblings and another as two cores,
/// and the comparison between classes would be measuring the placement
/// difference instead.
#[must_use]
fn within_class_pair(
    places: &[ProcessorPlace],
    class: u8,
) -> Option<(ProcessorPlace, ProcessorPlace)> {
    let members: Vec<&ProcessorPlace> = places
        .iter()
        .filter(|place| place.efficiency_class == class)
        .collect();

    members
        .iter()
        .flat_map(|a| members.iter().map(move |b| (**a, **b)))
        .find(|(a, b)| a.core != b.core && a.cache_domain == b.cache_domain)
}

/// Every efficiency class this machine has.
#[must_use]
fn efficiency_classes(places: &[ProcessorPlace]) -> Vec<u8> {
    let mut classes: Vec<u8> = places.iter().map(|place| place.efficiency_class).collect();
    classes.sort_unstable();
    classes.dedup();
    classes
}

/// What a run on this machine will involve, worked out before any of it starts.
///
/// # Why this is computed rather than estimated
///
/// A person is being asked to give up minutes of their machine as a favour, and
/// on a large multi-socket host the hop work alone grows as `2*n*(n-1)`: every
/// *ordered* pair of nodes, because a hop is directed, and each of those
/// measured at both ring placements. An earlier version of this note said
/// `n*(n-1)/2`, the undirected count, which understated the dominant term by a
/// factor of four -- 6 hops rather than 24 on a four-node host.
/// The *counts* here are exact -- they come from the same selection the run
/// will use -- so only the per-run duration is approximate, and it is presented
/// as an upper bound rather than a single confident number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RunPlan {
    /// Placements this machine can express.
    pub placements: usize,
    /// Directed NUMA node pairs: `(a, b)` and `(b, a)` are both counted.
    ///
    /// A hop is not symmetric even though the link is. The producer writes and
    /// the consumer reads, so swapping the endpoints swaps which side pays for
    /// the crossing.
    pub node_hops: usize,
    /// Ring placements measured per hop -- see [`memory_placements`].
    pub memory_placements_per_hop: usize,
    /// Efficiency classes compared like with like.
    pub classes: usize,
    /// Strategies measured per selection.
    pub strategies: usize,
    /// Repetitions per strategy.
    pub repetitions: usize,
}

impl RunPlan {
    /// Work out what a run on these processors will involve.
    #[must_use]
    pub fn for_processors(places: &[ProcessorPlace]) -> Self {
        let hops = node_pairs(places);
        Self {
            placements: representative_pairs(places).len(),
            node_hops: hops.len(),
            // Asked, not assumed. Taken from a hop this run will actually
            // perform, so a change to `memory_placements` moves the promise
            // with it. Any value on a machine with no hops, since it multiplies
            // a zero.
            memory_placements_per_hop: hops.values().next().map_or(0, |(producer, consumer)| {
                memory_placements(*producer, *consumer).len()
            }),
            // Exact, not an upper bound: a class is only measured when it has a
            // usable within-class pair, and this asks the same function the run
            // will. Counting classes instead would over-report on any host
            // whose classes have no such pair -- which is this workspace's own
            // x64 host, where it inflated the count by half.
            classes: efficiency_classes(places)
                .into_iter()
                .filter(|class| within_class_pair(places, *class).is_some())
                .count(),
            strategies: 2,
            repetitions: REPETITIONS,
        }
    }

    /// How many timed handoffs the run performs.
    ///
    /// Hops are counted separately from the rest because they are the only
    /// selections measured at more than one memory placement.
    #[must_use]
    pub fn timed_runs(self) -> usize {
        let selections =
            self.placements + self.classes + self.node_hops * self.memory_placements_per_hop;
        selections * self.strategies * self.repetitions
    }

    /// How long the run should take at most, in seconds.
    ///
    /// **An upper bound rather than a range, and that is a correction.** An
    /// earlier version quoted a low-to-high range, and the first real run
    /// finished in 0.6 s against a stated "roughly 1-8 seconds" -- under its own
    /// floor. A floor is the useless half of the promise anyway: someone
    /// deciding whether to start a favour needs to know the worst case, and
    /// finishing early is never the failure.
    ///
    /// 220 ns/item is the slowest per-item cost measured across the hosts this
    /// has run on, seen crossing a distant domain. A machine slower than that
    /// will overrun the estimate, and its result is exactly the one worth
    /// having.
    #[must_use]
    pub fn estimated_seconds(self) -> f64 {
        (self.timed_runs() * ITEMS) as f64 * 220e-9
    }
}

/// How a producer and a consumer are placed relative to each other.
///
/// # A label names a relationship, not a direction
///
/// These names are deliberately symmetric, and that is not an oversight left
/// over from before hops became directed. The *relationship* between two
/// processors genuinely is symmetric -- two processors either are SMT siblings
/// or are not, share a cache domain or do not -- so there is no honest
/// `CrossNumaNodeForward` to name. Splitting the labels by direction would
/// invent a distinction the topology does not have.
///
/// The *workload* is what is asymmetric: the producer writes and the consumer
/// reads, so swapping them swaps which side pays. Direction therefore lives
/// where it is real, not in the label:
///
/// - in the slice, whose participants carry `prod=` and `cons=` roles;
/// - in the node-pair column of the hop table, printed `a -> b`;
/// - in the ring's node, recorded per row as [`Measurement::memory_node`].
///
/// This matters when reading a table. A `CrossNumaNode` row is **one** direction
/// at **one** memory placement, never a summary of the four measurements an
/// edge admits. Taking it for a summary is the failure this note exists to
/// prevent: right labels over pairs that do not cover what the reader assumes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Placement {
    /// Two SMT siblings: the same physical core, sharing L1.
    ///
    /// Listed first because it is the tightest coupling a machine can offer,
    /// and kept distinct from `SameCacheSameClass` for a measured reason: on an
    /// SMT host a sibling pair and a two-core pair behind one cache would
    /// otherwise land in the same bucket, and the probe would report whichever
    /// it happened to select.
    ///
    /// **That distinction refuted the hypothesis it was built to test, and this
    /// comment used to state the refuted version.** The prediction was that
    /// siblings sharing L1 stay in lockstep, giving shallow batches, and that
    /// this was the condition making peer-index caching lose. Measurement says
    /// the opposite: siblings produce by far the *deepest* batches on an x64
    /// host and caching wins there by 1.8x, while the shallow batches -- and the
    /// loss -- are on the cross-core row. Sharing a cache decouples the two
    /// sides rather than locking them together.
    ///
    /// The verdict therefore flips *inside* one machine, which is why no rule
    /// keyed to the instruction set can be right. See
    /// [DESIGN-NOTES.md](../../windows-waitable-queues/DESIGN-NOTES.md)
    /// `D-28`, and note that this probe reproduces those numbers on the host it
    /// was written on -- which is the point of shipping it.
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
    /// Which NUMA node held the ring's slots, when a placement was arranged.
    ///
    /// None on a run that asked for none, and on one whose placement could
    /// not be achieved -- the two are the same fact here (we do not know where
    /// the memory is) and neither may be reported as a node.
    pub memory_node: Option<u32>,
    /// Which NUMA node this run *asked* for, independent of what it got.
    ///
    /// **The request is what identifies the row; the result is what it
    /// measured, and they are not the same fact.** A directed hop is measured
    /// once per ring placement, so the two rows for a pair differ only in what
    /// they requested -- and Windows may redirect an allocation, which
    /// `Slots::new_on` deliberately tolerates rather than failing. Recording
    /// only the achieved node therefore lets both rows serialise identically,
    /// collapsing the very dimension measuring both placements exists to
    /// expose.
    ///
    /// It also makes a redirect visible instead of silent: a row whose
    /// requested and observed nodes disagree did not measure the placement it
    /// names, and a reader can now see that rather than infer it.
    ///
    /// None means nothing was requested, which is the normal case for the
    /// placement and efficiency-class rows.
    pub requested_memory_node: Option<u32>,
}

/// Everything one invocation measured.
#[derive(Debug, Clone)]
pub struct Observation {
    /// Every logical processor, as discovered.
    pub processors: Vec<ProcessorPlace>,
    /// One within-class, within-cache pair per efficiency class, per strategy.
    ///
    /// Two rows per class, not one: the pair is chosen once and then measured
    /// under each strategy, which is the comparison the rows exist to support.
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
    /// One measurement per *directed* node pair, ring placement and strategy.
    ///
    /// **Three dimensions, not one, and the count is their product.** A hop is
    /// directed, because the producer writes and the consumer reads, so `(a, b)`
    /// and `(b, a)` are different measurements; each of those is measured with
    /// the ring on each endpoint's node; and each of those under each strategy.
    /// A two-node machine therefore contributes eight rows here, not one. An
    /// earlier version of this note promised one entry per distinct pair, which
    /// would lead a consumer to treat these rows as unique hops and to collapse
    /// the producer-local and consumer-local measurements into each other --
    /// the two quantities the ring placement exists to separate.
    ///
    /// Separate from [`Self::measurements`] for the same reason [`Self::by_class`]
    /// is: the placement categories collapse every node crossing into a single
    /// `CrossNumaNode` row, which answers "does leaving the node cost" while
    /// silently assuming every node is equidistant. On real multi-node hardware
    /// they are not -- two nodes on one package are far closer than two across a
    /// socket link -- so a single row would report whichever hop the enumeration
    /// happened to reach first.
    ///
    /// Empty on a single-node machine. A two-node machine yields **eight**
    /// rows -- two directed pairs, each at two ring placements, each under two
    /// strategies -- which is the product described above. This sentence said
    /// "a single entry" while the paragraph above it said three dimensions,
    /// which is the same doc comment contradicting itself.
    pub by_node_pair: Vec<Measurement>,
}

/// Classify a pair.
#[must_use]
pub fn classify(producer: ProcessorPlace, consumer: ProcessorPlace) -> Placement {
    // Tested first: two processors on one core share L1, which dominates any
    // statement about the cache domain or the class they also share.
    //
    // The group must match as well. A physical core cannot span a processor
    // group, so two processors reporting the same core id in different groups
    // are different cores whose ids collide -- and calling them SMT siblings
    // would attribute a shared L1 that does not exist.
    if producer.group == consumer.group && producer.core == consumer.core {
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
            if producer.id() == consumer.id() {
                // One processor cannot be both ends: that measures the
                // scheduler time-slicing a thread against itself. Two
                // processors on one *core* are a different matter entirely and
                // are measured, as `Placement::SameCoreSiblings`.
                //
                // **The full identity, not the number.** A number is unique
                // only within its processor group, so comparing numbers alone
                // treats group 0's processor 5 and group 1's processor 5 as one
                // processor and skips the pair -- discarding a real,
                // cross-group placement on exactly the large machines this tool
                // exists to measure. That is the same defect M1B removed from
                // `classify` and the selection maps; this comparison was
                // missed.
                continue;
            }
            chosen
                .entry(classify(*producer, *consumer))
                .or_insert((*producer, *consumer));
        }
    }
    chosen
}

/// Stop the run if this machine presents processor groups the tool cannot
/// honestly handle.
///
/// Currently a no-op beyond the check itself, because groups *are* handled --
/// every identity is a `(group, number)` pair and pinning goes through
/// `SetThreadGroupAffinity`. It exists as the place a future limitation is
/// declared, and it is deliberately loud rather than silent.
///
/// # Why a refusal rather than a best effort
///
/// A tool that quietly measures whatever subset it understands is worse than
/// one that stops, because its output is indistinguishable from a complete run.
/// The large multi-socket machines this exists for are borrowed, measured once,
/// and not available again: a wrong answer there is not a wrong answer we get to
/// correct. A refusal costs one message.
///
/// # Panics
///
/// If the discovered processors cannot be measured as they are.
fn assert_group_support(processors: &[ProcessorPlace]) {
    assert!(
        !processors.is_empty(),
        "no processors were discovered, so there is nothing to measure"
    );
    // Every discovered processor must be pinnable. A number at or above the
    // width of an affinity mask cannot be expressed in one, and measuring the
    // rest while dropping it would report a machine smaller than the real one.
    for place in processors {
        assert!(
            u32::from(place.number) < usize::BITS,
            "processor {place} has a number no affinity mask can express; \
             this machine cannot be measured honestly and the run is stopping"
        );
    }
}

/// Choose one representative processor pair for each *distinct pair of NUMA
/// nodes*.
///
/// Keyed by `(producer node, consumer node)`, and **both directions are
/// measured**.
///
/// An earlier version keyed on `(low, high)` and kept one direction, reasoning
/// that the two traverse the same link. That conflates the *link*, which is
/// symmetric, with the *workload over it*, which is not: the producer **writes**
/// slots and release-stores `tail` while the consumer **reads** them and
/// release-stores `head`, and a remote write needs exclusive ownership and
/// invalidation where a remote read does not. Swapping the ends is a different
/// measurement rather than a repeat of one.
///
/// Combined with the two memory placements in [`measure`], that gives four
/// configurations per undirected edge and `2*n*(n-1)` hop measurements in all.
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
            if producer.numa_node == consumer.numa_node {
                // Same node is not a crossing. Both *directions* are kept: see
                // the note above on why they are different measurements.
                continue;
            }
            chosen
                .entry((producer.numa_node, consumer.numa_node))
                .or_insert((*producer, *consumer));
        }
    }
    chosen
}

/// Where the ring is placed for one directed node hop.
///
/// **One definition, asked by both the run and the plan.** The hop loop iterates
/// this to decide what to measure, and [`RunPlan`] asks its length to decide
/// what to promise. Restating the count in the plan is how the plan came to
/// under-report once already: an earlier version quoted 18 timed handoffs
/// against a run that performed 12, because the two counted independently.
///
/// The two entries are the two quantities a single row would average away. With
/// the ring on the producer's node the producer writes locally and the consumer
/// reads across; on the consumer's node that reverses. Remote-write and
/// remote-read are not interchangeable, and on some interconnects they are not
/// even close.
#[must_use]
pub fn memory_placements(producer: ProcessorPlace, consumer: ProcessorPlace) -> [u32; 2] {
    [producer.numa_node, consumer.numa_node]
}

/// Measure every expressible placement under baseline and cached strategies.
///
/// # Do not add a seam here to inject a processor list
///
/// This reads the real machine on purpose, and the classification functions it
/// calls -- [`classify`], [`representative_pairs`], [`node_pairs`] -- are pure
/// so that they, and not this, are what synthetic topologies exercise. A
/// `measure_with(places)` overload would look like an obvious testability
/// improvement and would be a trap:
///
/// [`ProcessorPlace::numa_node`] and [`ProcessorPlace::number`] are independent
/// fields. Feed a synthetic four-node list to a sixteen-processor single-node
/// host and every processor number in it is still *valid*, so every pin
/// succeeds and the run produces genuine timings filed under fabricated node
/// ids -- a table indistinguishable from a real NUMA measurement that measured
/// no such thing. The pin assertion does not catch it: it rejects a processor
/// that does not exist, not a label that is wrong.
///
/// Selection is testable offline and is tested there; the timings need real
/// hardware and are worth nothing without it.
///
/// # Errors
///
/// Returns whatever [`discover_places`] failed with.
pub fn measure() -> std::io::Result<Observation> {
    let processors = discover_places()?;
    assert_group_support(&processors);
    let pairs = representative_pairs(&processors);
    let mut measurements = Vec::new();

    for (placement, (producer, consumer)) in pairs {
        for strategy in [Strategy::Baseline, Strategy::Cached] {
            let mut samples: Vec<_> = (0..REPETITIONS)
                .map(|_| time_model_on(strategy, Some(producer.id()), Some(consumer.id())))
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
                // Placement rows do not choose a node: they vary where the
                // threads run, holding everything else as it falls.
                memory_node: median.memory_node,
                requested_memory_node: None,
            });
        }
    }

    // One same-class, same-cache pair per efficiency class, so "are the fast
    // cores faster at this" is answerable and not folded into a single
    // same-class row.
    let mut by_class = Vec::new();
    for class in efficiency_classes(&processors) {
        // The same function `RunPlan` asks, so the estimate a runner is shown
        // cannot promise a comparison this loop then skips.
        let Some((producer, consumer)) = within_class_pair(&processors, class) else {
            continue;
        };
        for strategy in [Strategy::Baseline, Strategy::Cached] {
            let mut samples: Vec<_> = (0..REPETITIONS)
                .map(|_| time_model_on(strategy, Some(producer.id()), Some(consumer.id())))
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
                // Placement rows do not choose a node: they vary where the
                // threads run, holding everything else as it falls.
                memory_node: median.memory_node,
                requested_memory_node: None,
            });
        }
    }

    let mut by_node_pair = Vec::new();
    for ((left, right), (producer, consumer)) in node_pairs(&processors) {
        debug_assert_eq!((producer.numa_node, consumer.numa_node), (left, right));
        // Both memory placements, as separate rows -- see `memory_placements`,
        // which is also what the plan counts, so the two cannot disagree.
        for memory_node in memory_placements(producer, consumer) {
            for strategy in [Strategy::Baseline, Strategy::Cached] {
                let mut samples: Vec<_> = (0..REPETITIONS)
                    .map(|_| {
                        time_model_placed(
                            strategy,
                            Some(producer.id()),
                            Some(consumer.id()),
                            Some(memory_node),
                        )
                    })
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
                    // What the run *achieved*, and separately what it asked
                    // for. Both are needed: the request identifies the row,
                    // the result is the measurement, and a disagreement
                    // between them is itself the finding.
                    memory_node: median.memory_node,
                    requested_memory_node: Some(memory_node),
                });
            }
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

    /// Every node pair measured, as *directed* `(producer, consumer)` pairs.
    ///
    /// Both `(0, 1)` and `(1, 0)` appear, because they are different
    /// measurements rather than two spellings of one. This said "canonical
    /// `(low, high)` order" while returning the directed pairs, so a caller
    /// trusting the documentation would have deduplicated away half the hops.
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

    /// Every measurement for one directed node pair and strategy.
    ///
    /// **Plural, and that is a correction rather than a preference.** A pair and
    /// a strategy no longer identify one measurement: each hop is measured once
    /// per ring placement, so there are two. The singular version of this
    /// returned the first match, which silently discarded half the rows and
    /// handed back whichever placement happened to be pushed first -- so a
    /// caller comparing baseline against cached could unknowingly compare a
    /// producer-local run with a consumer-local one.
    ///
    /// Ordered as measured, which is the producer's node first.
    #[must_use]
    pub fn node_pair_rows(&self, pair: (u32, u32), strategy: Strategy) -> Vec<Measurement> {
        self.by_node_pair
            .iter()
            .filter(|m| {
                (m.producer.numa_node, m.consumer.numa_node) == pair && m.strategy == strategy
            })
            .cloned()
            .collect()
    }

    /// The measurement for one node pair, strategy and *requested* ring
    /// placement.
    ///
    /// The full key. `requested_memory_node` is what the singular lookup used
    /// to omit entirely, and then keyed on the achieved node instead.
    ///
    /// **Keyed on the request, because the request is what identifies the
    /// row.** Windows may satisfy an allocation on a node other than the one
    /// asked for, so two rows of a pair can share an achieved node while
    /// describing different placements. Keyed on that, one requested placement
    /// becomes unfindable and a lookup for the other returns whichever row
    /// happens to come first -- silently pairing a baseline taken at one
    /// placement against a cached run taken at the other, which is precisely
    /// the mistake the caller's own comment says this key exists to prevent.
    #[must_use]
    pub fn node_pair(
        &self,
        pair: (u32, u32),
        strategy: Strategy,
        requested_memory_node: Option<u32>,
    ) -> Option<Measurement> {
        self.by_node_pair
            .iter()
            .find(|m| {
                (m.producer.numa_node, m.consumer.numa_node) == pair
                    && m.strategy == strategy
                    && m.requested_memory_node == requested_memory_node
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
