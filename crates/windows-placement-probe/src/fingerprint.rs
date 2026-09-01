// Copyright (c) Mike Grier.

//! A one-line description of the machine, printed by every probe that measures
//! something.
//!
//! # Why this exists
//!
//! Every performance number this crate produces is a fact about a machine, and
//! is close to meaningless without knowing which. That is not a hypothetical
//! concern here: `probe-peer-index-cache` gave opposite verdicts on two hosts,
//! and when the question "did the other machine even have more than one cache
//! domain?" was asked, the answer could not be recovered -- the only record of
//! that host was the prose fragment "AMD EPYC 7763 (8C/16T, x64)" in a
//! checklist. Enough to identify the part number, not enough to say which
//! placements it could express.
//!
//! So the fingerprint travels with the measurement rather than being something
//! a reader is trusted to have written down separately.
//!
//! # The format
//!
//! One line, canonical, and short enough to paste into a table or a commit
//! message:
//!
//! ```text
//! aarch64 12p/12c smt- L2[6,6] ec[0:6,1:6] numa[12]
//! x86_64 16p/8c smt+ L3[16] ec[0:16] numa[16]
//! !!SYNTHETIC!! x86_64 32p/16c smt+ L3[16,16] ec[0:32] numa[16,16]
//! ```
//!
//! - `<arch>` -- the target architecture.
//! - `Np/Mc` -- N logical processors across M physical cores.
//! - `smt+` / `smt-` -- whether any core carries more than one processor.
//!   Written explicitly rather than left to be inferred from `N != M`, because
//!   it decides whether a whole class of placement exists.
//! - `L<level>[a,b,...]` -- the outermost cache level that *partitions* the
//!   machine, and how many processors sit behind each of its domains.
//!   `L-[N]` when no cache level divides it. Keyed on "the level that
//!   partitions" rather than literally on L3, because the development host
//!   reports no L3 at all.
//! - `ec[class:count,...]` -- efficiency classes and their sizes. A single
//!   entry means a homogeneous machine.
//! - `numa[...]` -- processors per NUMA node.
//!
//! A line **prefixed `!!SYNTHETIC!!` or `!!RESTORED!!` did not come from the
//! machine that printed it.** The first was fabricated; the second was loaded
//! from a description of some machine, which is not the same as a description
//! of this one. A measured host carries no prefix at all, so every fingerprint
//! recorded before this marker existed remains valid and comparable.
//!
//! The prefix is deliberately inside the string rather than reported beside
//! it. Because the string is canonical (below), a marker kept outside would let
//! a synthetic host compare *equal* to a real one -- and the comparison is the
//! whole point of having a canonical form.
//!
//! **It is a canonical summary of the machine's marginal shape.** Two hosts
//! rendering the same string have the same processor, core, cache-domain,
//! efficiency-class and NUMA-node *sizes*, which makes string equality a usable
//! way to group results by shape. It deliberately omits clock speeds, cache
//! sizes, and model names: those vary without changing which experiments are
//! possible, and a fingerprint that changes when the answer does not is a
//! fingerprint nobody can compare.
//!
//! **Equal strings do not mean the two hosts can express the same placements.**
//! Every partition is recorded as a list of sizes and never as how those
//! partitions intersect, so two hosts can agree here and still offer different
//! pairs to measure. That is set out in full on the `provenance` field of
//! [`Fingerprint`](crate::fingerprint::Fingerprint), along with why it is not a
//! key for pooling measurements; do not restate the stronger claim here, which
//! is what an earlier revision of this header did.
//!
//! (The path is spelled from the crate root deliberately: `lib.rs` also carries
//! an outer doc comment on `pub mod fingerprint;`, so the merged documentation
//! resolves its links in the parent's scope, where the bare name is not in
//! scope.)

use std::fmt;

use windows_topology_sys::{DomainKind, Provenance, Topology};

