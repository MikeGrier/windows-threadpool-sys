// Copyright (c) Mike Grier.

//! What shape is the machine, and which cache level actually partitions it?
//!
//! **An experiment, not a component.** These probes measure platform behaviour
//! and are not for production use. Do not call them from production code, and
//! do not lift a technique out of here. See this crate's DESIGN-NOTES.md.
//!
//! # Why this is a probe rather than a test
//!
//! Almost every number here is host-specific, so there is nothing to assert
//! about its *value* -- only about its internal consistency. That is the
//! binary-plus-asserted split this crate is built around: the binary prints the
//! shape for whoever is reading, and the tests pin the invariants that must hold
//! on any machine, so a parsing regression fails the build even though a core
//! count cannot.
//!
//! Running it in CI is the point. Hosted runners are a heterogeneous fleet, so
//! printing the discovered shape on every build turns ordinary CI into a slow
//! survey of what real machines look like -- including the negative result that
//! cloud runners are consistently single-node, which is itself evidence for how
//! the [uniform tunable architecture](../../../design-sessions/DESIGN-SESSION-2026-08-30-numa-sharded-io-execution-domains.md)
//! should size itself by default.
//!
//! # It measures the shipping crate, deliberately
//!
//! The parse comes from [`windows_topology_sys::MachineMemoryTopology::discover`] rather
//! than from a reimplementation here, for the same reason the pool-growth probe
//! uses the real thread-pool crate: a reimplementation would measure the
//! reimplementation. The raw counters below are then read *independently*
//! through Win32 and compared against it, so this probe doubles as a
//! cross-check on that crate's parsing across every machine CI ever runs on.

use std::io;

use windows_sys::Win32::System::Threading::{
    ALL_PROCESSOR_GROUPS, GetActiveProcessorCount, GetActiveProcessorGroupCount,
    GetNumaHighestNodeNumber,
};

use windows_topology_sys::{
    Coherence, DomainKind, EnumerationAnomaly, MachineMemoryTopology, ProcessorSet, Provenance,
    Source,
};

/// One cache level, summarised across the machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheLevel {
    /// 1, 2, 3, ... as the firmware reports it.
    pub level: u8,
    /// Processors per partition, in discovery order.
    pub processors_per_domain: Vec<usize>,
}

impl CacheLevel {
    /// How many distinct processor *partitions* exist at this level.
    ///
    /// Not the number of caches: a level Windows reports once per cache -- L1
    /// as separate `data` and `instruction` domains over the same processors --
    /// is several relationships but one partition per processor set, and it is
    /// the partition a caller dividing work cares about.
    ///
    /// **Derived, not stored.** This was a `domains: usize` field that
    /// `measure` always filled with `processors_per_domain.len()`, which made
    /// two tests assert one expression against itself -- they read as a check
    /// that the count matches the spans, and could not fail. Deleting them
    /// would have removed the dead assertions; deriving the count removes the
    /// disagreement they were written to catch, so no future test can restate
    /// it either.
    #[must_use]
    pub fn domains(&self) -> usize {
        self.processors_per_domain.len()
    }
}

/// One core, summarised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoreShape {
    /// Whether this core carries more than one logical processor.
    pub simultaneous_multithreading: bool,
    /// The firmware's performance ranking for this core. More than one distinct
    /// value across the machine means heterogeneous cores, and therefore that
    /// an unconstrained thread can be scheduled onto a slow one.
    pub efficiency_class: u8,
    /// Logical processors this core covers.
    pub processors: usize,
}

impl CoreShape {
    /// Whether this record contradicts itself.
    ///
    /// `windows-topology-sys` documents `simultaneous_multithreading` as
    /// "whether this core has more than one logical processor" -- a fact about
    /// the membership recorded beside it, not a capability the hardware might
    /// hold while only one sibling is online. So the two can be checked against
    /// each other, and a record where they disagree describes no machine.
    ///
    /// A method rather than an expression written twice: the asserted live-host
    /// test held the parse to this and [`Observation::cross_check`] did not, so
    /// the probe could print that every check it could make matched on a host
    /// whose own test had just gone red. Both now call this.
    #[must_use]
    pub fn contradicts_itself(&self) -> bool {
        self.simultaneous_multithreading != (self.processors > 1)
    }
}

