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
//! The parse comes from [`windows_topology_sys::Topology::discover`] rather
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

use windows_topology_sys::{DomainKind, Topology};

/// One cache level, summarised across the machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheLevel {
    /// 1, 2, 3, ... as the firmware reports it.
    pub level: u8,
    /// How many distinct domains exist at this level.
    pub domains: usize,
    /// Processors per domain, in discovery order.
    pub processors_per_domain: Vec<usize>,
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
    pub memoryless_numa_domains: usize,
    /// Physical packages (sockets).
    pub packages: usize,
    /// Every physical core.
    pub cores: Vec<CoreShape>,
    /// Cache levels, ascending, each summarised across the machine.
    pub caches: Vec<CacheLevel>,

    // --- read independently through Win32 ---
    /// `GetActiveProcessorCount(ALL_PROCESSOR_GROUPS)`.
    pub raw_active_processors: u32,
    /// `GetActiveProcessorGroupCount()`.
    pub raw_group_count: u16,
    /// `GetNumaHighestNodeNumber()`, or `None` if the call failed.
    pub raw_highest_numa_node: Option<u32>,
}

impl Observation {
    /// The outermost cache level that actually splits the machine into more
    /// than one domain, if any.
    ///
    /// This is the rule the design wants, and it is deliberately *not* "level
    /// 3". A shipping ARM64 laptop measured during the 2026-08-30 session
    /// reports **no L3 at all**, with two L2 domains of six processors forming
    /// the real cluster boundary, which is why the heuristic is phrased over
    /// "the outermost level that partitions" rather than over a fixed number.
    #[must_use]
    pub fn outermost_partitioning_cache(&self) -> Option<&CacheLevel> {
        self.caches
            .iter()
            .filter(|c| c.domains > 1)
            .max_by_key(|c| c.level)
    }

    /// How many execution domains each candidate policy would produce.
    ///
    /// Reported rather than recommended. The point of printing all of them is
    /// that they disagree, and the disagreement is the finding.
    #[must_use]
    pub fn domain_counts(&self) -> Vec<(&'static str, usize)> {
        vec![
            ("single", 1),
            ("by-package", self.packages),
            (
                "by-numa-domain-with-processors",
                self.numa_domains - self.memoryless_numa_domains,
            ),
            (
                "by-outermost-partitioning-cache",
                self.outermost_partitioning_cache().map_or(1, |c| c.domains),
            ),
            ("by-core", self.cores.len()),
        ]
    }

    /// Whether the independently-read Win32 counters agree with what the
    /// topology crate parsed.
    ///
    /// A disagreement is a real finding: it means the shipping crate's parse of
    /// `GetLogicalProcessorInformationEx` diverges from what the simple
    /// counters report on this machine.
    #[must_use]
    pub fn cross_check(&self) -> Vec<String> {
        let mut complaints = Vec::new();
        if self.online_processors != self.raw_active_processors as usize {
            complaints.push(format!(
                "online processors: topology crate says {}, GetActiveProcessorCount says {}",
                self.online_processors, self.raw_active_processors
            ));
        }
        if self.groups != self.raw_group_count as usize {
            complaints.push(format!(
                "groups: topology crate says {}, GetActiveProcessorGroupCount says {}",
                self.groups, self.raw_group_count
            ));
        }
        if let Some(highest) = self.raw_highest_numa_node
            && self.numa_domains != highest as usize + 1
        {
            complaints.push(format!(
                "NUMA domains: topology crate says {}, GetNumaHighestNodeNumber implies {}",
                self.numa_domains,
                highest + 1
            ));
        }
        complaints
    }
}

/// Discover the machine's shape.
///
/// # Errors
///
/// Propagates a failure from [`Topology::discover`].
pub fn measure() -> io::Result<Observation> {
    let topology = Topology::discover()?;

    let online_processors = topology.processors.iter().filter(|p| p.online).count();

    let mut groups = 0usize;
    let mut numa_domains = 0usize;
    let mut memoryless_numa_domains = 0usize;
    let mut packages = 0usize;
    let mut cores = Vec::new();
    let mut by_level: Vec<(u8, Vec<usize>)> = Vec::new();

    for domain in &topology.domains {
        match &domain.kind {
            DomainKind::Group => groups += 1,
            DomainKind::Package => packages += 1,
            DomainKind::Memory { .. } => {
                numa_domains += 1;
                if domain.processors.is_empty() {
                    memoryless_numa_domains += 1;
                }
            }
            DomainKind::Core {
                simultaneous_multithreading,
                efficiency_class,
            } => cores.push(CoreShape {
                simultaneous_multithreading: *simultaneous_multithreading,
                efficiency_class: *efficiency_class,
                processors: domain.processors.len(),
            }),
            DomainKind::Cache { level, .. } => {
                let count = domain.processors.len();
                match by_level.iter_mut().find(|(l, _)| l == level) {
                    Some((_, spans)) => spans.push(count),
                    None => by_level.push((*level, vec![count])),
                }
            }
            _ => {}
        }
    }

    by_level.sort_by_key(|(level, _)| *level);
    let caches = by_level
        .into_iter()
        .map(|(level, processors_per_domain)| CacheLevel {
            level,
            domains: processors_per_domain.len(),
            processors_per_domain,
        })
        .collect();

    // SAFETY: both take no pointer arguments and cannot fail in a way that
    // matters here; `ALL_PROCESSOR_GROUPS` is the documented way to ask for the
    // machine-wide count.
    let raw_active_processors = unsafe { GetActiveProcessorCount(ALL_PROCESSOR_GROUPS) };
    let raw_group_count = unsafe { GetActiveProcessorGroupCount() };

    let mut highest = 0u32;
    // SAFETY: `highest` is a live local for the duration of the call.
    let raw_highest_numa_node = if unsafe { GetNumaHighestNodeNumber(&raw mut highest) } != 0 {
        Some(highest)
    } else {
        None
    };

    Ok(Observation {
        online_processors,
        groups,
        numa_domains,
        memoryless_numa_domains,
        packages,
        cores,
        caches,
        raw_active_processors,
        raw_group_count,
        raw_highest_numa_node,
    })
}
