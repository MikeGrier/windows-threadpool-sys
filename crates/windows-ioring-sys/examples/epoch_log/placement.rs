// Copyright (c) 2026 Mike Grier
//! Where the registered arena is placed, and why (M22.3).
//!
//! # The decision
//!
//! The arena is allocated with [`NumaBuffer`](windows_ioring_sys::NumaBuffer) on the NUMA node the **log
//! file's own volume** reports, and with no preference when the volume reports
//! none. The node is asked of the handle the log already holds, through
//! `FSCTL_QUERY_VOLUME_NUMA_INFO`.
//!
//! This module exists because the alternative was silence. The crate's front
//! page tells every consumer that placing the registered pool near the device
//! "is very likely the highest-leverage locality decision available", and this
//! sample previously allocated its arena as a plain `vec![0u8; SLOT_LEN]` --
//! heap, no alignment, no node. A durability sample is entitled to decide that
//! locality is not its subject; it is not entitled to leave the question
//! looking like an oversight.
//!
//! # What this sample does not claim
//!
//! **That the placement pays here.** It almost certainly does not. The arena
//! is eight slots of four kilobytes, and this workload is bound by a device
//! flush that costs hundreds of microseconds per epoch -- `M22.1` measured
//! that directly, by removing seven of every eight submissions from the append
//! path and watching throughput not move. Thirty-two kilobytes of records
//! crossing an interconnect is not what this program spends its time on, and
//! `examples/ring_copy` is where buffer placement is put under a load that can
//! actually show it. What this sample demonstrates is *how the decision is
//! made and reported*, not that it was worth making.
//!
//! **That the node is the device's.** `FSCTL_QUERY_VOLUME_NUMA_INFO` answers a
//! question about a **volume**, and a volume is not a device: it may span
//! several, as an ordinary spanned volume or a Storage Spaces set does, and
//! what it reports is where the volume resides rather than where this file's
//! extents live. The crate's design notes decline to offer an automatic
//! file-to-node mapping for exactly these reasons, and this module is a
//! sample's local choice rather than a retraction of that.
//!
//! **That the pages landed there.** The allocator's parameter is
//! `nndPreferred`. See [`NumaBuffer`](windows_ioring_sys::NumaBuffer).

use std::io;
use std::os::windows::io::RawHandle;

use win_numa_sys::{NumaNode, highest_numa_node, volume_numa_node};

/// What the arena's placement was decided to be, kept so the sample can report
/// it rather than making a locality choice silently.
pub enum Placement {
    /// The log file's volume named a node, and the arena prefers it.
    OnVolumeNode {
        /// The node `FSCTL_QUERY_VOLUME_NUMA_INFO` reported.
        node: NumaNode,
        /// The highest node number this machine reports, when it would say.
        /// Kept because `node` alone cannot distinguish a real placement
        /// decision from the only answer a single-node machine can give.
        highest_node: Option<NumaNode>,
    },
    /// The volume named no node, so the arena carries no preference -- which
    /// is the same allocation the default heap would have made.
    Unplaced {
        /// Why the query did not answer, so a reader is not left guessing
        /// whether the sample simply did not ask.
        reason: io::Error,
    },
}

impl Placement {
    /// Ask `handle`'s volume which node it is on.
    ///
    /// Takes the log's own file handle: the documented FSCTL accepts a file or
    /// directory handle directly, so this needs no device-tree walk and no
    /// second open.
    pub fn decide(handle: RawHandle) -> Self {
        match volume_numa_node(handle) {
            Ok(node) => Self::OnVolumeNode {
                node,
                highest_node: highest_numa_node(),
            },
            Err(reason) => Self::Unplaced { reason },
        }
    }

    /// The node to hand [`NumaBuffer::new`](win_numa_sys::NumaBuffer::new), or
    /// `None` for no preference.
    pub fn node(&self) -> Option<NumaNode> {
        match self {
            Self::OnVolumeNode { node, .. } => Some(*node),
            Self::Unplaced { .. } => None,
        }
    }

    /// One line for the sample's report, saying what was decided **and** what
    /// that is worth on this machine.
    ///
    /// The second half is the part that matters: on a machine with one node,
    /// placing on node 0 and not placing at all are the same allocation, and a
    /// line that said only "placed on node 0" would read as a locality win
    /// that was never available.
    pub fn describe(&self) -> String {
        match self {
            Self::OnVolumeNode {
                node,
                highest_node: Some(highest),
            } if highest.get() == 0 => format!(
                "arena placed on NUMA {node}, which the log file's volume reports; this \
                 machine has one node, so that is the only answer available and the placement \
                 changes nothing here"
            ),
            Self::OnVolumeNode {
                node,
                highest_node: Some(highest),
            } => format!(
                "arena placed on NUMA {node}, which the log file's volume reports, out of \
                 nodes 0..={}",
                highest.get()
            ),
            Self::OnVolumeNode {
                node,
                highest_node: None,
            } => format!(
                "arena placed on NUMA {node}, which the log file's volume reports; how many \
                 nodes this machine has could not be determined"
            ),
            Self::Unplaced { reason } => format!(
                "arena allocated with no NUMA preference: the log file's volume does not report a \
                 node ({reason})"
            ),
        }
    }
}

#[cfg(test)]
mod tests;