/// One logical processor's position in the machine.
///
/// Lives here rather than with the affinity experiment because it is a fact
/// about the *host*, not about any one measurement: the slice below reports it,
/// and the placement classifier interprets it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessorPlace {
    /// Which processor group it belongs to.
    ///
    /// **A processor is identified by `(group, number)`, never by `number`
    /// alone.** Windows splits a machine with more than 64 logical processors
    /// into groups, each numbering from zero, so every group has a processor 5.
    /// Keying on the number alone silently collapses them -- and it fails
    /// quietly rather than loudly, because numbers stay below 64 within a group,
    /// so no bound check fires. The run then pins to whichever processor won the
    /// collision and prints a confident table describing a topology that does
    /// not exist. That is the exact hazard on the large multi-socket hosts this
    /// tool is written for.
    pub group: u16,
    /// Its number within [`Self::group`].
    pub number: u8,
    /// Which physical core it belongs to.
    ///
    /// Two processors sharing a core are SMT siblings, which is the tightest
    /// coupling a machine can offer -- they share L1 outright. On a host
    /// without SMT every core has exactly one processor and this only ever
    /// distinguishes cores.
    pub core: u32,
    /// Windows's efficiency class. Higher is faster; the values themselves are
    /// only meaningful relative to each other on the same machine.
    pub efficiency_class: u8,
    /// Which cache domain it sits behind, at the outermost level that
    /// partitions the machine, or `None` if no level does.
    pub cache_domain: Option<u32>,
    /// Which NUMA node it belongs to.
    ///
    /// Carried even though every host measured so far reports a single node,
    /// because the cost of *not* carrying it is paid at exactly the wrong
    /// moment. Without it a cross-node pair is indistinguishable from a
    /// cross-cache one, so a scarce run on a genuine NUMA machine would record
    /// a node crossing as a cache effect and nothing in the output would say
    /// which had been measured.
    pub numa_node: u32,
}

impl ProcessorPlace {
    /// This processor's full identity, as the pinning call needs it.
    ///
    /// Exists so no call site is tempted to pass a bare `number`, which is the
    /// whole defect: a number without its group names a different processor in
    /// every group, and names the wrong one in all but the first.
    #[must_use]
    pub fn id(self) -> (u16, u8) {
        (self.group, self.number)
    }
}

impl fmt::Display for ProcessorPlace {
    /// Renders the group **always**, including group 0 on a machine that has
    /// only one.
    ///
    /// Printing it only when non-zero would keep single-group output unchanged
    /// and still distinguish the groups on a large host, so it is tempting. It
    /// is refused because the failure this guards against is precisely a tool
    /// that *silently collapsed* the groups: a bare `cpu5` cannot tell a reader
    /// whether the group was considered and was zero, or never consulted at all.
    /// Group-awareness is cheap to show and expensive to assume.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "g{}/cpu{}/core{}/ec{}",
            self.group, self.number, self.core, self.efficiency_class
        )?;
        match self.cache_domain {
            Some(id) => write!(f, "/cd{id}")?,
            None => write!(f, "/cd-")?,
        }
        write!(f, "/n{}", self.numa_node)
    }
}

/// Which part of the machine an experiment actually ran on.
///
/// # Why this is recorded beside the host
///
/// The host fingerprint says which experiments a machine *can* express; it does
/// not say which one was run. Two measurements from the same host can differ by
/// 5.6x purely because of where their threads were placed, so a number labelled
/// only with the host is still ambiguous -- and an unpinned run is ambiguous
/// even against itself, because the scheduler is free to choose differently on
/// the next invocation.
///
/// So an unpinned run says so, plainly, instead of rendering a slice it does
/// not actually have. That distinction is the point: "these threads ran here"
/// and "these threads ran wherever Windows put them" are different claims, and
/// only the first supports comparison between runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Slice {
    /// Threads were left to the scheduler, which may place them differently on
    /// every invocation.
    Unpinned {
        /// How many threads participated, since that is all that is knowable.
        threads: usize,
    },
    /// Each named thread was confined to one processor.
    Pinned {
        /// `(role, where it ran)`, in the order the experiment defines.
        participants: Vec<(&'static str, ProcessorPlace)>,
    },
}

impl Slice {
    /// A two-participant pinned slice, the common case.
    #[must_use]
    pub fn pair(producer: ProcessorPlace, consumer: ProcessorPlace) -> Self {
        Self::Pinned {
            participants: vec![("prod", producer), ("cons", consumer)],
        }
    }

    /// Whether every participant sits behind the same cache domain.
    ///
    /// `None` when the slice is unpinned, because the question has no answer
    /// rather than a negative one.
    #[must_use]
    pub fn same_cache_domain(&self) -> Option<bool> {
        let Self::Pinned { participants } = self else {
            return None;
        };
        let mut domains = participants.iter().map(|(_, place)| place.cache_domain);
        let first = domains.next()?;
        Some(domains.all(|domain| domain == first))
    }

