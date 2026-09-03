// Copyright (c) 2026 Mike Grier
//! Assembling a [`MachineMemoryTopology`] from discovered relations.

use std::io;

use crate::cpu_set::CpuSet;
use crate::domain::{Domain, DomainKind, Processor, ProcessorId};
use crate::observation::{Observation, Source};
use crate::provenance::Provenance;
use crate::relation::{self, Relations};

/// A processor, cache, and memory topology: a set of processors and the
/// domains that relate them.
///
/// Built either by [`MachineMemoryTopology::discover`] from the running system, by hand,
/// or (with the `serde` feature) by deserializing a fed-in description.
///
/// The JSON shape this produces and accepts is explicitly not covered by this
/// crate's semver contract -- see [`Domain`]'s documentation and D-8 in
/// `DESIGN-NOTES.md`.
#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MachineMemoryTopology {
    /// Every logical processor, including one for each inactive slot up to a
    /// group's maximum processor count.
    pub processors: Vec<Processor>,
    /// Every domain.
    pub domains: Vec<Domain>,
    /// What `GetSystemCpuSetInformation` reported, as **its own observation**.
    ///
    /// Windows describes processors through two APIs, and this is the second
    /// one. It is not a more convenient spelling of [`Self::domains`]: it
    /// carries facts the relationship walk has no equivalent for -- whether a
    /// processor is parked, whether it is allocated to *this* process, the
    /// scheduler's own last-level-cache grouping, a scheduling class and an
    /// allocation tag.
    ///
    /// It also **duplicates** some facts, deliberately and without
    /// reconciliation. `CoreIndex`, `NumaNodeIndex` and `EfficiencyClass` also
    /// appear, derived differently, in `domains` and [`Self::processors`]. The
    /// two paths can disagree -- under a hypervisor, or where one is stale --
    /// and merging them here would silently pick a winner and destroy the
    /// disagreement, which is the only thing a second observer is *for*. Which
    /// of them a consumer should believe, and what to do when they differ, is a
    /// decision that has not been taken.
    ///
    /// `None` means **not observed**, which is not the same as observed-and-
    /// empty: a hand-built or deserialized topology has not asked the running
    /// system, and a consumer must be able to tell that from a machine that
    /// genuinely reported nothing.
    #[cfg_attr(feature = "serde", serde(default))]
    pub cpu_sets: Option<Vec<CpuSet>>,
    /// Where this content came from.
    ///
    /// **Defaults to [`Provenance::Synthetic`]**, so a topology built by hand
    /// or by [`Default`] is tainted unless its author says otherwise. Only
    /// [`Self::discover`] produces [`Provenance::Measured`], and
    /// deserialization can never produce it -- see
    /// [`Provenance`] for why the default points this way.
    #[cfg_attr(
        feature = "serde",
        serde(
            default,
            deserialize_with = "crate::provenance::deserialize_downgraded"
        )
    )]
    pub provenance: Provenance,
}

impl MachineMemoryTopology {
    /// Discover the running system's topology.
    ///
    /// # Errors
    ///
    /// Returns any error from the underlying `GetLogicalProcessorInformationEx`
    /// or `GetSystemCpuSetInformation` calls.
    pub fn discover() -> io::Result<Self> {
        let relations = relation::discover()?;
        let mut topology = Self::from_relations(relations);
        // The second observation, kept beside the first rather than folded into
        // it. Both are cheap reads of the running system, so both belong to
        // discovery -- neither is a measurement in the sense that would make it
        // expensive or optional.
        topology.cpu_sets = Some(crate::cpu_set::enumerate()?);
        // The one place in the crate that may claim this is the machine you are
        // on, because it is the one place that asked the operating system.
        topology.provenance = Provenance::Measured;
        Ok(topology)
    }