/// The machine's shape, as the shipping topology crate sees it, plus the raw
/// counters read independently for cross-checking.
#[derive(Debug, Clone)]
pub struct Observation {
    // --- read through windows-topology-sys ---
    /// Logical processors reported as online.
    pub online_processors: usize,
    /// Processor groups. More than one is a hard affinity boundary: a thread's
    /// affinity names exactly one group, so above 64 logical processors the
    /// partition is forced whether or not it is wanted.
    pub groups: usize,
    /// NUMA domains, including any that report no processors.
    pub numa_domains: usize,
    /// NUMA domains that report no processors at all -- ordinary on machines
    /// with CXL expanders or HBM tiers, and the reason a domain count cannot be
    /// used as a thread count.
    ///
    /// Named for what it counts. This was `memoryless_numa_domains` until a
    /// review round caught that the name says the opposite of the measurement:
    /// a *memoryless* node is the established term for one with processors and
    /// no memory, whereas the filter here is `processors.is_empty()` and the
    /// documented examples -- CXL expanders, HBM tiers -- are memory with no
    /// processors. The arithmetic was right and only the label was inverted,
    /// which is the dangerous shape: nothing misbehaved, so only a reader
    /// aggregating the JSON key across a fleet would have been misled, and
    /// about the reverse population.
    pub numa_domains_without_processors: usize,
    /// NUMA domains the relationship walk never described, which only CPU Sets
    /// reported.
    ///
    /// A different question from [`Self::coherence`], which compares PROCESSOR
    /// SETS: two sources can name exactly the same processors and still group
    /// them into nodes differently, and that disagreement lands here as a
    /// domain only one source reported rather than as a coherence failure. No
    /// counter reaches it either -- the node totals can match while the
    /// membership does not -- so if the probe does not carry it, nothing does.
    pub numa_domains_only_in_cpu_sets: usize,
    /// NUMA domains no source reported at all.
    ///
    /// Separate from [`Self::numa_domains_only_in_cpu_sets`] because that
    /// count's message names CPU Sets as the reporter, which would be false
    /// here. Unreachable through [`measure`] and reachable through the public
    /// [`observe`]; it blocks agreement rather than being ignored, because a
    /// domain nobody described still raises [`Self::numa_domains`] while
    /// contributing no node number to compare.
    pub numa_domains_unreported: usize,
    /// NUMA domains carrying more than one distinct node number.
    ///
    /// A contradiction, not an alias: node numbers are machine-wide, so one
    /// domain cannot honestly be both. The crate keeps every label on purpose
    /// (D-15); taking the maximum to compare against the counter would resolve
    /// the disagreement without reporting it.
    ///
    /// Counted over DISTINCT VALUES, and named for that rather than for "the
    /// two sources disagreed" -- which is the usual cause on the path
    /// [`measure`] takes, but is not what the count establishes. Nothing here
    /// groups the labels by which API issued them, so a message naming the two
    /// sources would assert more than was checked.
    pub numa_domains_with_conflicting_labels: usize,
    /// Whether the topology came from enumerating this machine.
    ///
    /// `MachineMemoryTopology` defaults to [`Provenance::Synthetic`] and
    /// deserialization downgrades to it, precisely so a topology nobody
    /// measured cannot pass for one that was. [`observe`] is public and takes
    /// any topology, so without this a hand-built or restored one with matching
    /// counters reached `Agree` -- certifying consistency with a machine no
    /// enumeration ever read.
    pub topology_was_measured: bool,
    /// Cores the relationship walk never described, which only CPU Sets
    /// reported.
    ///
    /// The core-level twin of [`Self::numa_domains_only_in_cpu_sets`]: two
    /// sources that group the same processors into cores differently leave both
    /// groupings in [`Self::cores`], so the count is inflated and `by-core`
    /// oversizes.
    pub cores_only_in_cpu_sets: usize,
    /// Cores that cover no processors, and packages likewise.
    ///
    /// The empty-record twins of [`Self::numa_domains_without_processors`]. A
    /// zero affinity mask raises no anomaly, so a spurious empty record inflates
    /// the count and the policy derived from it while nothing else notices.
    pub cores_without_processors: usize,
    /// Packages that cover no processors. See [`Self::cores_without_processors`].
    pub packages_without_processors: usize,
    /// Relations the relationship walk reported that share a processor with
    /// another walk-reported relation of the same kind.
    ///
    /// A logical processor belongs to exactly one physical package and exactly
    /// one core, so two walk records covering it cannot both describe this
    /// machine. Nothing else here reaches that: no raw counter measures
    /// packages or cores, an overlapping record raises no enumeration anomaly,
    /// and both records are non-empty, so the `*_without_processors` counts stay
    /// clear while [`Self::packages`] and `by-core` silently hold the duplicate.
    ///
    /// Restricted to the WALK. Two *sources* grouping the same processors
    /// differently is a state `windows-topology-sys` keeps deliberately, and
    /// [`Self::cores_only_in_cpu_sets`] is the count that reports it; including
    /// it here would report one disagreement twice.
    pub overlapping_walk_relations: usize,
    /// Relations a caller described rather than any platform API reporting them.
    ///
    /// Distinct from [`Self::topology_was_measured`], which is object-level:
    /// `Provenance::Measured` permits hand-inserted relations, and
    /// `Source::Description` is the crate's marker for exactly that mixed case.
    /// So a measured topology can still carry counted relations nobody read
    /// from the platform.
    pub described_relations: usize,
    /// Relations no source reported at all, of any kind.
    ///
    /// Counted for every kind, not just memory domains: the crate documents an
    /// empty observation list as the honest state for a relation built by hand,
    /// and such a core, package or cache is still counted here and can still
    /// change a policy or the cache partitioning.
    pub unreported_relations: usize,
    /// Per-processor attributes carrying more than one distinct value.
    ///
    /// The crate preserves these separately from the relations
    /// (`attribute_conflicts`) because they have no membership to compare --
    /// efficiency class is the case, reported per core by the walk and per
    /// processor by CPU Sets. [`Self::cores`] carries the walk's value alone,
    /// so without this the report could call a machine homogeneous while the
    /// other source disagreed.
    ///
    /// Named for distinct VALUES, not for "the two sources disagreed":
    /// `attribute_conflicts` groups claims by subject and keeps those with more
    /// than one distinct value, without grouping by the source that issued
    /// them. The two-source case is the usual cause and is not what the count
    /// establishes.
    pub processor_attribute_conflicts: usize,
    /// The largest NUMA node number the topology crate reported, or `None` when
    /// no memory domain carried a label from either source.
    ///
    /// Not "when it reported no memory domain at all", which is what this said
    /// and is a stronger claim than the code makes: a memory domain with an
    /// empty `observations` list -- which `windows-topology-sys` documents as
    /// the honest state for "a relation nobody reported" -- raises
    /// [`Self::numa_domains`] while contributing no label here. Unreachable
    /// through [`measure`], since every domain `discover` builds carries at
    /// least one observation, but [`observe`] is public and takes any topology.
    ///
    /// Kept beside the count because the two answer different questions and
    /// Windows only promises the second one: node numbers are not guaranteed
    /// dense, so a machine with nodes 0 and 2 has a count of two and a highest
    /// of two. Comparing the count against `GetNumaHighestNodeNumber` would
    /// call that correct machine a parsing regression.
    pub highest_numa_node: Option<u32>,
    /// Physical packages (sockets).
    pub packages: usize,
    /// Every physical core.
    pub cores: Vec<CoreShape>,
    /// Cache levels, ascending, each summarised across the machine.
    pub caches: Vec<CacheLevel>,
    /// Which of [`Self::caches`] the topology crate named as the outermost
    /// level that splits the machine into more than one domain, if any.
    ///
    /// Captured from `MachineMemoryTopology::outermost_partitioning_cache`
    /// rather than derived from the summaries above, so the rule has one
    /// implementation (`SH-16.9`). `None` is a real answer and not a failure --
    /// but it is "no level was NAMED", which is not the same as "no level
    /// divides this machine", and this said the latter. The crate also answers
    /// `None` when two levels partition the machine incomparably, and when its
    /// candidate filter left nothing at all. [`Self::partitioning_cache`] is
    /// what tells those apart; read it rather than reading a cause into this.
    pub partitioning_cache_level: Option<u8>,
    /// What the topology crate could not fully decode of what Windows returned.
    ///
    /// **Not all of them are dropped records**, which is the trap: an
    /// `Undersized` or `OverrunsBuffer` record decodes to nothing and leaves
    /// the counts above short, but a `TruncatedArray` record is *kept* --
    /// `windows-topology-sys` says so directly, "the entries that did fit are
    /// decoded and kept; this records that the count claimed more". A cache
    /// record kept with a partial affinity mask presents a processor set that
    /// is smaller than the truth and therefore DISTINCT from it, which
    /// `cache_partitions_at_level` counts as its own partition. So an anomaly
    /// can leave a count short or overstated, and this observation must not
    /// claim to know which.
    ///
    /// Carried because `discover` returns `Ok` when it hits one: the record is
    /// recorded here and the parse continues, so nothing else in this
    /// observation is sensitive to it. `windows-topology-sys` states the
    /// consequence directly -- dropping the list "would leave a consumer
    /// unable to tell a truncated enumeration from a small machine" -- and a
    /// probe whose whole job is to say what the run established is exactly
    /// such a consumer.
    pub enumeration_anomalies: Vec<EnumerationAnomaly>,
    /// Whether the topology crate's two Win32 sources described the same
    /// machine.
    ///
    /// Also `Ok` when they did not: [`Coherence::Disagreed`] is the crate's
    /// *conclusion* that the disagreement is real rather than transient, and
    /// it names processors that CPU Sets saw and the relationship walk did not
    /// -- processors deliberately absent from the parsed list, so no count
    /// derived from that list can reveal them.
    pub coherence: Coherence,

    // --- read independently through Win32 ---
    /// `GetActiveProcessorCount(ALL_PROCESSOR_GROUPS)`.
    pub raw_active_processors: u32,
    /// `GetActiveProcessorGroupCount()`.
    pub raw_group_count: u16,
    /// `GetNumaHighestNodeNumber()`, or `None` if the call failed.
    pub raw_highest_numa_node: Option<u32>,
    /// Whether the counters moved across the parse, so the two readings
    /// describe different instants and cannot be compared.
    ///
    /// Bears on the COMPARISON only. The topology is still a valid snapshot of
    /// the machine as it was, so the parse-side findings stand on their own and
    /// `cross_check` evaluates them regardless.
    ///
    /// [`measure`] computes this from its two counter readings; [`observe`]
    /// takes it as an argument and stores what the caller passed, because only
    /// the caller knows what produced its counters. This said `observe` "leaves
    /// it `false`", which was true while the field was a `bool` and survived
    /// the change to [`BracketOutcome`] -- naming a value the type no longer
    /// has, for a parameter the function now takes.
    pub bracket: BracketOutcome,
}

