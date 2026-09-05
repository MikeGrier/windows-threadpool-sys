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

/// What `--placement remote` can actually be given on this topology.
///
/// Three answers rather than `Option<u32>`, because the two ways of having no
/// remote node need opposite handling and an `Option` cannot tell them apart.
/// Conflating them is what let a restored topology silently produce a *local*
/// measurement while the caller had asked for a remote one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteNode {
    /// A NUMA node other than the local one. The switch does what it says.
    Other(u32),
    /// The memory domains name their nodes, and there is only the local one --
    /// an ordinary single-node machine. Falling back to local is the honest
    /// answer here, because no other node exists to place on.
    SameAsLocal,
    /// No memory domain carries an operational node number, so which nodes
    /// exist cannot be determined -- let alone which is remote.
    ///
    /// This is what a **restored** topology gives. `Domain::deserialize`
    /// deliberately leaves `observations` empty (D-12/D-22: a file cannot
    /// establish that the relationship walk saw anything), and the node number
    /// is carried by a `Source::RelationshipWalk` observation -- so
    /// `label_from` answers `None` for every domain in a description, however
    /// many nodes that description describes.
    Unnamed,
    /// The machine names its nodes, but *this domain's own* node is unknown, so
    /// there is nothing for a candidate to be remote from.
    ///
    /// Distinct from [`RemoteNode::Unnamed`], which is about the machine, where
    /// this is about one domain. Both refuse, but conflating them would hide
    /// that a partially-named topology is a different situation from an
    /// entirely unnamed one.
    LocalUnknown,
}

/// A NUMA node other than `local`, for the sample's `--placement remote`
/// switch -- deliberately the wrong node, so the buffer-placement effect
/// (M7.4) is measurable rather than assumed.
///
/// That purpose is why [`RemoteNode::Unnamed`] must not degrade to the local
/// node: a run that measures local placement while reporting itself as remote
/// would show no placement effect, and the reader would conclude there is
/// none. A refused run says less than a wrong one, and says it honestly.
/// Whether any memory domain carries an operational node number at all.
///
/// The *machine-level* question, kept separate from [`remote_numa_node`]'s
/// per-domain one. Asking the latter with `local = None` used to serve as this,
/// and stopped working the moment an unknown local node became its own answer
/// -- the global check then refused every run, including on a live machine.
/// Two questions, two functions.
#[must_use]
pub fn names_any_numa_node(topology: &MachineMemoryTopology) -> bool {
    topology.domains.iter().any(|domain| {
        matches!(domain.kind, DomainKind::Memory { .. })
            && domain.label_from(Source::RelationshipWalk).is_some()
    })
}

pub fn remote_numa_node(topology: &MachineMemoryTopology, local: Option<u32>) -> RemoteNode {
    // **Remoteness is a relationship, so it needs both ends.** Without a local
    // node there is nothing for a candidate to be remote *from*: the comparison
    // below is `Some(id) != local`, and against `None` that is true for every
    // named node, so the first one would be returned as remote on no evidence
    // at all. That is the same unknown-promoted-to-a-finding this function was
    // rewritten to stop doing at the other end. Raised in the PR #56 review.
    if local.is_none() {
        return RemoteNode::LocalUnknown;
    }
    let mut any_named = false;
    for domain in &topology.domains {
        if !matches!(domain.kind, DomainKind::Memory { .. }) {
            continue;
        }
        let Some(id) = domain.label_from(Source::RelationshipWalk) else {
            continue;
        };
        any_named = true;
        if Some(id) != local {
            return RemoteNode::Other(id);
        }
    }
    if any_named {
        RemoteNode::SameAsLocal
    } else {
        RemoteNode::Unnamed
    }
}