    fn from_relations(relations: Relations) -> Self {
        let mut domains = Vec::new();

        for group in &relations.groups {
            domains.push(Domain {
                kind: DomainKind::Group,
                id: u32::from(group.group),
                processors: group.active_processors.clone(),
                observations: vec![Observation::new(
                    Source::RelationshipWalk,
                    u32::from(group.group),
                )],
            });
        }
        for (index, package) in relations.packages.iter().enumerate() {
            domains.push(Domain {
                kind: DomainKind::Package,
                id: index as u32,
                processors: package.processors.clone(),
                observations: vec![Observation::new(Source::RelationshipWalk, index as u32)],
            });
        }
        for (index, die) in relations.dies.iter().enumerate() {
            domains.push(Domain {
                kind: DomainKind::Die,
                id: index as u32,
                processors: die.processors.clone(),
                observations: vec![Observation::new(Source::RelationshipWalk, index as u32)],
            });
        }
        for (index, module) in relations.modules.iter().enumerate() {
            domains.push(Domain {
                kind: DomainKind::Module,
                id: index as u32,
                processors: module.processors.clone(),
                observations: vec![Observation::new(Source::RelationshipWalk, index as u32)],
            });
        }
        for (index, core) in relations.cores.iter().enumerate() {
            domains.push(Domain {
                kind: DomainKind::Core {
                    simultaneous_multithreading: core.simultaneous_multithreading,
                    efficiency_class: core.efficiency_class,
                },
                id: index as u32,
                processors: core.processors.clone(),
                observations: vec![Observation::new(Source::RelationshipWalk, index as u32)],
            });
        }
        for (index, cache) in relations.caches.iter().enumerate() {
            domains.push(Domain {
                kind: DomainKind::Cache {
                    level: cache.level,
                    associativity: cache.associativity,
                    line_size: cache.line_size,
                    size_bytes: cache.cache_size,
                    cache_type: cache.cache_type,
                },
                id: index as u32,
                processors: cache.processors.clone(),
                observations: vec![Observation::new(Source::RelationshipWalk, index as u32)],
            });
        }
        for node in &relations.numa_nodes {
            domains.push(Domain {
                kind: DomainKind::Memory { memory_bytes: None },
                id: node.node_number,
                processors: node.processors.clone(),
                observations: vec![Observation::new(Source::RelationshipWalk, node.node_number)],
            });
        }

        let processors = Self::processors_from(&relations, &domains);
        Self {
            processors,
            domains,
            cpu_sets: None,
            // Synthetic, not measured: this is a pure transform of whatever
            // relations it was handed, and cannot know where they came from.
            // `discover` stamps the claim because `discover` is what read the
            // machine -- so if this ever gains a second caller, that caller
            // does not silently inherit an assertion it has not earned.
            provenance: Provenance::Synthetic,
        }
    }

    /// One `Processor` entry per slot up to each group's maximum processor
    /// count, online or not -- Windows only reports core/cache/node relations
    /// for active processors, so an inactive slot's capacity falls back to
    /// `0` rather than being invented.
    fn processors_from(relations: &Relations, domains: &[Domain]) -> Vec<Processor> {
        let mut result = Vec::new();
        for group_info in &relations.groups {
            for number in 0..group_info.maximum_processor_count {
                let id = ProcessorId {
                    group: group_info.group,
                    number,
                };
                let online = group_info
                    .active_processors
                    .contains(group_info.group, number);
                let capacity = online
                    .then(|| {
                        domains.iter().find_map(|d| match d.kind {
                            DomainKind::Core {
                                efficiency_class, ..
                            } if d.processors.contains(group_info.group, number) => {
                                Some(u32::from(efficiency_class))
                            }
                            _ => None,
                        })
                    })
                    .flatten()
                    .unwrap_or(0);
                result.push(Processor {
                    id,
                    online,
                    capacity,
                });
            }
        }
        result
    }

    /// The processor named by `id`, if this topology has one.
    #[must_use]
    pub fn processor(&self, id: ProcessorId) -> Option<&Processor> {
        self.processors.iter().find(|p| p.id == id)
    }

    /// Every domain of kind [`DomainKind::Group`].
    pub fn groups(&self) -> impl Iterator<Item = &Domain> {
        self.domains
            .iter()
            .filter(|d| matches!(d.kind, DomainKind::Group))
    }

    /// Every domain of kind [`DomainKind::Package`].
    pub fn packages(&self) -> impl Iterator<Item = &Domain> {
        self.domains
            .iter()
            .filter(|d| matches!(d.kind, DomainKind::Package))
    }

    /// Every domain of kind [`DomainKind::Core`].
    pub fn cores(&self) -> impl Iterator<Item = &Domain> {
        self.domains
            .iter()
            .filter(|d| matches!(d.kind, DomainKind::Core { .. }))
    }

    /// Every cache domain, at any level.
    pub fn caches(&self) -> impl Iterator<Item = &Domain> {
        self.domains
            .iter()
            .filter(|d| matches!(d.kind, DomainKind::Cache { .. }))
    }

    /// Every cache domain at exactly `level`.
    pub fn caches_at_level(&self, level: u8) -> impl Iterator<Item = &Domain> {
        self.domains.iter().filter(
            move |d| matches!(&d.kind, DomainKind::Cache { level: found, .. } if *found == level),
        )
    }

    /// Every cache level this machine reports, ascending, without repeats.
    ///
    /// Derived from what the topology actually contains rather than from a
    /// fixed ceiling. [`DomainKind::Cache`]'s `level` is a `u8`, so a caller
    /// that sweeps a hard-coded `1..=4` silently reports a partitioning L5 as
    /// absent -- a wrong answer that looks like a confident one.
    pub fn cache_levels(&self) -> Vec<u8> {
        let mut levels: Vec<u8> = self
            .caches()
            .filter_map(|d| match &d.kind {
                DomainKind::Cache { level, .. } => Some(*level),
                _ => None,
            })
            .collect();
        levels.sort_unstable();
        levels.dedup();
        levels
    }