impl Observation {
    /// The outermost cache level that actually splits the machine into more
    /// than one domain, if any.
    ///
    /// **Asked of `windows-topology-sys`, not re-derived here.** This method
    /// used to restate the rule as "the highest level with more than one
    /// domain", and by the time `M4+.4` landed that restatement differed from
    /// the crate's own answer in two ways: it omitted the pairwise-disjointness
    /// check, so a hand-built topology with overlapping blocks would have been
    /// accepted, and it ordered candidates by **level number**, which the
    /// topology crate stopped doing because a higher number is not always
    /// coarser -- the ARM64 machine with no L3 is the standing counterexample.
    ///
    /// So the level is now captured at survey time from
    /// `MachineMemoryTopology::outermost_partitioning_cache`, and this method
    /// only looks up the summary for it. `SH-16.9` records this rule going
    /// wrong three times in two crates; there is now one implementation.
    #[must_use]
    pub fn outermost_partitioning_cache(&self) -> Option<&CacheLevel> {
        let level = self.partitioning_cache_level?;
        self.caches.iter().find(|c| c.level == level)
    }

    /// The same answer as [`Self::outermost_partitioning_cache`], with the
    /// `None` cases told apart.
    ///
    /// `None` is several different findings wearing one face, and which one it is
    /// changes what a reader may conclude about the machine. The prose report
    /// has always refused to conflate them -- "naming only the first turns a
    /// reported ambiguity into a false claim about the hardware" -- but the
    /// NDJSON emitted `"outermost_partitioning_cache_level":null` for all of
    /// them, on a line the verdict had already certified as `agree`, because an
    /// incomparable partitioning touches nothing `cross_check` consults. A
    /// fleet query counting nulls as "machines no cache level partitions" --
    /// the natural reading, and the one this crate's own no-L3 story invites --
    /// silently folded in the others.
    ///
    /// What separates them is data the probe already holds, and reading it is
    /// not the re-derivation `SH-16.9` forbids: the crate is still the only
    /// thing that decides WHICH level is outermost. This only asks whether any
    /// level partitions at all, to classify an answer the crate already gave.
    #[must_use]
    pub fn partitioning_cache(&self) -> PartitioningCache<'_> {
        let Some(level) = self.partitioning_cache_level else {
            // Deliberately not "incomparable". The crate reaches `None` both
            // when two maximal candidates are not the same partition and when
            // its candidate filter left nothing at all -- a partitioning level
            // rejected for overlapping blocks lands in the second. This probe
            // cannot tell those apart and does not guess.
            // Checked BEFORE the others, because both of them read as facts
            // about the hardware and an empty list supports neither. `any()` on
            // it is vacuously false, so it fell into "no level partitions this
            // machine" -- a claim about cache structure drawn from a survey that
            // reported no cache structure at all.
            return if self.caches.is_empty() {
                PartitioningCache::NoLevelsReported
            } else if self.caches.iter().any(|c| c.domains() > 1) {
                PartitioningCache::NoUniqueOutermost
            } else {
                PartitioningCache::NoLevelPartitions
            };
        };
        self.caches.iter().find(|c| c.level == level).map_or(
            PartitioningCache::SummaryMissing(level),
            PartitioningCache::Level,
        )
    }

    /// How many execution domains each candidate policy would produce.
    ///
    /// Reported rather than recommended. The point of printing all of them is
    /// that they disagree, and the disagreement is the finding.
    #[must_use]
    pub fn domain_counts(&self) -> Vec<(&'static str, usize)> {
        vec![
            ("single", 1),
            // Every count below is clamped to one, and the clamp is the
            // contract rather than defensiveness: there is always at least one
            // execution domain, because the machine exists, and a fleet sized
            // at zero domains performs no I/O at all.
            //
            // Zero is reachable for each of these, which is why none of them is
            // passed through raw. A `PROCESSOR_RELATIONSHIP` record whose body
            // is shorter than its declared size decodes to nothing and is
            // dropped as an enumeration anomaly, while `discover` still returns
            // `Ok` -- so a topology with no package or core relationships is a
            // legal, non-error result rather than a parse failure.
            //
            // "Every count below" is meant literally, and was not: an earlier
            // version clamped `packages` and `cores` while leaving the cache
            // arm's `c.domains` raw, because the `1` there is only the `None`
            // default and reads like a clamp without being one. `Observation`'s
            // fields are public and `domain_counts` is `pub`, so a cache summary
            // reporting zero domains is constructible even though `measure`
            // cannot produce one -- and this comment asserted otherwise.
            ("by-package", self.packages.max(1)),
            (
                "by-numa-domain-with-processors",
                // Saturating as well as clamped. Not because the two counts can
                // diverge in a measured run -- they come from one loop over one
                // enumeration, so the second is a subset of the first -- but
                // because `Observation`'s fields and `domain_counts` are both
                // public, so a caller can hand this an out-of-range pair, and
                // an unsigned subtraction would panic rather than report.
                self.numa_domains
                    .saturating_sub(self.numa_domains_without_processors)
                    .max(1),
            ),
            (
                "by-outermost-partitioning-cache",
                // Matched per variant rather than read through
                // `outermost_partitioning_cache`, whose `None` folds four
                // distinct answers into one. Every absent case does size to a
                // single domain, but they are spelled out so a variant added
                // later is a compile error here instead of silently joining
                // the fallback -- which is the whole reason `PartitioningCache`
                // exists rather than an `Option`.
                //
                // `.max(1)` is on the SELECTED value, not just the default:
                // the default covers "no level was chosen", and this covers "a
                // level was chosen whose summary reports no domains".
                match self.partitioning_cache() {
                    PartitioningCache::Level(cache) => cache.domains().max(1),
                    PartitioningCache::NoLevelsReported
                    | PartitioningCache::NoLevelPartitions
                    | PartitioningCache::NoUniqueOutermost
                    | PartitioningCache::SummaryMissing(_) => 1,
                },
            ),
            ("by-core", self.cores.len().max(1)),
        ]
    }

    /// Whether these counts include relations no platform API reported.
    ///
    /// A single named predicate that argues its own membership, rather than a
    /// condition restated where it is used. Each member is a way for a count
    /// here to have come from somewhere other than a parse of this machine:
    ///
    /// - not measured at all, so no enumeration produced any of it;
    /// - a relation a caller *described*, which `Provenance::Measured` permits
    ///   on an otherwise measured topology and `Source::Description` marks;
    /// - a relation carrying no observation from any source, which the owning
    ///   crate documents as the honest state for one built by hand.
    ///
    /// The three are one idea -- the counts are not wholly a parse -- which is
    /// why they are named once here rather than re-tested at the comparison.
    /// Being conservative costs nothing on the path this probe actually takes:
    /// `discover` gives every relation at least one platform source, so
    /// [`measure`] never reaches this. Only the public [`observe`] can.
    #[must_use]
    pub fn counts_include_unparsed_relations(&self) -> bool {
        !self.topology_was_measured || self.described_relations > 0 || self.unreported_relations > 0
    }

    /// Whether the independently-read Win32 counters agree with what the
    /// topology crate parsed.
    ///
    /// A disagreement is a real finding: it means the shipping crate's parse of
    /// `GetLogicalProcessorInformationEx` diverges from what the simple
    /// counters report on this machine.
    ///
    /// # Why a counter that could not be read is not a disagreement
    ///
    /// The two outcomes mean different things and belong to different owners. A
    /// disagreement is a statement about the shipping crate; a counter that
    /// could not be read is a statement about this measurement, and reporting
    /// the second as the first would send a reader to audit a parse that was
    /// never contradicted.
    ///
    /// Collapsing them also produced the failure this whole probe exists to
    /// avoid. An earlier version skipped the NUMA comparison when
    /// `GetNumaHighestNodeNumber` failed, contributed no entry, and so let the
    /// renderer print its agreement line directly below
    /// "GetNumaHighestNodeNumber : failed" -- claiming an agreement the run had
    /// not established, in the report whose entire job is to separate what was
    /// measured from what was assumed.
    ///
    /// # Why the crate's own report is consulted before the counters
    ///
    /// The three counters are scalars, and none of them is sensitive to a
    /// dropped `PROCESSOR_RELATIONSHIP` record or to a processor that only CPU
    /// Sets saw. So a parse that `windows-topology-sys` itself reported as
    /// incomplete could satisfy all three and reach `Agree` -- the same shape
    /// as the NUMA defect above, and worse, because the evidence was already
    /// in hand rather than needing a fourth call to go and get.
    ///
    /// The rule is therefore fiat and simple: **`Agree` requires all three
    /// lists empty**, so anything this method pushes blocks it, whether or not
    /// a counter noticed. Stated over the LISTS and not over their causes,
    /// deliberately -- the causes are enumerated in exactly one place, the body
    /// below, so adding a fourth needs no prose anywhere to be corrected. An
    /// earlier wording named the two causes that existed at the time ("an empty
    /// anomaly list and `Coherence::Agreed`"), a third was added without the
    /// sweep, and the stale pair was then transcribed into DESIGN-NOTES.md as
    /// settled fiat -- inviting a future reader to delete the undocumented
    /// branch to make the code match its own rule.
    ///
    /// **The causes are not listed here, and deliberately are not.** The
    /// paragraph above once ended with "as of now the body pushes for ..."
    /// naming the three that existed; a fourth was added a commit later without
    /// the sweep, so the very doc diagnosing that rot had rotted the same way
    /// inside two rounds. A list of causes kept beside the rule is not a
    /// summary of the body, it is a second copy of it that nothing checks. Read
    /// the body.
    ///
    /// The match on coherence is exhaustive so that a variant added later is a
    /// compile error here rather than a silent new path to agreement.
    #[must_use]
    pub fn cross_check(&self) -> CrossCheck {
        let mut check = CrossCheck::default();

        if !self.enumeration_anomalies.is_empty() {
            check.parse_incomplete.push(format!(
                // States the CONDITION, not a direction. "Records were dropped
                // and the counts are short" was true of the two anomaly kinds
                // that decode to nothing and false of `TruncatedArray`, which
                // keeps the record with the entries that fit -- and a cache
                // record kept with a partial affinity mask can INFLATE a
                // partition count rather than shorten it, so the old message
                // pointed a reader the wrong way. `AnomalyKind` is
                // `#[non_exhaustive]`, so classifying here would need a
                // catch-all arm that a future variant falls into silently;
                // saying only what is true of all of them cannot rot that way.
                "windows-topology-sys recorded {} enumeration anomal{}, so what Windows returned \
                 was not fully decoded and the counts above may be short, or overstated where a \
                 record was kept with an incomplete processor set",
                self.enumeration_anomalies.len(),
                if self.enumeration_anomalies.len() == 1 {
                    "y"
                } else {
                    "ies"
                },
            ));
        }

        if self.numa_domains_only_in_cpu_sets > 0 {
            check.parse_incomplete.push(format!(
                "{} NUMA domain(s) were reported only by CPU Sets and never by the relationship \
                 walk, so the two sources group nodes differently -- which no counter and no \
                 coherence check reaches, since coherence compares processor sets",
                self.numa_domains_only_in_cpu_sets,
            ));
        }

        // No cache levels at all. Distinct from the per-level case below, and
        // NOT reachable the same way: an empty affinity mask still leaves the
        // level present, because `cache_levels` filters on kind. This is the
        // survey reporting no cache relationships whatsoever, so there is no
        // record that failed to decode, nothing raises an anomaly, and every
        // conclusion the report draws about cache structure would be drawn from
        // nothing.
        if self.caches.is_empty() {
            check.parse_incomplete.push(
                "no cache levels were reported at all, so what divides this machine by cache \
                 was not established in either direction"
                    .to_string(),
            );
        }

        // A level the survey DOES carry, with no partitions at all.
        // `cache_levels` filters on kind while `cache_partitions_at_level`
        // drops domains covering no processors, so a cache record whose
        // affinity mask is empty yields exactly this -- and it raises no
        // anomaly, because `read_cache_body` reports only a declared-versus-read
        // count mismatch and never inspects mask contents. So nothing else here
        // is sensitive to it, and without this the report printed
        // "L3 0 domain(s)" a few lines above "no cache level partitions this
        // machine", with the verdict certifying the pair as `agree`.
        let empty_levels: Vec<u8> = self
            .caches
            .iter()
            .filter(|c| c.domains() == 0)
            .map(|c| c.level)
            .collect();
        if !empty_levels.is_empty() {
            check.parse_incomplete.push(format!(
                "cache level(s) {empty_levels:?} decoded to no partitions at all, so what \
                 divides this machine at those levels was not established"
            ));
        }

        // A running machine has processors and groups whatever the enumeration
        // said, so a MEASURED topology reporting none of either did not describe
        // its host. Nothing below reaches this: both raw counters report failure
        // as zero, so a zero parse beside a failed read is filed as
        // `not_compared` -- leaving `parse_in_doubt` false, and the report free
        // to state an impossible machine without a caveat.
        //
        // Stated over the LIST rather than once per count, so a third such count
        // joins the array instead of needing its own rule to be remembered.
        let absent: Vec<&str> = [
            ("online processors", self.online_processors),
            ("processor groups", self.groups),
        ]
        .into_iter()
        .filter(|(_, count)| *count == 0)
        .map(|(name, _)| name)
        .collect();
        if self.topology_was_measured && !absent.is_empty() {
            check.parse_incomplete.push(format!(
                "this topology was measured from a running machine, which cannot have none, but \
                 it reported no {}",
                absent.join(" and "),
            ));
        }

        // The machine has packages and cores whatever the enumeration said, so
        // reporting none of either means the enumeration did not describe them.
        // Treated exactly as an empty cache survey is, and for the same reason:
        // no record needs to have FAILED for this to happen -- Windows can
        // simply not report the relationship, in which case no anomaly fires
        // and nothing else here notices.
        if self.online_processors > 0 && self.packages == 0 {
            check
                .parse_incomplete
                .push("no packages were reported at all, though the machine has one".to_string());
        }
        if self.online_processors > 0 && self.cores.is_empty() {
            check
                .parse_incomplete
                .push("no cores were reported at all, though the machine has one".to_string());
        }

        // A record that contradicts ITSELF, which no counter reaches: nothing
        // independent measures cores or caches, and neither shape raises an
        // enumeration anomaly. Both were already invariants the asserted
        // live-host tests hold the parse to, and `cross_check` did not -- so on
        // a host where one of those tests went red, this said every check it
        // could make had matched. CI now prints the report even when the tests
        // fail, which is exactly when that sentence gets read.
        let contradictory_cores = self
            .cores
            .iter()
            .filter(|core| core.contradicts_itself())
            .count();
        if contradictory_cores > 0 {
            check.parse_incomplete.push(format!(
                "{contradictory_cores} core(s) report an SMT flag that disagrees with the number \
                 of processors recorded beside it, so the record contradicts itself"
            ));
        }

        // Windows numbers cache levels from 1, so a level of 0 is a level the
        // parse did not read rather than one the machine has.
        let unnumbered_levels = self.caches.iter().filter(|c| c.level == 0).count();
        if unnumbered_levels > 0 {
            check.parse_incomplete.push(format!(
                "{unnumbered_levels} cache level(s) are numbered 0, which is not a level Windows \
                 reports, so what they describe was not established"
            ));
        }

        // The renderer prints this state as "BUG IN THIS PROBE ... Nothing
        // below about cache partitioning can be trusted", and nothing here
        // said anything about it -- so the verdict could certify the same run
        // as `agree`, two paragraphs apart on one page. `domain_counts` sized
        // it to a single domain besides.
        //
        // Unreachable through `observe`, since the level is captured from the
        // same survey the summaries are built from. `Observation`'s fields and
        // `partitioning_cache` are both public, which is the same reason the
        // clamps in `domain_counts` exist.
        if let PartitioningCache::SummaryMissing(level) = self.partitioning_cache() {
            check.parse_incomplete.push(format!(
                "L{level} was named as the outermost partitioning cache and this survey carries \
                 no summary for it, so what it divides was not established"
            ));
        }

        if !self.topology_was_measured {
            check.parse_incomplete.push(
                "this topology was not measured from a running machine, so nothing here \
                 describes the host it is reported on"
                    .to_string(),
            );
        }

        if self.cores_without_processors > 0 || self.packages_without_processors > 0 {
            check.parse_incomplete.push(format!(
                "{} core(s) and {} package(s) cover no processors, so they raise those counts \
                 and the policies derived from them without describing any part of the machine",
                self.cores_without_processors, self.packages_without_processors,
            ));
        }

        if self.unreported_relations > 0 {
            check.parse_incomplete.push(format!(
                "{} relation(s) carry no observation from any source, so they are counted here \
                 without any platform API having described them",
                self.unreported_relations,
            ));
        }

        if self.described_relations > 0 {
            check.parse_incomplete.push(format!(
                "{} relation(s) were described by a caller rather than reported by any platform \
                 API, so the counts above are not all of them measured",
                self.described_relations,
            ));
        }

        if self.cores_only_in_cpu_sets > 0 {
            check.parse_incomplete.push(format!(
                "{} core(s) were reported only by CPU Sets and never by the relationship walk, so \
                 the two group processors into cores differently and the core count above holds \
                 both groupings",
                self.cores_only_in_cpu_sets,
            ));
        }

        if self.overlapping_walk_relations > 0 {
            check.parse_incomplete.push(format!(
                "{} relation(s) reported by the relationship walk share a processor with another \
                 of the same kind, so one processor is claimed by two packages, two cores or two \
                 NUMA nodes and the counts above hold both",
                self.overlapping_walk_relations,
            ));
        }
        if self.processor_attribute_conflicts > 0 {
            check.parse_incomplete.push(format!(
                "{} per-processor attribute(s) carry more than one distinct value, so the \
                 efficiency classes above are one claim rather than an agreed one",
                self.processor_attribute_conflicts,
            ));
        }

        if self.numa_domains_with_conflicting_labels > 0 {
            check.parse_incomplete.push(format!(
                "{} NUMA domain(s) carry more than one distinct node number, so what node they \
                 are was not established and the highest below takes the larger",
                self.numa_domains_with_conflicting_labels,
            ));
        }

        if self.numa_domains_unreported > 0 {
            check.parse_incomplete.push(format!(
                "{} NUMA domain(s) carry no observation from either source, so they raise the \
                 domain count while contributing no node number to compare",
                self.numa_domains_unreported,
            ));
        }

        match &self.coherence {
            Coherence::Agreed => {}
            Coherence::Disagreed {
                walk_only,
                cpu_sets_only,
                attempts,
            } => check.parse_incomplete.push(format!(
                "windows-topology-sys reports its two enumerations never agreed within {attempts} \
                 attempt(s): {} processor(s) seen only by the relationship walk, {} seen only by \
                 CPU Sets and so absent from the parsed list entirely",
                walk_only.len(),
                cpu_sets_only.len(),
            )),
            // Unreachable from `discover`, which returns `Agreed` or
            // `Disagreed`. Reported rather than ignored because reaching it
            // would mean the parse came from somewhere that read nothing
            // twice, and a cross-check cannot certify that either.
            Coherence::NotCollected => check.parse_incomplete.push(
                "windows-topology-sys reports its coherence was never collected, so nothing \
                 established that its two enumerations describe the same machine"
                    .to_string(),
            ),
        }

        // Everything above is about the PARSE and is evaluated unconditionally.
        // Only the counter comparisons below are skipped when the machine moved
        // under the run, because only they compare two readings.
        //
        // This was an early return, which suppressed every check above it: a
        // host that hot-added a processor AND dropped a record reported
        // `parse_incomplete: 0`, so `parse_in_doubt` was false, the renderer
        // printed "this machine reports no L3 at all" as an uncaveated hardware
        // fact, and the dropped record appeared nowhere in the report at all --
        // the exact failure this probe exists to prevent, introduced by the fix
        // for the timing skew.
        // Matched exhaustively, so a fourth outcome cannot silently become a
        // reason to compare.
        match self.bracket {
            BracketOutcome::HeldStill => {}
            BracketOutcome::Changed => {
                check.not_compared.push(
                    "the machine changed while this ran -- the counters moved across the parse, \
                     so the two readings describe different instants"
                        .to_string(),
                );
                return check;
            }
            BracketOutcome::NotEstablished => {
                check.not_compared.push(
                    "the bracket around the parse was not closed -- a counter failed one of its \
                     two reads, so nothing established that the machine held still"
                        .to_string(),
                );
                return check;
            }
        }

        // A mismatch can only be filed against the PARSE while the counts ARE
        // the parse. `observe` is public and takes any topology, so a caller's
        // described relation -- or one nobody reported -- raises `groups`, or a
        // NUMA label, that no platform API ever produced; the comparisons below
        // then read that difference as the shipping crate contradicting Windows.
        // Same inversion the NUMA labels once carried, and `verdict` gives
        // `Disagree` precedence, so the report leads with an accusation this run
        // did not establish while the reason sits further down the same page.
        //
        // Filed as `not_compared` rather than skipped silently: this is a
        // reading that could not be trusted, which is exactly what that list is.
        if self.counts_include_unparsed_relations() {
            check.not_compared.push(
                "these counts include relations no platform API reported, so a counter mismatch \
                 could not be attributed to the parse"
                    .to_string(),
            );
            return check;
        }

        // Zero is how these two report failure, and no machine has zero active
        // processors or zero groups, so zero is a failed read rather than a
        // count to compare against. Treating it as a count reported a failed
        // measurement as though the crate's parse were wrong.
        if self.raw_active_processors == 0 {
            check.not_compared.push(
                "GetActiveProcessorCount returned 0, which is its failure report".to_string(),
            );
        } else if self.online_processors != self.raw_active_processors as usize {
            check.disagreements.push(format!(
                "online processors: topology crate says {}, GetActiveProcessorCount says {}",
                self.online_processors, self.raw_active_processors
            ));
        }

        if self.raw_group_count == 0 {
            check.not_compared.push(
                "GetActiveProcessorGroupCount returned 0, which is its failure report".to_string(),
            );
        } else if self.groups != self.raw_group_count as usize {
            check.disagreements.push(format!(
                "groups: topology crate says {}, GetActiveProcessorGroupCount says {}",
                self.groups, self.raw_group_count
            ));
        }

        let Some(highest) = self.raw_highest_numa_node else {
            check.not_compared.push(
                "GetNumaHighestNodeNumber failed, so no NUMA comparison was made".to_string(),
            );
            return check;
        };

        if self.highest_numa_node != Some(highest) {
            // Highest against highest, deliberately, and not a count against
            // `highest + 1`. `GetNumaHighestNodeNumber` reports the largest node
            // *number*, which Windows does not promise equals the node count --
            // nodes 0 and 2 are a valid sparse topology, and the count form
            // would report a regression on hardware that is reporting itself
            // correctly.
            check.disagreements.push(format!(
                "NUMA nodes: topology crate's highest node is {}, GetNumaHighestNodeNumber says {}",
                self.highest_numa_node
                    .map_or_else(|| "none".to_string(), |n| n.to_string()),
                highest
            ));
        }
        check
    }
}

