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
//! **It is canonical**, so two hosts that render the same string can express
//! the same placements, and string equality is a usable comparison. It
//! deliberately omits clock speeds, cache sizes, and model names: those vary
//! without changing which experiments are possible, and a fingerprint that
//! changes when the answer does not is a fingerprint nobody can compare.

use std::fmt;

use windows_topology_sys::{DomainKind, Topology};

/// One logical processor's position in the machine.
///
/// Lives here rather than with the affinity experiment because it is a fact
/// about the *host*, not about any one measurement: the slice below reports it,
/// and the placement classifier interprets it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessorPlace {
    /// Its number within the (single) processor group.
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

impl fmt::Display for ProcessorPlace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "cpu{}/core{}/ec{}",
            self.number, self.core, self.efficiency_class
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
}

impl Fingerprint {
    /// Read this machine's shape.
    ///
    /// # Errors
    ///
    /// Returns whatever [`Topology::discover`] failed with.
    pub fn discover() -> std::io::Result<Self> {
        let topology = Topology::discover()?;

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

        // The outermost level that actually divides the machine. A level with
        // one domain covers everything and partitions nothing.
        let mut partitioning_cache_level = None;
        let mut cache_domain_sizes = Vec::new();
        for level in 1..=4_u8 {
            let sizes: Vec<usize> = topology
                .caches_at_level(level)
                .map(|domain| domain.processors.len())
                .collect();
            if sizes.len() > 1 {
                partitioning_cache_level = Some(level);
                cache_domain_sizes = sizes;
            }
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

        Ok(Self {
            arch: std::env::consts::ARCH,
            processors,
            cores: cores.len(),
            smt,
            partitioning_cache_level,
            cache_domain_sizes,
            efficiency_classes,
            numa_node_sizes,
        })
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
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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
    let topology = Topology::discover()?;

    let mut class_of = std::collections::BTreeMap::new();
    let mut core_of = std::collections::BTreeMap::new();
    for core in topology.cores() {
        for (_group, number) in core.processors.iter() {
            core_of.insert(number, core.id);
        }
        let DomainKind::Core {
            efficiency_class, ..
        } = core.kind
        else {
            continue;
        };
        for (_group, number) in core.processors.iter() {
            class_of.insert(number, efficiency_class);
        }
    }

    // The outermost cache level that actually divides the machine, matching
    // the fingerprint's own rule so the two cannot disagree.
    let mut cache_of = std::collections::BTreeMap::new();
    for level in 1..=4_u8 {
        let domains: Vec<_> = topology.caches_at_level(level).collect();
        if domains.len() > 1 {
            cache_of.clear();
            for domain in domains {
                for (_group, number) in domain.processors.iter() {
                    cache_of.insert(number, domain.id);
                }
            }
        }
    }

    // Node 0 is the correct default rather than a fallback: a machine with no
    // NUMA partitioning has exactly one node, and every processor is in it.
    let mut numa_of = std::collections::BTreeMap::new();
    for domain in topology.memory_domains() {
        for (_group, number) in domain.processors.iter() {
            numa_of.insert(number, domain.id);
        }
    }

    Ok(class_of
        .into_iter()
        .map(|(number, efficiency_class)| ProcessorPlace {
            number,
            core: core_of.get(&number).copied().unwrap_or(u32::from(number)),
            efficiency_class,
            cache_domain: cache_of.get(&number).copied(),
            numa_node: numa_of.get(&number).copied().unwrap_or(0),
        })
        .collect())
}

/// Print the host fingerprint as a probe's first line, or say why it could not
/// be read.
///
/// Never fails the probe: a measurement without a fingerprint is still worth
/// having, and is far better than one that refused to run. But it says so
/// loudly, because an unlabelled number is what this exists to prevent.
pub fn print_banner() {
    match Fingerprint::discover() {
        Ok(fingerprint) => println!("host:  {fingerprint}"),
        Err(error) => println!("host:  UNKNOWN -- topology discovery failed: {error}"),
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
