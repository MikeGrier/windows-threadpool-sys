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

use std::ffi::c_void;
use std::io;
use std::os::windows::io::RawHandle;

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Ioctl::FSCTL_QUERY_VOLUME_NUMA_INFO;
use windows_sys::Win32::System::Threading::GetNumaHighestNodeNumber;

/// What the arena's placement was decided to be, kept so the sample can report
/// it rather than making a locality choice silently.
pub enum Placement {
    /// The log file's volume named a node, and the arena prefers it.
    OnVolumeNode {
        /// The node `FSCTL_QUERY_VOLUME_NUMA_INFO` reported.
        node: u32,
        /// The highest node number this machine reports, when it would say.
        /// Kept because `node` alone cannot distinguish a real placement
        /// decision from the only answer a single-node machine can give.
        highest_node: Option<u32>,
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

    /// The node to hand [`NumaBuffer::new`](windows_ioring_sys::NumaBuffer::new), or `None` for no preference.
    pub fn node(&self) -> Option<u32> {
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
                highest_node: Some(0),
            } => format!(
                "arena placed on NUMA node {node}, which the log file's volume reports; this \
                 machine has one node, so that is the only answer available and the placement \
                 changes nothing here"
            ),
            Self::OnVolumeNode {
                node,
                highest_node: Some(highest),
            } => format!(
                "arena placed on NUMA node {node}, which the log file's volume reports, out of \
                 nodes 0..={highest}"
            ),
            Self::OnVolumeNode {
                node,
                highest_node: None,
            } => format!(
                "arena placed on NUMA node {node}, which the log file's volume reports; how many \
                 nodes this machine has could not be determined"
            ),
            Self::Unplaced { reason } => format!(
                "arena allocated with no NUMA preference: the log file's volume does not report a \
                 node ({reason})"
            ),
        }
    }
}

/// The NUMA node `handle`'s volume reports, if it reports one.
///
/// # Why this calls `DeviceIoControl` directly
///
/// This sample already depends on `windows-overlapped-io-sys` and already
/// issues two other `FSCTL`s through its typed adapter --
/// [`reclaim.rs`](../reclaim.rs) uses `BlockingEndpoint::ioctl` for
/// `FSCTL_SET_SPARSE` and `FSCTL_SET_ZERO_DATA`. Using the raw call *here* and
/// the adapter *there* looks like an inconsistency, and the note exists because
/// it was read as one: the two sites differ in their handles, not in their care.
///
/// The adapter is unavailable to this function for two independent reasons,
/// either of which alone would be enough:
///
/// - **The handle is borrowed.** `decide` takes a `RawHandle` that the log's
///   `File` owns. `UnassociatedEndpoint::assume_overlapped` takes an
///   `OwnedHandle`, so routing through it would transfer ownership and close
///   the log's handle out from under it.
/// - **The handle is synchronous.** The log is opened with a plain
///   `OpenOptions` and carries no `FILE_FLAG_OVERLAPPED`; `BlockingEndpoint`
///   issues an *overlapped* `DeviceIoControl`, and `assume_overlapped`'s safety
///   contract requires the handle actually be overlapped. Calling it here would
///   be unsound, not merely awkward.
///
/// `reclaim.rs` meets neither constraint because it opens its own handle with
/// `UnassociatedEndpoint::open`, which always sets `FILE_FLAG_OVERLAPPED` and
/// yields an owned endpoint. That is a second open, which this function
/// deliberately avoids -- the documented `FSCTL` accepts the file handle the log
/// already holds.
///
/// **`M25.3` lifts the second reason** by opening the log
/// `NO_BUFFERING | OVERLAPPED`. The first still stands, so revisit this then
/// rather than assuming it resolves itself.
fn volume_numa_node(handle: RawHandle) -> io::Result<u32> {
    let mut node: u32 = u32::MAX;
    let mut returned: u32 = 0;
    // SAFETY: `handle` is the log's own open file handle, which outlives this
    // call. The FSCTL takes no input buffer, and its output is documented as
    // `FSCTL_QUERY_VOLUME_NUMA_INFO_OUTPUT { ULONG NumaNode }` -- one `u32`,
    // which is what `node` provides and what the size argument reports.
    let ok = unsafe {
        DeviceIoControl(
            handle as HANDLE,
            FSCTL_QUERY_VOLUME_NUMA_INFO,
            std::ptr::null(),
            0,
            (&raw mut node).cast::<c_void>(),
            u32::try_from(size_of::<u32>()).expect("four fits in a u32"),
            &raw mut returned,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    if returned as usize != size_of::<u32>() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "FSCTL_QUERY_VOLUME_NUMA_INFO returned {returned} bytes, not {}",
                size_of::<u32>()
            ),
        ));
    }
    Ok(node)
}

/// The highest NUMA node number this machine reports, if it answers.
///
/// Used only to qualify the report line: a machine that answers zero has one
/// node, and on such a machine no placement decision is distinguishable from
/// any other.
fn highest_numa_node() -> Option<u32> {
    let mut highest: u32 = 0;
    // SAFETY: the out parameter is a live local for the call's duration.
    let ok = unsafe { GetNumaHighestNodeNumber(&raw mut highest) };
    (ok != 0).then_some(highest)
}

#[cfg(test)]
mod tests;
