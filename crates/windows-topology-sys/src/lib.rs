// Copyright (c) 2026 Mike Grier
//! Safe enumeration of Windows processor, cache, and memory topology, with a
//! JSON-describable schema for discovered or synthetic topologies.
//!
//! # What this crate is
//!
//! Two things, deliberately separated:
//!
//! - **[`MachineMemoryTopology::discover`]** reads the running system's processor groups,
//!   cores, caches, and NUMA nodes safely, via
//!   [`GetLogicalProcessorInformationEx`][gpi].
//! - **[`MachineMemoryTopology`]**, [`Domain`], and friends are plain data. They do not
//!   need Windows to construct: build one by hand, or (with the `serde`
//!   feature) deserialize one from JSON written for a machine you do not
//!   have. See [`examples/print_topology.rs`] for the shape a description
//!   takes.
//!
//! [gpi]: https://learn.microsoft.com/windows/win32/api/sysinfoapi/nf-sysinfoapi-getlogicalprocessorinformationex
//! [`examples/print_topology.rs`]: https://github.com/MikeGrier/windows-threadpool-sys/blob/main/crates/windows-topology-sys/examples/print_topology.rs
//!
//! # What this crate is not
//!
//! **Not a topology renderer.** It hands back faithful records and an
//! open-kinded [`Domain`] over a set of processors; it does not decide what
//! a "locality domain worth partitioning by" is. That decision -- by NUMA
//! node, by last-level cache, by processor package -- belongs to the
//! consumer, because the right answer depends on the workload and on
//! hardware this crate cannot see (see `DESIGN-NOTES.md`'s "Why the NUMA
//! node is the wrong key").
//!
//! **Not a partitioning policy.** This crate never decides how many rings,
//! threads, or buffer pools a consumer should create. It exists to serve
//! [`windows-ioring-sys`](https://docs.rs/windows-ioring-sys)'s locality
//! story without that crate having to own such a policy either.
//!
//! **Not a device topology.** Domains cover processors and memory; nothing
//! here names an NVMe controller, a NIC, or a GPU as a topology participant,
//! and there is no per-initiator, per-target attributed-distance model
//! (HMAT). Both were considered and explicitly declined for now -- see D-9 in
//! `DESIGN-NOTES.md` for the reasoning and what would justify revisiting it.
//! If your question is "which device sits close to which processor," this
//! crate does not answer it.
//!
//! # Availability
//!
//! `GetLogicalProcessorInformationEx` is documented back to Windows Vista /
//! Server 2008, so [`MachineMemoryTopology::discover`] works on every version this
//! repository's shared baseline supports; nothing here is gated on a runtime
//! capability probe the way `windows-ioring-sys` needs one.
//!
//! # The JSON schema is not semver-covered
//!
//! With the `serde` feature, [`MachineMemoryTopology`] and [`Domain`] serialize to and
//! deserialize from a JSON shape documented on [`Domain`] itself. That shape
//! is **not** covered by this crate's semver contract (D-8 in
//! `DESIGN-NOTES.md`), even though the Rust types that produce it are, as
//! always. This is what makes the deferrals in D-9 safe to defer rather than
//! merely convenient: extending the schema later is a schema evolution, not
//! a breaking change to a promise never made.

#![warn(missing_docs)]

#[cfg(windows)]
mod cpu_set;
#[cfg(windows)]
mod domain;
#[cfg(windows)]
mod granularity;
#[cfg(windows)]
mod observation;
/// Absence with its reason attached.
mod observed;
#[cfg(windows)]
mod processor_set;
/// Where a topology's content came from.
mod provenance;
#[cfg(windows)]
mod relation;
#[cfg(windows)]
mod topology;
#[cfg(windows)]
mod walk;

#[cfg(windows)]
pub use cpu_set::CpuSet;
#[cfg(windows)]
pub use domain::{AttributeValue, Domain, DomainKind, Processor, ProcessorFacts, ProcessorId};
#[cfg(windows)]
pub use granularity::{Granularity, Proximity};
#[cfg(windows)]
pub use observation::{AttributeObservation, Observation, ProcessorAttribute, Source};
pub use observed::Observed;
pub use processor_set::ProcessorSet;
pub use provenance::Provenance;
#[cfg(windows)]
pub use relation::{
    CacheKind, CacheRelation, CoreRelation, GroupRelation, NumaNodeRelation, PackageRelation,
    Relations, discover,
};
#[cfg(windows)]
pub use topology::MachineMemoryTopology;