    /// The distinct processor partitions the caches at `level` form.
    ///
    /// # Why this is not [`Self::caches_at_level`]
    ///
    /// Windows reports one relationship per *cache*, not per partition, and a
    /// level is routinely reported more than once over the very same
    /// processors. Measured on the eight-core development host rather than
    /// reasoned about: L1 arrives as eight `data` domains **plus** eight
    /// `instruction` domains covering exactly the same eight processor pairs.
    ///
    /// Counting relationships therefore claims sixteen L1 partitions where the
    /// machine has eight. Two consequences, both silent: a partition count that
    /// is a whole multiple too large, and -- on a machine whose L1i and L1d are
    /// its only two cache domains -- a level reported as *partitioning* when it
    /// divides nothing at all.
    ///
    /// Deduplication is by processor set, which is the thing a caller
    /// partitioning work actually cares about; the first domain covering each
    /// distinct set is kept, so the returned ids are stable for a topology.
    ///
    /// **Distinct is not disjoint.** Two sets that overlap without being equal
    /// both survive this, so the result is a set of domains rather than a
    /// proven partition. [`Self::outermost_partitioning_cache`] is where that
    /// stronger property is required and checked.
    ///
    /// **A domain covering no processors is dropped**, because it partitions
    /// nothing and every consumer of this list wants pieces of the machine. It
    /// would otherwise survive both filters here and in
    /// [`Self::outermost_partitioning_cache`]: it is not *equal* to any
    /// non-empty set, so deduplication keeps it, and it is disjoint from
    /// everything vacuously, so the pairwise check passes it. A level with one
    /// real cache plus one empty domain would then count two partitions and be
    /// reported as dividing a machine it does not divide. Contrast
    /// [`Self::memory_domains`], which deliberately keeps a processor-less
    /// domain because a memory domain with no CPUs is real hardware (D-5); a
    /// *cache* over no processors is not.
    pub fn cache_partitions_at_level(&self, level: u8) -> Vec<&Domain> {
        let mut partitions: Vec<&Domain> = Vec::new();
        for domain in self
            .caches_at_level(level)
            .filter(|domain| !domain.processors.is_empty())
        {
            if !partitions
                .iter()
                .any(|kept| kept.processors == domain.processors)
            {
                partitions.push(domain);
            }
        }
        partitions
    }

    /// The outermost cache level that actually divides this machine, together
    /// with the distinct partitions it forms.
    ///
    /// A level whose caches all cover the same processors partitions nothing --
    /// a fully shared L3 is one domain spanning everything -- so it is never a
    /// candidate however far out it sits. `None` means no reported level
    /// divides the machine, which is a real answer and not a failure: a
    /// single-core host is one partition by every measure.
    ///
    /// This is deliberately not "level 3". A shipping ARM64 laptop measured
    /// during the 2026-08-30 session reports **no L3 at all**, with two L2
    /// domains of six processors forming the real cluster boundary.
    ///
    /// # A level must be a partition, not merely a set of domains
    ///
    /// [`Self::cache_partitions_at_level`] deduplicates by equal processor set,
    /// which is what the measured case needs (L1i and L1d cover identical
    /// sets). That alone does **not** make the result a partition: two distinct
    /// sets can still overlap. Real hardware does not do this, but a
    /// `MachineMemoryTopology` is deliberately constructible by hand and by deserialization
    /// (see [`Provenance`](crate::Provenance)), so this method cannot assume
    /// hardware produced it -- and a caller splitting work across overlapping
    /// "partitions" double-counts the processors in the intersection and
    /// overwrites their domain assignment, silently.
    ///
    /// So a level qualifies only when its distinct sets are **pairwise
    /// disjoint**. Full coverage of the online processors is deliberately *not*
    /// required: a processor with no cache reported at a level is a gap in what
    /// the firmware said, not evidence that the domains which *were* reported
    /// overlap, and rejecting the level would discard a true boundary over it.
    ///
    /// Defined here, in the crate that owns the topology, so that every
    /// consumer asks the same question rather than restating the rule and
    /// drifting from it.
    pub fn outermost_partitioning_cache(&self) -> Option<(u8, Vec<&Domain>)> {
        self.cache_levels().into_iter().rev().find_map(|level| {
            let partitions = self.cache_partitions_at_level(level);
            (partitions.len() > 1 && Self::are_pairwise_disjoint(&partitions))
                .then_some((level, partitions))
        })
    }

    /// Whether no two of these domains claim the same processor.
    ///
    /// Quadratic on purpose: the input is one cache level's distinct domains,
    /// which is a handful even on a large machine, and pairwise intersection
    /// asks the question directly rather than through a set type this crate
    /// would otherwise not need.
    fn are_pairwise_disjoint(domains: &[&Domain]) -> bool {
        domains.iter().enumerate().all(|(i, left)| {
            domains[i + 1..]
                .iter()
                .all(|right| left.processors.is_disjoint(&right.processors))
        })
    }

    /// Every memory domain, including one with no processors (D-5).
    pub fn memory_domains(&self) -> impl Iterator<Item = &Domain> {
        self.domains
            .iter()
            .filter(|d| matches!(d.kind, DomainKind::Memory { .. }))
    }
}

#[cfg(test)]
mod tests;
