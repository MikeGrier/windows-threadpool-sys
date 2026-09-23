// Copyright (c) 2026 Mike Grier
//! [`NumaNode`], and the two questions Windows will answer about nodes.

use std::io;
use std::os::windows::io::RawHandle;

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Ioctl::FSCTL_QUERY_VOLUME_NUMA_INFO;
use windows_sys::Win32::System::Threading::GetNumaHighestNodeNumber;

/// A NUMA node number, as Windows numbers them.
///
/// A newtype rather than a bare `u32` because the two `u32`s in this area mean
/// different things and are trivially swapped at a call site: a node *number*
/// and a *count* of nodes are both small integers, and
/// [`highest_numa_node`] returns the former while reading like the latter.
///
/// **Node numbers are not an index.** Windows does not promise they run
/// `0..n`, so a caller deriving a node from a position in some list is making
/// an assumption the platform never offered -- see `windows-placement-probe`,
/// where a positional index reaching `VirtualAllocExNuma` was a real defect and
/// is called out at the site that fixed it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NumaNode(u32);

impl NumaNode {
    /// Wrap a raw node number.
    ///
    /// Not validated against the machine: `VirtualAllocExNuma` is where an
    /// impossible node is rejected, and validating here would mean a second,
    /// weaker opinion about what exists. See [`NumaBuffer::new`] for what the
    /// platform actually does with an out-of-range node, which is not what its
    /// documentation says.
    ///
    /// [`NumaBuffer::new`]: crate::NumaBuffer::new
    #[must_use]
    pub const fn new(node: u32) -> Self {
        Self(node)
    }

    /// The raw node number, for handing to a Win32 call.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl std::fmt::Display for NumaNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "node {}", self.0)
    }
}

/// The highest node number this machine reports, if it answers.
///
/// `GetNumaHighestNodeNumber`. The value is a *node number*, not a count: a
/// machine with one node answers `NumaNode(0)`, not one.
///
/// # Why a caller wants this
///
/// Mostly to qualify a report rather than to drive a choice. On a machine that
/// answers `NumaNode(0)` there is exactly one node, so every placement decision
/// is the same decision, and a line saying "placed on node 0" is true and
/// misleading. Asking this is how a caller can say which of those it is.
///
/// # Errors
///
/// `None` when the call fails, which is reported as "unknown" rather than as a
/// node count of zero -- a machine that will not say how many nodes it has is a
/// different thing from a machine with none.
#[must_use]
pub fn highest_numa_node() -> Option<NumaNode> {
    let mut highest: u32 = 0;
    // SAFETY: the out parameter is a live local for the call's duration.
    let ok = unsafe { GetNumaHighestNodeNumber(&raw mut highest) };
    (ok != 0).then_some(NumaNode::new(highest))
}

/// The NUMA node `handle`'s **volume** reports, if it reports one.
///
/// `FSCTL_QUERY_VOLUME_NUMA_INFO`, issued against the handle directly -- the
/// documented control code accepts a file or directory handle, so this needs no
/// device-tree walk and no second open.
///
/// # This answers a question about a volume, not about a file
///
/// The documented meaning is the node the *volume* resides on. A volume may
/// span several devices -- an ordinary spanned volume or a Storage Spaces set
/// does -- and then a single reported node is a fiction rather than an answer,
/// because the file's extents may live anywhere across the set. **It therefore
/// cannot tell a caller which node a particular file's I/O is closest to**, even
/// when it succeeds.
///
/// That is a limit of the question, not of this wrapper, and it is why this
/// function reports rather than acts: a caller who knows their storage layout
/// can decide what the answer is worth, and one who does not should not have a
/// placement chosen for them on the strength of it.
///
/// # Errors
///
/// Any error from `DeviceIoControl`, and
/// [`io::ErrorKind::InvalidData`] if the control code returns a payload that is
/// not the documented single `u32`.
pub fn volume_numa_node(handle: RawHandle) -> io::Result<NumaNode> {
    let mut node: u32 = u32::MAX;
    let mut returned: u32 = 0;
    // SAFETY: `handle` is the caller's, and outlives this call by the contract
    // on this function. The FSCTL takes no input buffer, and its output is
    // documented as `FSCTL_QUERY_VOLUME_NUMA_INFO_OUTPUT { ULONG NumaNode }` --
    // one `u32`, which is what `node` provides and what the size argument says.
    let ok = unsafe {
        DeviceIoControl(
            handle as HANDLE,
            FSCTL_QUERY_VOLUME_NUMA_INFO,
            std::ptr::null(),
            0,
            (&raw mut node).cast::<std::ffi::c_void>(),
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
    Ok(NumaNode::new(node))
}

#[cfg(test)]
mod tests;
