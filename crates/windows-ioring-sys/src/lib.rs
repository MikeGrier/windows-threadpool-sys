// Copyright (c) 2026 Mike Grier
//! Memory-safe Rust over the Windows `IoRing` submission/completion ring.
//!
//! Windows 11 and Server 2022 added `IoRing`, a submission/completion ring for
//! file I/O closer in shape to `io_uring` than to anything else Windows offers.
//! This crate raises those primitives into safe Rust with the minimum additional
//! CPU and memory cost: a completion hands the caller's buffer back without the
//! crate having allocated anything to track it.
//!
//! # Scope: a file data plane, not a general completion backend
//!
//! The kernel's operation table is fixed at seven entries -- no-op, read, write,
//! flush, register-files, register-buffers, and cancel. There is no ioctl
//! operation, no socket operation, and no directory-change operation, and unlike
//! Linux's `io_uring` -- which grew to roughly fifty opcodes including full
//! socket support -- Windows `IoRing` has not grown beyond file I/O.
//!
//! So this crate is not a replacement for
//! [`windows-overlapped-io-sys`](https://docs.rs/windows-overlapped-io-sys),
//! which remains the crate for arbitrary I/O on arbitrary handles. Use this one
//! for high-volume file reads and writes; use that one for everything else.
//! Neither can subsume the other.
//!
//! # Availability is a runtime question
//!
//! `IoRing` is present from Windows 11 and Server 2022, but three separate facts
//! about it are decided at runtime rather than at compile time: which ring
//! version the system supports, whether the ring is a real kernel ring or a
//! user-mode emulation with no kernel benefit, and whether the completion-event
//! feature the thread-pool delivery path depends on is available at all. All
//! three are answerable without creating a ring.
//!
//! # Choosing a delivery architecture
//!
//! There are two coherent high-performance shapes for consuming completions,
//! and picking the wrong one costs more than any API detail in this crate.
//!
//! **Model A -- shared queue, kernel load-balances.** A pool of threads waits;
//! work is handed to whichever thread the system picks next. This is
//! [`EventDelivery`]: the ring's completion event wired to a thread-pool wait,
//! so no thread of yours ever blocks on I/O. Load balancing is automatic and
//! locality is incidental -- start here. See
//! `examples/model_a_delivery.rs` for a full worked example (M6.2).
//!
//! **Model B -- shared-nothing execution domains.** One pinned thread per
//! domain, owning its ring, its buffer pool, and its shard of the
//! application's state, with no cross-thread synchronization on the data
//! path -- a pinned thread parked directly in [`Batch::submit_and_wait`] *is*
//! the event loop. This is what `IoRing`'s own API is shaped for: the
//! submission queue is not thread-safe, registration is per-ring, and there is
//! exactly one completion event per ring, none of which are limitations to
//! work around.
//!
//! Most real applications want Model B on the hot data path and Model A
//! everywhere else -- the control plane, background work, cold paths -- where
//! the thread pool's quiescence is worth more than locality. Both are
//! first-class here; neither is a degraded form of the other. The full
//! trade-off -- including why the NUMA node is the wrong key for sizing a
//! Model B execution domain, and why buffer placement likely dominates thread
//! placement -- is in "Two delivery architectures" in `DESIGN-NOTES.md`.
//!
//! # Topology guidance
//!
//! This crate does not partition anything for you (D-8 in `DESIGN-NOTES.md`):
//! it makes a ring cheap and correct, makes its affinity explicit, and leaves
//! sizing a Model B execution domain to the caller. Three pointers, not a
//! partitioning policy:
//!
//! - **Size a domain by last-level (L3) cache, not by NUMA node.** Node count
//!   is a firmware setting a process cannot see (AMD NPS, Intel Sub-NUMA
//!   Clustering), and most real deployments are virtualized, where NUMA
//!   topology is often invisible entirely. `GetLogicalProcessorInformationEx`
//!   filtered to `RelationCache` / `CacheLevel == 3` degrades sanely instead:
//!   one reported domain on a VM is correct. See `examples/l3_domains.rs` for
//!   a runnable enumeration (M6.3), built on the safe wrapper in
//!   [`windows-topology-sys`](https://docs.rs/windows-topology-sys).
//! - **Processor groups are a hard floor.** A thread's affinity is a
//!   `GROUP_AFFINITY` and a ring's waiter lives in exactly one group, so above
//!   64 logical processors the partition is forced whether or not it is
//!   wanted.
//! - **Buffer placement likely dominates thread placement.** A registered
//!   buffer on a node remote from the device means every byte crosses the
//!   interconnect, on every operation, forever; where the completion callback
//!   happens to run is a one-time cache-warmth question by comparison.
//!   `VirtualAllocExNuma`, on the node closest to the device, registered once
//!   into that domain's ring (see [`Batch::register_buffers`]), is very
//!   likely the highest-leverage locality decision available.
//!
//! # Status
//!
//! Under construction. The design, including the delivery-architecture guidance
//! this crate exists to make usable, is recorded in `DESIGN-NOTES.md` beside the
//! source; the build-out is tracked in `CHECKLIST.md`.

#![warn(missing_docs)]

#[cfg(windows)]
mod batch;
#[cfg(windows)]
mod buf;
#[cfg(windows)]
mod capability;
#[cfg(windows)]
mod error;
#[cfg(windows)]
mod event_delivery;
#[cfg(windows)]
mod ring;
#[cfg(windows)]
mod token;

#[cfg(windows)]
pub use batch::{
    Batch, FileRef, PendingBufferRegistration, PendingFileRegistration, PushOptions,
    RegisteredBuffers, RegisteredFile, RegisteredFiles, RegisteredSpan, RegisteredUse,
};
#[cfg(windows)]
pub use buf::{IoBuf, IoBufMut};
#[cfg(windows)]
pub use capability::{Capabilities, RingVersion, capabilities};
#[cfg(windows)]
pub use error::IoRingError;
#[cfg(windows)]
pub use event_delivery::EventDelivery;
#[cfg(windows)]
pub use ring::{Completion, IoRing, Op, RingInfo};
#[cfg(windows)]
pub use token::Token;
