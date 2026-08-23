// Copyright (c) 2026 Mike Grier
//! Faithful, owned records for each processor relationship Windows reports.
//!
//! "Faithful" means each record says what Win32 said, with a [`ProcessorSet`]
//! in place of a raw mask and no interpretation layered on top of it (D-2 in
//! `DESIGN-NOTES.md`).

use std::io;

use crate::processor_set::ProcessorSet;
use crate::walk::{self, GroupAffinity, Record};

/// The Win32 `LTP_PC_SMT` flag on `PROCESSOR_RELATIONSHIP::Flags`.
///
/// `windows-sys` does not export this constant, so it is named here rather
/// than written as a bare literal at the call site.
const LTP_PC_SMT: u8 = 0x1;

fn to_processor_set(masks: &[GroupAffinity]) -> ProcessorSet {
    let mut set = ProcessorSet::empty();
    for m in masks {
        set = set.union(&ProcessorSet::from_group_mask(m.group, m.mask));
    }
    set
}

/// A processor core: one or more logical processors sharing a physical core
/// as SMT siblings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoreRelation {
    /// Whether this core has more than one logical processor (SMT /
    /// hyperthreading).
    pub simultaneous_multithreading: bool,
    /// The scheduler's efficiency class for this core. Windows documents only
    /// that a higher value means a more efficient (and typically lower
    /// performance) core on a hybrid part; it does not fix an absolute scale.
    pub efficiency_class: u8,
    /// The logical processors that make up this core.
    pub processors: ProcessorSet,
}

/// A physical processor package (socket).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageRelation {
    /// The logical processors in this package.
    pub processors: ProcessorSet,
}

/// What a [`CacheRelation`] holds.
///
/// With the `serde` feature, a well-known variant serializes as its lowercase
/// name (`"unified"`, `"instruction"`, `"data"`, `"trace"`); `Other` serializes
/// as `{"other": <raw value>}`, so an unrecognised `PROCESSOR_CACHE_TYPE`
/// still round-trips rather than losing its value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
#[non_exhaustive]
pub enum CacheKind {
    /// A unified instruction-and-data cache.
    Unified,
    /// An instruction-only cache.
    Instruction,
    /// A data-only cache.
    Data,
    /// A trace cache.
    Trace,
    /// A cache type this crate does not have a name for; carries Windows's
    /// raw `PROCESSOR_CACHE_TYPE` value rather than losing it.
    Other(i32),
}

/// A cache level shared by a set of logical processors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheRelation {
    /// Cache level (1, 2, 3, ...).
    pub level: u8,
    /// Associativity Windows reported, or `0xFF` for fully associative.
    pub associativity: u8,
    /// Cache line size in bytes.
    pub line_size: u16,
    /// Total cache size in bytes.
    pub cache_size: u32,
    /// What the cache holds.
    pub cache_type: CacheKind,
    /// The logical processors that share this cache.
    pub processors: ProcessorSet,
}

/// A NUMA node, as Windows reports it: a set of logical processors.
///
/// Windows does not report a memory-only NUMA node through this API -- every
/// node it returns here has at least one processor. This crate's own
/// `Domain` type, built on top of these records, does not assume that; see
/// `windows-topology-sys`'s D-5 for why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NumaNodeRelation {
    /// The NUMA node number.
    pub node_number: u32,
    /// The logical processors in this node.
    pub processors: ProcessorSet,
}

/// One processor group Windows has created.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupRelation {
    /// The group number. Windows documents `GroupInfo[i]` as describing group
    /// `i`, which is where this comes from.
    pub group: u16,
    /// The maximum number of processors this group could ever hold.
    pub maximum_processor_count: u8,
    /// The number of processors currently active in this group.
    pub active_processor_count: u8,
    /// The processors currently active in this group.
    pub active_processors: ProcessorSet,
}

/// Every relationship Windows reports, decoded and owned.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Relations {
    /// Every processor core.
    pub cores: Vec<CoreRelation>,
    /// Every processor package.
    pub packages: Vec<PackageRelation>,
    /// Every processor die, on systems that report the distinction from
    /// package. Empty on a system that does not.
    pub dies: Vec<PackageRelation>,
    /// Every processor module, on systems that report it. Empty on a system
    /// that does not.
    pub modules: Vec<PackageRelation>,
    /// Every cache level.
    pub caches: Vec<CacheRelation>,
    /// Every NUMA node.
    pub numa_nodes: Vec<NumaNodeRelation>,
    /// Every processor group.
    pub groups: Vec<GroupRelation>,
}

fn core_from(body: walk::ProcessorBody) -> CoreRelation {
    CoreRelation {
        simultaneous_multithreading: body.flags & LTP_PC_SMT != 0,
        efficiency_class: body.efficiency_class,
        processors: to_processor_set(&body.group_masks),
    }
}

fn package_from(body: walk::ProcessorBody) -> PackageRelation {
    PackageRelation {
        processors: to_processor_set(&body.group_masks),
    }
}

fn cache_from(body: walk::CacheBody) -> CacheRelation {
    CacheRelation {
        level: body.level,
        associativity: body.associativity,
        line_size: body.line_size,
        cache_size: body.cache_size,
        cache_type: walk::cache_kind_from_raw(body.cache_type),
        processors: to_processor_set(&body.group_masks),
    }
}

fn numa_from(body: walk::NumaNodeBody) -> NumaNodeRelation {
    NumaNodeRelation {
        node_number: body.node_number,
        processors: to_processor_set(&body.group_masks),
    }
}

fn groups_from(body: walk::GroupBody) -> Vec<GroupRelation> {
    body.group_info
        .into_iter()
        .enumerate()
        .map(|(index, info)| GroupRelation {
            group: index as u16,
            maximum_processor_count: info.maximum_processor_count,
            active_processor_count: info.active_processor_count,
            active_processors: ProcessorSet::from_group_mask(
                index as u16,
                info.active_processor_mask,
            ),
        })
        .collect()
}

/// Enumerate every processor, package, cache, NUMA node, and group relation
/// the system reports.
///
/// # Errors
///
/// Returns any error from the underlying `GetLogicalProcessorInformationEx`
/// call.
pub fn discover() -> io::Result<Relations> {
    let mut relations = Relations::default();
    for record in walk::enumerate()? {
        match record {
            Record::ProcessorCore(body) => relations.cores.push(core_from(body)),
            Record::ProcessorPackage(body) => relations.packages.push(package_from(body)),
            Record::ProcessorDie(body) => relations.dies.push(package_from(body)),
            Record::ProcessorModule(body) => relations.modules.push(package_from(body)),
            Record::Cache(body) => relations.caches.push(cache_from(body)),
            Record::NumaNode(body) => relations.numa_nodes.push(numa_from(body)),
            Record::Group(body) => relations.groups.extend(groups_from(body)),
            Record::Unknown(_) => {}
        }
    }
    Ok(relations)
}

#[cfg(test)]
mod tests;