    /// Whether every participant is of the same efficiency class.
    ///
    /// `None` when the slice is unpinned, for the same reason.
    #[must_use]
    pub fn same_efficiency_class(&self) -> Option<bool> {
        let Self::Pinned { participants } = self else {
            return None;
        };
        let mut classes = participants.iter().map(|(_, place)| place.efficiency_class);
        let first = classes.next()?;
        Some(classes.all(|class| class == first))
    }
}

impl fmt::Display for Slice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unpinned { threads } => {
                write!(
                    f,
                    "unpinned {threads}thr (scheduler-placed, not reproducible)"
                )
            }
            Self::Pinned { participants } => {
                write!(f, "pinned")?;
                for (role, place) in participants {
                    write!(f, " {role}={place}")?;
                }
                // The relationship, spelled out, so a reader does not have to
                // compare the domain ids themselves.
                let cache = match self.same_cache_domain() {
                    Some(true) => "same-cache",
                    Some(false) => "cross-cache",
                    None => "?-cache",
                };
                let class = match self.same_efficiency_class() {
                    Some(true) => "same-class",
                    Some(false) => "cross-class",
                    None => "?-class",
                };
                write!(f, " [{cache},{class}]")
            }
        }
    }
}

/// A machine's shape, in the terms that decide which placements exist.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Fingerprint {
    /// Target architecture, as `std::env::consts::ARCH` reports it.
    pub arch: &'static str,
    /// Logical processors.
    pub processors: usize,
    /// Physical cores.
    pub cores: usize,
    /// Whether any core carries more than one logical processor.
    pub smt: bool,
    /// The outermost cache level that divides the machine, if any.
    pub partitioning_cache_level: Option<u8>,
    /// Processors behind each domain of that cache level, ascending.
    pub cache_domain_sizes: Vec<usize>,
    /// `(efficiency class, processor count)`, ascending by class.
    pub efficiency_classes: Vec<(u8, usize)>,
    /// Processors per NUMA node, ascending.
    pub numa_node_sizes: Vec<usize>,
    /// Where the topology behind this fingerprint came from.
    ///
    /// Rendered *inside* the string, not beside it. A marker kept outside would
    /// leave a synthetic host comparing equal to a real one, which is the
    /// specific bug this prevents rather than a display nicety.
    ///
    /// # What equal fingerprints do and do not mean
    ///
    /// **Equal strings mean equal marginal shape. They do not mean the two
    /// hosts can express the same placements**, and an earlier version of this
    /// note claimed they did.
    ///
    /// Every partition here is recorded as a list of *sizes* -- processors per
    /// cache domain, per efficiency class, per NUMA node -- and never as how
    /// those partitions intersect. Two eight-processor hosts can both render
    /// `L2[4,4] ec[0:4,1:4] numa[4,4]` while one puts each efficiency class in
    /// its own cache domain and the other splits both classes across both. Only
    /// the second can express a same-cache/cross-class pair; only the first can
    /// express cross-cache/same-class cleanly. The placements available to a
    /// run therefore differ while the fingerprint agrees.
    ///
    /// So this is a summary for a banner and for grouping *by shape*, not a
    /// key for pooling measurements. A consumer that needs placement
    /// equivalence should read the placements a record actually reports, which
    /// name themselves in every measurement row, rather than inferring them
    /// from this string. Making the string canonical again would mean carrying
    /// a placement signature, which is a serialized field and so a schema bump;
    /// that is tracked as `PT-6.1` rather than done here.
    pub provenance: Provenance,
}

impl Fingerprint {
    /// Read this machine's shape.
    ///
    /// # Errors
    ///
    /// Returns whatever [`Topology::discover`] failed with.
    pub fn discover() -> std::io::Result<Self> {
        Ok(Self::from_topology(&Topology::discover()?))
    }