/// What this run established about the topology crate's parse.
///
/// Three lists rather than one, because they are claims about three different
/// things and each has a different owner -- see [`Observation::cross_check`].
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CrossCheck {
    /// Counters that were compared and did not match. Each is a finding about
    /// the shipping crate's parse.
    pub disagreements: Vec<String>,
    /// Comparisons this run could not make, or could not trust. Each is a gap
    /// in this measurement, not a finding about the parse.
    ///
    /// Stated over what the list MEANS rather than over what fills it. This
    /// read "counters that could not be read", which named one cause and was
    /// already false of two others -- a machine that changed under the run
    /// (a bracket that closed on two different instants), and counts holding
    /// relations no platform API reported. Both are comparisons that were not
    /// made; neither is a counter that failed. The causes are enumerated in
    /// exactly one place, [`Observation::cross_check`]'s body.
    pub not_compared: Vec<String>,
    /// Ways the parse is short, or its claims mutually inconsistent, such that
    /// agreeing counters cannot certify it.
    ///
    /// Neither of the other two: nothing this probe read was contradicted, and
    /// nothing it wanted to read was missing. Established from the PARSE rather
    /// than from any counter, which is why no counter agreeing can retire an
    /// entry here.
    ///
    /// Not "things the crate reported about itself", which is how this read
    /// while every entry happened to be a crate self-assessment. One is not:
    /// a NUMA domain only CPU Sets described is derived here, from provenance
    /// the crate carries but draws no conclusion about.
    pub parse_incomplete: Vec<String>,
}

