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
//! - **[`MachineMemoryTopology`]**, [`Domain`], and friends are plain data. They need no
//!   Windows *API call* to construct: build one by hand, or (with the `serde`
//!   feature) deserialize one from JSON written for a machine you do not
//!   have. See [`examples/print_topology.rs`] for the shape a description
//!   takes.
//!
//!   That is a claim about not calling the platform, **not** about other
//!   platforms: this crate is Windows-only and does not build elsewhere. An
//!   earlier version of these docs said it degraded to an empty shell on other
//!   targets, which was never true and never built in CI -- raised in PR #56
//!   review. That correction reached the prose but not the code, leaving two
//!   modules ungated and the crate genuinely buildable off Windows with a
//!   two-type API; a `compile_error!` now enforces the claim -- raised in
//!   PR #61 review. Depend on it under
//!   `[target.'cfg(windows)'.dependencies]`.
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
//! **Windows 11 / Windows Server 2025 and later.**
//!
//! That is the floor this crate claims, and it is a claim about what is
//! *tested* rather than the oldest version the APIs might work on.
//! `GetLogicalProcessorInformationEx` is documented back to Vista, and an
//! earlier version of this section said so -- but [`MachineMemoryTopology::discover`]
//! also calls `GetSystemCpuSetInformation`, which is documented only from
//! Windows 10 / Server 2016, and this crate imports it statically. A down-level
//! system would therefore fail to *load* the process, not merely get a poorer
//! answer. Raised in PR #56 review.
//!
//! The floor is stated at Windows 11 / Server 2025 rather than at the older
//! version the imports would technically permit, because nothing below that is
//! tested here and an untested floor is a guess presented as a guarantee.
//! Server 2025 is the server release built on the Windows 11 codebase; Server
//! 2022 is not, despite the adjacent version numbers.
//!
//! Nothing here is gated on a runtime capability probe the way
//! `windows-ioring-sys` needs one.
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

// The docs above claim this crate does not build off Windows. Until now that
// was prose only: every module was `cfg(windows)` except `observed` and
// `provenance`, so the crate really did build for a Linux target and hand back
// a two-type API. Confirmed by building it, not by reading it.
//
// Enforced here rather than by gating those two modules, because gating alone
// would leave a crate that compiles to nothing on other targets -- which is
// the "empty shell" description a PR #56 review round already rejected as
// false. A crate that will not build is the claim actually being made, so the
// compiler should be the one making it. Raised in the PR #61 review.
#[cfg(not(windows))]
compile_error!(
    "windows-topology-sys is Windows-only: it enumerates Windows processor, cache, and \
     memory topology and has no meaning on another platform. Gate it in your manifest \
     with [target.'cfg(windows)'.dependencies]."
);

#[cfg(windows)]
mod anomaly;

#[cfg(windows)]
mod cpu_set;
#[cfg(windows)]
mod domain;
#[cfg(windows)]
mod granularity;
#[cfg(windows)]
mod observation;
/// Absence with its reason attached.
#[cfg(windows)]
mod observed;
#[cfg(windows)]
mod processor_set;
/// Where a topology's content came from.
#[cfg(windows)]
mod provenance;

#[cfg(windows)]
mod records;
#[cfg(windows)]
mod relation;
#[cfg(windows)]
mod topology;
#[cfg(windows)]
mod walk;

#[cfg(windows)]
pub use anomaly::{AnomalyKind, EnumerationAnomaly};
#[cfg(windows)]
pub use cpu_set::CpuSet;
#[cfg(windows)]
pub use domain::{AttributeValue, Domain, DomainKind, Processor, ProcessorFacts, ProcessorId};
#[cfg(windows)]
pub use granularity::{Granularity, Proximity};
#[cfg(windows)]
pub use observation::{AttributeObservation, Observation, ProcessorAttribute, Source};
#[cfg(windows)]
pub use observed::Observed;
#[cfg(windows)]
pub use processor_set::ProcessorSet;
#[cfg(windows)]
pub use provenance::Provenance;
#[cfg(windows)]
pub use relation::{
    CacheKind, CacheRelation, CoreRelation, GroupRelation, NumaNodeRelation, PackageRelation,
    Relations, discover,
};
#[cfg(windows)]
pub use topology::{Coherence, MachineMemoryTopology};

// The crate's markdown documentation is compiled as doctests, so an example that
// a contract change invalidates breaks the build instead of quietly teaching the
// old answer. `cfg(doctest)` means these items exist only while rustdoc collects
// tests, so they cost an ordinary build nothing.
#[cfg(all(doctest, windows))]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