    /// Read a shape from any topology, discovered or not.
    ///
    /// Separate from [`Self::discover`] so provenance *flows* rather than being
    /// stamped on afterwards: whatever the topology says about where it came
    /// from is what the fingerprint reports, and there is no path here that
    /// invents the answer.
    #[must_use]
    pub fn from_topology(topology: &Topology) -> Self {
        let cores: Vec<_> = topology.cores().collect();
        let processors: usize = cores.iter().map(|core| core.processors.len()).sum();
        let smt = cores.iter().any(|core| core.processors.len() > 1);

        let mut efficiency_classes: Vec<(u8, usize)> = Vec::new();
        for core in &cores {
            let DomainKind::Core {
                efficiency_class, ..
            } = core.kind
            else {
                continue;
            };
            let count = core.processors.len();
            match efficiency_classes
                .iter_mut()
                .find(|(class, _)| *class == efficiency_class)
            {
                Some((_, total)) => *total += count,
                None => efficiency_classes.push((efficiency_class, count)),
            }
        }
        efficiency_classes.sort_unstable();

        // The outermost level that actually divides the machine, asked of the
        // topology rather than recomputed here: `Topology` owns that rule, and
        // a second statement of it drifts. It also deduplicates a level
        // reported once per cache -- an L1 arriving as separate `data` and
        // `instruction` domains over the same processors is two relationships
        // but one partition, and counting relationships would put a doubled
        // domain count into the fingerprint.
        let mut partitioning_cache_level = None;
        let mut cache_domain_sizes = Vec::new();
        if let Some((level, partitions)) = topology.outermost_partitioning_cache() {
            partitioning_cache_level = Some(level);
            cache_domain_sizes = partitions
                .iter()
                .map(|domain| domain.processors.len())
                .collect();
        }
        cache_domain_sizes.sort_unstable();
        if partitioning_cache_level.is_none() {
            cache_domain_sizes = vec![processors];
        }

        let mut numa_node_sizes: Vec<usize> = topology
            .memory_domains()
            .map(|domain| domain.processors.len())
            .filter(|size| *size > 0)
            .collect();
        numa_node_sizes.sort_unstable();

        Self {
            arch: std::env::consts::ARCH,
            processors,
            cores: cores.len(),
            smt,
            partitioning_cache_level,
            cache_domain_sizes,
            efficiency_classes,
            numa_node_sizes,
            provenance: topology.provenance,
        }
    }

    /// Whether this machine is heterogeneous.
    #[must_use]
    pub fn heterogeneous(&self) -> bool {
        self.efficiency_classes.len() > 1
    }

    /// Whether any cache level divides this machine.
    #[must_use]
    pub fn partitioned(&self) -> bool {
        self.cache_domain_sizes.len() > 1
    }
}

impl fmt::Display for Fingerprint {
    /// Renders the shape, preceded by a taint marker when the topology behind
    /// it was not measured.
    ///
    /// A measured fingerprint renders exactly as it always did, so every string
    /// already recorded in a checklist or design note stays valid and
    /// comparable. Only the untrusted cases gain a prefix, and they gain it at
    /// the *front*, where a reader scanning a column of results cannot skip it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.provenance.is_measured() {
            write!(f, "!!{}!! ", self.provenance)?;
        }
        write!(
            f,
            "{} {}p/{}c smt{}",
            self.arch,
            self.processors,
            self.cores,
            if self.smt { '+' } else { '-' }
        )?;

        match self.partitioning_cache_level {
            Some(level) => write!(f, " L{level}[")?,
            None => write!(f, " L-[")?,
        }
        for (index, size) in self.cache_domain_sizes.iter().enumerate() {
            if index > 0 {
                write!(f, ",")?;
            }
            write!(f, "{size}")?;
        }
        write!(f, "] ec[")?;
        for (index, (class, count)) in self.efficiency_classes.iter().enumerate() {
            if index > 0 {
                write!(f, ",")?;
            }
            write!(f, "{class}:{count}")?;
        }
        write!(f, "] numa[")?;
        for (index, size) in self.numa_node_sizes.iter().enumerate() {
            if index > 0 {
                write!(f, ",")?;
            }
            write!(f, "{size}")?;
        }
        write!(f, "]")
    }
}

/// Discover where each logical processor sits.
///
/// # Errors
///
/// Returns whatever [`Topology::discover`] failed with.
pub fn discover_places() -> std::io::Result<Vec<ProcessorPlace>> {
    Ok(places_from_topology(&Topology::discover()?))
}