impl CrossCheck {
    /// What this run is entitled to claim.
    ///
    /// `Agree` requires all three lists empty, so it means "everything this
    /// probe could check was checked and matched, and nothing about the parse
    /// itself left it unable to certify" -- which is what the rendered line
    /// claims. Only [`Self::disagreements`] produces `Disagree`, because that
    /// is the only list whose entries contradict the parse; an incomplete parse
    /// is not a wrong one, and reporting it as a divergence would send a reader
    /// to audit a mismatch that does not exist.
    #[must_use]
    pub fn verdict(&self) -> Verdict {
        if !self.disagreements.is_empty() {
            Verdict::Disagree
        } else if self.not_compared.is_empty() && self.parse_incomplete.is_empty() {
            Verdict::Agree
        } else {
            Verdict::Incomplete
        }
    }

    /// Whether anything here bears on the parsed topology describing this
    /// machine -- and so on whether the counts derived from it may be read as
    /// hardware facts.
    ///
    /// Two of the three lists, deliberately, and this is the one place that
    /// says which. [`Self::disagreements`] belongs because a counter read
    /// independently from Windows contradicting the walk is the *strongest*
    /// available evidence that the parsed list is short: a crate reporting one
    /// processor group where `GetActiveProcessorGroupCount` says two did not
    /// merely miscount, it never saw the second group's records, including its
    /// caches. [`Self::parse_incomplete`] belongs by construction.
    ///
    /// [`Self::not_compared`] is excluded, and that exclusion is a claim rather
    /// than an oversight. Every entry there is a reading this probe could not
    /// make or could not trust -- which bears on its ability to CHECK the parse
    /// and on nothing in the parse itself. Caveating the cache conclusions
    /// there would assert a doubt this run does not have, which is the same
    /// defect as asserting a certainty it does not have.
    ///
    /// Stated over what the list means, not over what fills it. This said "its
    /// only entries are the three Win32 counters failing to read", which two
    /// later rounds falsified by adding the bracket outcomes -- and one of
    /// those, a machine that CHANGED, is the opposite of a counter that failed
    /// to read: every counter was read, twice, and they differed.
    #[must_use]
    pub fn parse_in_doubt(&self) -> bool {
        !self.disagreements.is_empty() || !self.parse_incomplete.is_empty()
    }
}

