// Copyright (c) 2026 Mike Grier
//! Assembling a [`Topology`] from discovered relations.

use std::io;

use crate::domain::{Distances, Domain, DomainKind, Processor, ProcessorId};
use crate::provenance::Provenance;
use crate::relation::{self, Relations};

/// A processor, cache, and memory topology: a set of processors and the
/// domains that relate them.
///
/// Built either by [`Topology::discover`] from the running system, by hand,
/// or (with the `serde` feature) by deserializing a fed-in description.
///
/// The JSON shape this produces and accepts is explicitly not covered by this
/// crate's semver contract -- see [`Domain`]'s documentation and D-8 in
/// `DESIGN-NOTES.md`.
#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Topology {
    /// Every logical processor, including one for each inactive slot up to a
    /// group's maximum processor count.
    pub processors: Vec<Processor>,
    /// Every domain.
    pub domains: Vec<Domain>,
    /// An optional scalar distance matrix.
    pub distances: Option<Distances>,
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

impl Topology {
    /// Discover the running system's topology.
    ///
    /// # Errors
    ///
    /// Returns any error from the underlying `GetLogicalProcessorInformationEx`
    /// call.
    pub fn discover() -> io::Result<Self> {
        let relations = relation::discover()?;
        let mut topology = Self::from_relations(relations);
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
            });
        }
        for (index, package) in relations.packages.iter().enumerate() {
            domains.push(Domain {
                kind: DomainKind::Package,
                id: index as u32,
                processors: package.processors.clone(),
            });
        }
        for (index, die) in relations.dies.iter().enumerate() {
            domains.push(Domain {
                kind: DomainKind::Die,
                id: index as u32,
                processors: die.processors.clone(),
            });
        }
        for (index, module) in relations.modules.iter().enumerate() {
            domains.push(Domain {
                kind: DomainKind::Module,
                id: index as u32,
                processors: module.processors.clone(),
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
            });
        }
        for node in &relations.numa_nodes {
            domains.push(Domain {
                kind: DomainKind::Memory { memory_bytes: None },
                id: node.node_number,
                processors: node.processors.clone(),
            });
        }

        let processors = Self::processors_from(&relations, &domains);
        Self {
            processors,
            domains,
            distances: None,
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

    /// Every memory domain, including one with no processors (D-5).
    pub fn memory_domains(&self) -> impl Iterator<Item = &Domain> {
        self.domains
            .iter()
            .filter(|d| matches!(d.kind, DomainKind::Memory { .. }))
    }
}

#[cfg(test)]
mod tests;
