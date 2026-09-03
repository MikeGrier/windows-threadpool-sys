// Copyright (c) 2026 Mike Grier
//! Domain -> ring plan (M7.1/M7.2): the platform-level affinity and NUMA
//! placement each selected domain gets, or why it cannot be expressed.

use std::io;

use windows_topology_sys::{Domain, DomainKind, MachineMemoryTopology, ProcessorSet, Source};

/// What one execution domain needs to run: a single-group affinity mask and,
/// if known, the NUMA node its registered buffer should prefer.
#[derive(Clone, Debug)]
pub struct DomainPlan {
    pub label: String,
    pub group: u16,
    pub mask: usize,
    pub local_numa_node: Option<u32>,
}

/// Build one plan per domain, rejecting any domain the platform cannot
/// express as a single ring's affinity (M7.2, topology D-10).
///
/// # Errors
///
/// Returns an error naming the offending domain if its processors span more
/// than one Windows processor group -- Windows affinity is a single
/// `GROUP_AFFINITY`, one group and one mask, so such a domain has no
/// representable plan. This does not silently narrow it to a subset; the
/// fed-in (or discovered) topology described something the platform cannot
/// do, and that is reported rather than papered over.
pub fn build_plan(
    topology: &MachineMemoryTopology,
    domains: &[Domain],
) -> io::Result<Vec<DomainPlan>> {
    domains
        .iter()
        .enumerate()
        .map(|(index, domain)| {
            let label = label_for(domain);
            let mut groups = domain.processors.group_masks();
            let first = groups.next();
            let second = groups.next();
            let (group, mask) = match (first, second) {
                (Some(only), None) => only,
                (None, _) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("domain {index} ({label}) names no processors"),
                    ));
                }
                (Some(_), Some(_)) => {
                    let group_count = domain.processors.group_masks().count();
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!(
                            "domain {index} ({label}) spans {group_count} processor groups; \
                             Windows affinity is a single GROUP_AFFINITY, so this domain has no \
                             representable plan (M7.2, topology D-10) -- pick a narrower policy"
                        ),
                    ));
                }
            };
            Ok(DomainPlan {
                label,
                group,
                mask,
                local_numa_node: numa_node_for(topology, &domain.processors),
            })
        })
        .collect()
}

fn label_for(domain: &Domain) -> String {
    // The relationship walk's label, which is what the NUMA node number and
    // the group number are. A relation has no single "id" now that two sources
    // may label it differently, so the source is named rather than assumed.
    let id = domain
        .label_from(Source::RelationshipWalk)
        .map_or_else(|| "?".to_string(), |label| label.to_string());
    match &domain.kind {
        DomainKind::Cache { level, .. } => format!("L{level} cache #{}", id),
        DomainKind::Memory { .. } => format!("NUMA node {}", id),
        DomainKind::Package => format!("package #{}", id),
        DomainKind::Core { .. } => format!("core #{}", id),
        DomainKind::Group => format!("group #{}", id),
        DomainKind::Die => format!("die #{}", id),
        DomainKind::Module => format!("module #{}", id),
        DomainKind::Other { name, .. } => name.clone(),
        // `DomainKind` is `#[non_exhaustive]`: a future variant this sample
        // does not yet know falls back to a generic label rather than
        // failing to build.
        _ => format!("domain #{}", id),
    }
}

/// The NUMA node whose processors overlap `processors`, if any domain
/// reports one -- `None` on a machine that reports no NUMA nodes at all.
fn numa_node_for(topology: &MachineMemoryTopology, processors: &ProcessorSet) -> Option<u32> {
    topology
        .domains
        .iter()
        .find_map(|domain| match domain.kind {
            DomainKind::Memory { .. } if !domain.processors.is_disjoint(processors) => {
                domain.label_from(Source::RelationshipWalk)
            }
            _ => None,
        })
}

/// A NUMA node other than `local`, for the sample's `--placement remote`
/// switch -- deliberately the wrong node, so the buffer-placement effect
/// (M7.4) is measurable rather than assumed.
pub fn remote_numa_node(topology: &MachineMemoryTopology, local: Option<u32>) -> Option<u32> {
    topology
        .domains
        .iter()
        .filter_map(|domain| match domain.kind {
            DomainKind::Memory { .. } => domain.label_from(Source::RelationshipWalk),
            _ => None,
        })
        .find(|&id| Some(id) != local)
}