/// Which answer [`Observation::partitioning_cache`] gave, with the ways there
/// can be no level named held apart from each other.
///
/// Counted rather than enumerated, once, as "the three ways" -- and a fourth
/// arrived a round later. A count is a restatement of the variant list below
/// that nothing checks, so there is none.
///
/// An enum rather than an `Option` for the reason [`Verdict`] is one rather
/// than a `bool`: a renderer or a serialiser cannot emit the absent case
/// without having decided which absent case it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitioningCache<'a> {
    /// The crate named this level, and its summary is here.
    Level(&'a CacheLevel),
    /// The survey reported no cache levels at all.
    ///
    /// Not a fact about cache structure in either direction -- nothing was
    /// measured. Its own variant because `any()` over an empty list is
    /// vacuously false, so this used to reach [`Self::NoLevelPartitions`] and
    /// print "no cache level reported more than one domain, so no cache
    /// boundary divides the work": a conclusion about the hardware drawn from a
    /// survey that found no hardware to draw it from.
    NoLevelsReported,
    /// At least one level was reported, none reports more than one domain, and
    /// no level was named.
    ///
    /// Says what was COUNTED, and nothing about coverage. This read "every
    /// level reported covers the whole machine, so no cache boundary divides
    /// the work" -- a claim the probe holds no data for: a level whose one
    /// domain covers half the online processors lands here too, and nothing
    /// checks that a level's domains cover the machine. The renderer's sentence
    /// was narrowed for exactly this reason and this copy was not swept with
    /// it.
    ///
    /// A level whose records decoded to no partitions at all is also not more
    /// than one, and `cross_check` pushes a `parse_incomplete` entry for that,
    /// so the verdict moves off `Agree` and the renderer caveats it. Read the
    /// cross-check before taking this for anything.
    NoLevelPartitions,
    /// No level was named, yet at least one level reports more than one
    /// distinct domain.
    ///
    /// Named as "not the two cases above" and no further, because
    /// `CacheLevel::domains` counts DISTINCT processor sets and the topology
    /// crate is explicit that "distinct is not disjoint ... the result is a set
    /// of domains rather than a proven partition". So this covers both two
    /// levels partitioning the machine incomparably AND a level whose blocks
    /// overlap, which `outermost_partitioning_cache` rejects and which
    /// partitions nothing at all. Saying "at least one level does split the
    /// machine" -- as this once did -- asserts the first and is contradicted by
    /// the second.
    NoUniqueOutermost,
    /// The crate named this level and the survey has no summary for it.
    ///
    /// A defect in this probe rather than anything about the machine, and
    /// `the_outermost_partitioning_cache_is_the_deepest_level_that_splits_the_machine`
    /// asserts it cannot happen. It is a variant anyway so that the renderer
    /// must handle it rather than folding it into an answer about the hardware,
    /// which is how it would otherwise be printed as "no level partitions".
    SummaryMissing(u8),
}