/// Work out where each logical processor sits, in any topology.
///
/// # Why this seam exists when `measure` deliberately has none
///
/// This is a **pure conversion**: topology in, positions out, nothing measured
/// and nothing pinned. Feeding it a synthetic topology yields synthetic
/// positions, which is exactly what a caller asked for and cannot be mistaken
/// for a measurement. The seam refused on
/// [`measure`](crate::core_affinity::measure) is a different thing entirely --
/// there, a synthetic topology's processor *numbers* would still be valid on
/// the real host, so every pin would succeed and real timings would be filed
/// under fabricated labels.
///
/// The distinction is the rule: **a seam that only moves data is safe; a seam
/// that lets fabricated labels reach real hardware is not.**
///
/// Without this, the rules below -- which cache level partitions the machine,
/// which core and class each processor belongs to, and which NUMA node -- could
/// only ever execute against whatever machine ran the suite. The NUMA mapping
/// in particular was unverifiable on a single-node host, where a completely
/// broken lookup and a correct one both yield node 0.
#[must_use]
pub fn places_from_topology(topology: &Topology) -> Vec<ProcessorPlace> {
    // Every map here is keyed by the full `(group, number)` pair. Keying on the
    // number alone is the defect this function is written against: on a machine
    // with more than 64 logical processors each group numbers from zero, so
    // group 1's processor 5 would overwrite group 0's and the machine would
    // silently shrink to one group's worth of processors.
    let mut class_of = std::collections::BTreeMap::new();
    let mut core_of = std::collections::BTreeMap::new();
    for core in topology.cores() {
        for id in core.processors.iter() {
            core_of.insert(id, core.id);
        }
        let DomainKind::Core {
            efficiency_class, ..
        } = core.kind
        else {
            continue;
        };
        for id in core.processors.iter() {
            class_of.insert(id, efficiency_class);
        }
    }

    // The outermost cache level that actually divides the machine. This calls
    // the same `Topology` method the fingerprint does, rather than repeating
    // the rule, so the two cannot disagree about which level partitions the
    // host or about how many partitions it has.
    let mut cache_of = std::collections::BTreeMap::new();
    if let Some((_, partitions)) = topology.outermost_partitioning_cache() {
        for domain in partitions {
            for id in domain.processors.iter() {
                cache_of.insert(id, domain.id);
            }
        }
    }

    // Node 0 is the correct default rather than a fallback: a machine with no
    // NUMA partitioning has exactly one node, and every processor is in it.
    let mut numa_of = std::collections::BTreeMap::new();
    for domain in topology.memory_domains() {
        for id in domain.processors.iter() {
            numa_of.insert(id, domain.id);
        }
    }

    class_of
        .into_iter()
        .map(|(id, efficiency_class)| {
            let (group, number) = id;
            ProcessorPlace {
                group,
                number,
                // The fallback keeps distinct processors distinct across groups:
                // a topology that reports no core for this processor must not
                // collapse group 1's cpu5 onto group 0's.
                core: core_of
                    .get(&id)
                    .copied()
                    .unwrap_or_else(|| u32::from(group) << 8 | u32::from(number)),
                efficiency_class,
                cache_domain: cache_of.get(&id).copied(),
                numa_node: numa_of.get(&id).copied().unwrap_or(0),
            }
        })
        .collect()
}

/// Print the host fingerprint as a probe's first line, or say why it could not
/// be read.
///
/// Never fails the probe: a measurement without a fingerprint is still worth
/// having, and is far better than one that refused to run. But it says so
/// loudly, because an unlabelled number is what this exists to prevent.
pub fn print_banner() {
    println!("{}", banner_line());
}

/// The banner as a string, so what a probe prints can be asserted rather than
/// inspected.
///
/// Separated from [`print_banner`] for one reason: the taint marker reaching
/// this line is the whole point of carrying provenance, and a property that
/// matters that much should not rest on a human having read the format string.
#[must_use]
pub fn banner_line() -> String {
    match Fingerprint::discover() {
        Ok(fingerprint) => format!("host:  {fingerprint}"),
        Err(error) => format!("host:  UNKNOWN -- topology discovery failed: {error}"),
    }
}

/// Print the host fingerprint and the slice one measurement ran on.
///
/// Both, always: the host says which experiments the machine can express, and
/// the slice says which one this number came from. Either alone leaves a
/// reader unable to tell whether two figures are comparable.
pub fn print_banner_with(slice: &Slice) {
    print_banner();
    println!("slice: {slice}");
}

#[cfg(test)]
mod tests;
