// Copyright (c) 2026 Mike Grier
//! Memory-safe Rust over the Windows NUMA APIs.
//!
//! Windows provides some NUMA concepts; this crate builds library notions on
//! top of them. Whether a client uses them is up to the client.
//!
//! # What is here
//!
//! - [`NumaNode`] -- a node number, as Windows numbers them.
//! - [`NumaBuffer`] -- an owned allocation made with `VirtualAllocExNuma` and
//!   released with `VirtualFree`, so a buffer can be placed on a chosen node
//!   rather than wherever the default allocator lands it.
//! - [`highest_numa_node`] and [`volume_numa_node`] -- the two questions
//!   Windows will answer about nodes, wrapped so a caller can find a node to
//!   pass without writing the FFI themselves.
//!
//! # What is not here, and will not be
//!
//! **Any opinion about which node anything should use.** This crate reports
//! what Windows says and allocates where it is told. It does not map a file to
//! a node, does not shard anything, and does not choose a node on a caller's
//! behalf. Those are workload decisions, and a consumer on hardware this
//! workspace has never seen is better placed to take them.
//!
//! The `-sys` suffix is that promise, and it is the repository's meaning of the
//! suffix rather than a convention borrowed from elsewhere: thin over Win32,
//! memory-safe, adding no policy.
//!
//! # A node argument is a preference, not an instruction
//!
//! The underlying parameter says so in its own name -- `nndPreferred`. A
//! successful allocation is therefore **not** evidence that the pages landed on
//! the node that was asked for, and code that needs to know must observe rather
//! than assume. Two measured facts about that, both from this workspace:
//!
//! - Committed pages are demand-zero, so until something writes to them no
//!   physical page has been drawn from the preferred node at all. A caller
//!   measuring placement must fault the pages in first.
//! - An *invalid* node is refused, and `u32::MAX` is not a test of that: it is
//!   the API's own no-preference sentinel, accepted by design. A measurement
//!   that asks for `u32::MAX` and sees it succeed has measured the sentinel,
//!   not a range check. [`NumaBuffer`]'s tests use `u32::MAX - 1`.
//!
//! Observing where pages actually landed needs `QueryWorkingSetEx`, which lives
//! in `windows-placement-probe` today and is a candidate to move here; see that
//! crate's `peer_index_cache`.

#![cfg(windows)]
#![deny(missing_docs)]

mod buffer;
mod node;

pub use buffer::NumaBuffer;
pub use node::{NumaNode, highest_numa_node, volume_numa_node};