/// The three things a cross-check can conclude.
///
/// An enum rather than a `bool` so that a renderer cannot print "agree" without
/// having handled the third case: the compiler makes the incomplete arm
/// impossible to omit, which is what a stray `is_empty()` on one of the two
/// lists would have allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Every counter was read, every one matched, and nothing about the parse
    /// itself blocked certifying it. All three of [`CrossCheck`]'s lists empty.
    Agree,
    /// At least one counter was read and did not match.
    Disagree,
    /// Nothing disagreed, but something was missing or unestablished -- a
    /// counter that could not be read, or the parse being short or disputed --
    /// so this run did not establish agreement.
    ///
    /// Stated over the lists rather than over their causes: this is every
    /// entry in [`CrossCheck::not_compared`] and
    /// [`CrossCheck::parse_incomplete`], whatever put it there. Naming the
    /// causes here is what let this doc go stale when a third was added.
    Incomplete,
}

/// Discover the machine's shape.
///
/// # Errors
///
/// Propagates a failure from [`MachineMemoryTopology::discover`].
pub fn measure() -> io::Result<Observation> {
    // Bracketed, because the parse and the counters are separate reads of a
    // machine that can change between them: Windows supports processor hot-add,
    // and a machine that gained one mid-run would have both readings correct
    // for different instants and be reported as the crate parsing wrongly.
    // Re-reading afterwards detects it, since the same hot-add moves the
    // counters.
    let before = read_counters();
    let topology = MachineMemoryTopology::discover()?;
    let after = read_counters();

    let observation = observe(
        &topology,
        after.0,
        after.1,
        after.2,
        bracket_outcome(before, after),
    );
    Ok(observation)
}

/// Whether two bracketing counter reads establish that the machine changed.
///
/// Extracted rather than inlined in [`measure`], for the reason [`observe`] is:
/// a stable host produces `before == after` with every read succeeding, so no
/// test driving `measure` can reach the interesting branches. Inlined, the test
/// for this predicate reproduced it verbatim and asserted against its own copy
/// -- a mutation sweep killed nothing across all six operators here.
///
/// Only counters read successfully in BOTH brackets are evidence. Comparing the
/// failure sentinels as values made a counter that failed once and succeeded
/// once -- 0 then 4 -- read as "the machine changed while this ran", a claim
/// about the hardware nothing established. A read that failed is reported as a
/// failed read instead.
#[must_use]
pub fn bracket_outcome(
    before: (u32, u16, Option<u32>),
    after: (u32, u16, Option<u32>),
) -> BracketOutcome {
    let changed = (before.0 != 0 && after.0 != 0 && before.0 != after.0)
        || (before.1 != 0 && after.1 != 0 && before.1 != after.1)
        || (before.2.is_some() && after.2.is_some() && before.2 != after.2);
    if changed {
        return BracketOutcome::Changed;
    }
    let closed = before.0 != 0
        && after.0 != 0
        && before.1 != 0
        && after.1 != 0
        && before.2.is_some()
        && after.2.is_some();
    if closed {
        BracketOutcome::HeldStill
    } else {
        BracketOutcome::NotEstablished
    }
}

/// What bracketing the parse with two counter reads established.
///
/// One value rather than a `changed` flag beside a `held_still` flag, for the
/// reason [`Verdict`] is an enum rather than a bool: those two can be set to
/// contradict each other, and were -- "changed, and also held still" is
/// meaningless, and a test constructed it the moment both existed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BracketOutcome {
    /// Every counter was read twice and none moved.
    HeldStill,
    /// Two good reads of some counter differed: the machine changed under the
    /// run, so the parse and the counters describe different instants.
    Changed,
    /// A counter failed one of its two reads, leaving the bracket open at one
    /// end.
    ///
    /// Establishes NEITHER change nor stability, which is why it is not the
    /// absence of [`Self::Changed`]. Comparing anyway would let a concurrent
    /// hot-add be filed against the parse.
    NotEstablished,
}

/// The three counters, read machine-wide.
///
/// # Safety
///
/// The first two take no pointer arguments, so there is nothing to alias or
/// outlive; `ALL_PROCESSOR_GROUPS` is the documented way to ask for the
/// machine-wide count. `highest` is a live local for the duration of its call.
/// Failure is handled rather than dismissed: the first two report it by
/// returning 0, which `cross_check` treats as a read that did not happen.
fn read_counters() -> (u32, u16, Option<u32>) {
    // SAFETY: as stated above.
    let active = unsafe { GetActiveProcessorCount(ALL_PROCESSOR_GROUPS) };
    // SAFETY: as stated above.
    let groups = unsafe { GetActiveProcessorGroupCount() };
    let mut highest = 0u32;
    // SAFETY: as stated above.
    let highest_node = if unsafe { GetNumaHighestNodeNumber(&raw mut highest) } != 0 {
        Some(highest)
    } else {
        None
    };
    (active, groups, highest_node)
}

/// Walk-reported domains matching `is_kind` that share a processor with another.
///
/// Split out so the rule is stated once and applied to a list of kinds, rather
/// than written out per kind where a third kind would have to be remembered.
/// See [`Observation::overlapping_walk_relations`] for why overlap is a defect
/// and why only the relationship walk is considered.
fn overlapping_walk_records(
    topology: &MachineMemoryTopology,
    is_kind: impl Fn(&DomainKind) -> bool,
) -> usize {
    let members: Vec<&ProcessorSet> = topology
        .domains
        .iter()
        .filter(|domain| is_kind(&domain.kind) && domain.observed_by(Source::RelationshipWalk))
        .map(|domain| &domain.processors)
        .collect();
    members
        .iter()
        .enumerate()
        .filter(|(index, set)| {
            members
                .iter()
                .enumerate()
                .any(|(other, candidate)| other != *index && !set.is_disjoint(candidate))
        })
        .count()
}

