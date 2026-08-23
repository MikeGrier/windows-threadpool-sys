// Copyright (c) 2026 Mike Grier
//! The topology description: an open-kinded domain over a set of processors.
//!
//! This is the "interpretation" layer built on top of `relation.rs`'s
//! faithful, Win32-shaped records (D-2 in `DESIGN-NOTES.md`). Everything here
//! is plain data with no Win32 dependency, so it can be built either by
//! discovering the running system or entirely by hand -- or fed in from
//! elsewhere, which is this crate's whole reason for separating discovery
//! from description.

use std::collections::BTreeMap;

use crate::CacheKind;
use crate::processor_set::ProcessorSet;

/// The identity of one logical processor: its group and its number within
/// that group.
///
/// Never flattened to a single index (D-7 in `DESIGN-NOTES.md`): a Windows
/// thread's affinity is a `GROUP_AFFINITY`, one group and a bitmask within
/// it, so the group is a hard boundary a flattened index would lose.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProcessorId {
    /// The processor group.
    pub group: u16,
    /// The processor's number within that group (0..64).
    pub number: u8,
}

/// One logical processor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Processor {
    /// This processor's identity.
    pub id: ProcessorId,
    /// Whether the processor is currently active. A processor slot can exist
    /// -- count toward a group's maximum -- without being online, since
    /// Windows reserves group capacity for processors that may be added
    /// later.
    pub online: bool,
    /// A relative scheduling weight for this processor, higher meaning more
    /// capable, on no fixed scale. On Windows this is the owning core's raw
    /// `EfficiencyClass`; `0` for an offline processor or one with no known
    /// owning core.
    pub capacity: u32,
}

/// What a [`Domain`] represents.
///
/// Open-kinded rather than a fixed enumeration (D-4 in `DESIGN-NOTES.md`):
/// Linux alone models `die`, `cluster`, and (on `s390x`) `book` and `drawer`,
/// none of which Windows reports and none of which a cache domain reliably
/// approximates. Enumerating every level any architecture will ever have is a
/// losing game, so an unrecognised kind is carried in [`DomainKind::Other`]
/// rather than rejected.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum DomainKind {
    /// A Windows processor group.
    Group,
    /// A physical processor package (socket).
    Package,
    /// A processor die, on systems that report the distinction from package.
    Die,
    /// A processor module, on systems that report it.
    Module,
    /// A physical core: one or more logical processors as SMT siblings.
    Core {
        /// Whether this core has more than one logical processor.
        simultaneous_multithreading: bool,
        /// The scheduler's raw Windows efficiency class for this core.
        efficiency_class: u8,
    },
    /// A cache level shared by a set of logical processors.
    Cache {
        /// Cache level (1, 2, 3, ...).
        level: u8,
        /// Associativity Windows reported, or `0xFF` for fully associative.
        associativity: u8,
        /// Cache line size in bytes.
        line_size: u16,
        /// Total cache size in bytes.
        size_bytes: u32,
        /// What the cache holds.
        cache_type: CacheKind,
    },
    /// A memory locality domain -- a NUMA node modelled as a memory domain
    /// that may contain no processors at all (D-5), because CXL expanders,
    /// persistent memory, HBM tiers, and coherent GPU memory all present that
    /// way. `memory_bytes` is `None` when the size is not known: Windows's
    /// own enumeration (`GetLogicalProcessorInformationEx`) does not report a
    /// NUMA node's capacity at all, so a domain discovered by this crate
    /// always has `memory_bytes: None`; a hand-written or fed-in description
    /// may supply it.
    Memory {
        /// The domain's memory capacity, if known.
        memory_bytes: Option<u64>,
    },
    /// A domain kind this crate does not have a name for, carrying its raw
    /// name and whatever attributes came with it, so a description this
    /// crate cannot fully interpret still round-trips losslessly.
    Other {
        /// The raw kind name.
        name: String,
        /// Whatever attributes accompanied this domain, beyond `id` and
        /// `processors`.
        attributes: BTreeMap<String, AttributeValue>,
    },
}

/// A plain value for an unrecognised [`DomainKind::Other`]'s attributes.
///
/// Deliberately not `serde_json::Value`: this crate depends only on `serde`'s
/// traits, optionally, following `windows-file-watcher`'s D-72 precedent --
/// never on a specific format crate. A consumer chooses their own serializer.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum AttributeValue {
    /// A JSON `null`.
    Null,
    /// A JSON boolean.
    Bool(bool),
    /// A JSON number, represented as `f64` regardless of source precision.
    Number(f64),
    /// A JSON string.
    String(String),
    /// A JSON array.
    Array(Vec<AttributeValue>),
    /// A JSON object.
    Object(BTreeMap<String, AttributeValue>),
}

/// One domain: a named relationship over a set of processors.
///
/// Domains reference processors; they do not nest (D-6 in `DESIGN-NOTES.md`).
/// No hierarchy is imposed, so this type never asserts that a package
/// contains a node or that a node contains a cache -- chiplets and CXL
/// already violate assumptions like that, and Linux's own levels do not form
/// a strict hierarchy either.
#[derive(Clone, Debug, PartialEq)]
pub struct Domain {
    /// What this domain represents.
    pub kind: DomainKind,
    /// An identifier for this domain, unique among domains of the same
    /// `kind`. Where Windows reports a natural number (a NUMA node number, a
    /// group number) that number is used; otherwise domains are numbered in
    /// the order they were discovered.
    pub id: u32,
    /// The logical processors this domain covers. Empty for a memory-only
    /// domain (D-5).
    pub processors: ProcessorSet,
}

/// A scalar relative-distance matrix over one domain kind.
///
/// Deliberately not the HMAT attributed-relation model (per-initiator,
/// per-target read/write latency and bandwidth): that was considered and
/// declined for now, see D-9 in `DESIGN-NOTES.md`. Windows exposes no
/// user-mode SLIT reader, so a [`crate::Topology`] this crate discovers never
/// populates this; it exists for a fed-in description sourced from a system
/// that does report it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Distances {
    /// Which domain kind the matrix's rows and columns index. Kept as a
    /// plain string, naming a [`DomainKind`] the way its JSON `kind` tag will
    /// read once this crate's `serde` feature exists (M3), because domain
    /// kinds are themselves open (D-4).
    pub over: String,
    /// The distance matrix, in the order those domains appear in
    /// [`crate::Topology::domains`] filtered to `over`. Square;
    /// `matrix[i][i]` is conventionally `10`, Windows's and ACPI SLIT's own
    /// "local" value.
    pub matrix: Vec<Vec<u32>>,
}

#[cfg(test)]
mod tests;