/// Reduce a topology, three already-read counters and what their bracket
/// established to an [`Observation`].
///
/// The bracket is an ARGUMENT because only the caller knows it. This set it to
/// [`BracketOutcome::HeldStill`] itself, which claims "every counter was read
/// twice and none moved" -- a thing `observe` never establishes, having been
/// handed one set of counters and no way to know what produced them. Asserting
/// it let a caller's stale pair reach `Agree`, or be filed against the parse as
/// a `Disagree`.
///
/// Split out of [`measure`] so the extraction is reachable without a host. It
/// is the half that decides what the probe reports, and it was untestable while
/// it sat behind `MachineMemoryTopology::discover`: the defect where a NUMA node
/// only CPU Sets reported raised the domain count but could not raise
/// `highest_numa_node` lived here, and no test could construct the topology that
/// exhibits it. The counters stay in `measure` because nothing but the running
/// machine can produce them.
#[must_use]
pub fn observe(
    topology: &MachineMemoryTopology,
    raw_active_processors: u32,
    raw_group_count: u16,
    raw_highest_numa_node: Option<u32>,
    bracket: BracketOutcome,
) -> Observation {
    let online_processors = topology.processors.iter().filter(|p| p.online).count();

    let mut groups = 0usize;
    let mut numa_domains = 0usize;
    let mut numa_domains_without_processors = 0usize;
    let mut numa_domains_only_in_cpu_sets = 0usize;
    let mut numa_domains_unreported = 0usize;
    let mut numa_domains_with_conflicting_labels = 0usize;
    let mut cores_only_in_cpu_sets = 0usize;
    let mut cores_without_processors = 0usize;
    let mut packages_without_processors = 0usize;
    let mut described_relations = 0usize;
    let mut unreported_relations = 0usize;
    let mut highest_numa_node: Option<u32> = None;
    let mut packages = 0usize;
    let mut cores = Vec::new();
    let mut by_level: Vec<(u8, Vec<usize>)> = Vec::new();

    for domain in &topology.domains {
        // Provenance per RELATION, not just per topology. `Provenance::Measured`
        // is an object-level fact and the crate is explicit that it permits
        // hand-inserted relations -- `Source::Description` exists for "a caller
        // adding described relations to a topology that was discovered, which
        // is exactly the case per-relation provenance exists to make visible".
        // So a measured topology can carry counted relations nobody measured.
        //
        // **Every** observation must be a `Description`, not merely one of them.
        // `observed_by(Description)` also matched a relation the walk reported
        // and a caller then annotated -- whose membership IS platform-backed --
        // so the entry this feeds said it was "described by a caller rather than
        // reported by any platform API" of a relation the platform had reported.
        if !domain.observations.is_empty()
            && domain
                .observations
                .iter()
                .all(|o| o.source == Source::Description)
        {
            described_relations += 1;
        }
        // A relation nobody reported, which the crate documents as the honest
        // state for one built by hand. Counted for EVERY kind: this was checked
        // only inside the memory arm, so a hand-inserted core, package or cache
        // was counted -- and could change a policy or the cache partitioning --
        // with every quality check still clear.
        if domain.observations.is_empty() {
            unreported_relations += 1;
        }

        match &domain.kind {
            DomainKind::Group => groups += 1,
            DomainKind::Package => {
                packages += 1;
                // The empty-record twin of `numa_domains_without_processors`.
                // A zero affinity mask raises no anomaly, so a spurious empty
                // package inflates the count and `by-package` with it.
                if domain.processors.is_empty() {
                    packages_without_processors += 1;
                }
            }
            DomainKind::Memory { .. } => {
                numa_domains += 1;
                // Every label the crate reported for this node, from EITHER
                // source: node numbers are machine-wide from both, so all of
                // them are comparable with the machine-wide counter. Reading
                // the walk's alone hid a node only CPU Sets described.
                //
                // More than one DISTINCT label on one domain is a contradiction
                // rather than an alias, and the crate keeps both deliberately --
                // "the labels differ and both are kept, which is the whole of
                // D-15". Taking the maximum silently resolves it, so it is
                // counted and reported instead.
                // Labels from a PLATFORM source only. A caller-described label
                // is not something `GetNumaHighestNodeNumber` could have
                // reported, so folding one into the maximum files the caller's
                // number against the counter -- an accusation against the
                // shipping parse for a number no platform API produced. It is
                // reachable: a domain the walk reported AND a caller annotated
                // is platform-backed, so `described_relations` does not count
                // it and the provenance gate stays open.
                //
                // The conflict count below reads the same filtered list. It
                // establishes that a domain carries more than one DISTINCT
                // label, not which source supplied which -- nothing here groups
                // the labels by their source. Two platform sources disagreeing
                // is the usual cause on the path `measure` takes, and is not
                // what the count checks.
                let mut labels: Vec<u32> = domain
                    .observations
                    .iter()
                    .filter(|o| o.source != Source::Description)
                    .map(|o| o.label)
                    .collect();
                labels.sort_unstable();
                labels.dedup();
                if labels.len() > 1 {
                    numa_domains_with_conflicting_labels += 1;
                }
                for node in labels {
                    highest_numa_node =
                        Some(highest_numa_node.map_or(node, |seen: u32| seen.max(node)));
                }
                // A memory domain the walk never described. Not the same
                // question as `coherence`, which compares PROCESSOR SETS: two
                // sources can name the same processors and still group them
                // into nodes differently, and that lands here as a domain only
                // one of them reported. No counter can see it either -- the
                // node totals can match while the membership does not.
                // Both halves asserted, so the message this feeds stays true.
                // `!observed_by(walk)` alone is also satisfied by a domain with
                // NO observations, which "reported only by CPU Sets" would then
                // describe wrongly -- nobody reported it.
                if domain.observed_by(Source::CpuSets)
                    && !domain.observed_by(Source::RelationshipWalk)
                {
                    numa_domains_only_in_cpu_sets += 1;
                } else if domain.observations.is_empty() {
                    numa_domains_unreported += 1;
                }
                if domain.processors.is_empty() {
                    numa_domains_without_processors += 1;
                }
            }
            DomainKind::Core {
                simultaneous_multithreading,
                efficiency_class,
            } => {
                // Same provenance question as a memory domain, and the same
                // answer. `fold_memberships` pushes a core only CPU Sets
                // described as its own domain, so two sources that group the
                // same processors into cores differently leave BOTH groupings
                // here -- inflating `by-core`, which is a sizing decision.
                if domain.observed_by(Source::CpuSets)
                    && !domain.observed_by(Source::RelationshipWalk)
                {
                    cores_only_in_cpu_sets += 1;
                }
                if domain.processors.is_empty() {
                    cores_without_processors += 1;
                }
                cores.push(CoreShape {
                    simultaneous_multithreading: *simultaneous_multithreading,
                    efficiency_class: *efficiency_class,
                    processors: domain.processors.len(),
                });
            }
            _ => {}
        }
    }

    // Asked of the topology rather than counted in the loop above, and stated
    // over a LIST of kinds so the rule lives in one place: an empty affinity
    // mask is not the only way a record can fail to describe the machine, and a
    // record that overlaps another is the way none of the existing counts sees.
    //
    // MEMORY IS IN THE LIST. Writing the list out as two `+`-joined calls was
    // the same enumerate-the-causes shape this file has been bitten by before:
    // a processor belongs to exactly one NUMA node just as it belongs to one
    // package and one core, so a walk that reports it in two nodes describes no
    // machine -- and `numa_domains` feeds a policy directly. The highest-label
    // comparison cannot see it, because two overlapping nodes can carry any
    // labels at all, including the right maximum.
    let overlapping_walk_relations: usize = [
        (|kind: &DomainKind| matches!(kind, DomainKind::Package)) as fn(&DomainKind) -> bool,
        |kind| matches!(kind, DomainKind::Core { .. }),
        |kind| matches!(kind, DomainKind::Memory { .. }),
    ]
    .into_iter()
    .map(|is_kind| overlapping_walk_records(topology, is_kind))
    .sum();

    // Asked of the topology rather than counted from `domains` above, because
    // Windows reports one relationship per *cache* and not per partition.
    // Measured here: L1 arrives as eight `data` domains plus eight
    // `instruction` domains over the same eight processor pairs, so counting
    // relationships printed "L1 16 domain(s)" on a machine with eight L1
    // partitions -- and fed a doubled count to every policy in
    // `domain_counts`.
    for level in topology.cache_levels() {
        let spans = topology
            .cache_partitions_at_level(level)
            .iter()
            .map(|domain| domain.processors.len())
            .collect();
        by_level.push((level, spans));
    }

    by_level.sort_by_key(|(level, _)| *level);
    let caches = by_level
        .into_iter()
        .map(|(level, processors_per_domain)| CacheLevel {
            level,
            processors_per_domain,
        })
        .collect();

    // Asked once, here, rather than restated: the crate that owns the topology
    // owns the rule (D-21).
    let partitioning_cache_level = topology
        .outermost_partitioning_cache()
        .map(|(level, _)| level);

    Observation {
        online_processors,
        groups,
        numa_domains,
        numa_domains_without_processors,
        numa_domains_only_in_cpu_sets,
        numa_domains_unreported,
        numa_domains_with_conflicting_labels,
        topology_was_measured: topology.provenance == Provenance::Measured,
        cores_only_in_cpu_sets,
        cores_without_processors,
        packages_without_processors,
        overlapping_walk_relations,
        described_relations,
        unreported_relations,
        processor_attribute_conflicts: topology.attribute_conflicts().len(),
        highest_numa_node,
        packages,
        cores,
        caches,
        partitioning_cache_level,
        enumeration_anomalies: topology.enumeration_anomalies.clone(),
        coherence: topology.coherence.clone(),
        raw_active_processors,
        raw_group_count,
        raw_highest_numa_node,
        bracket,
    }
}
