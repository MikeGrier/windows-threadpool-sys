// Copyright (c) 2026 Mike Grier
//! The NUMA node a disk is attached to: `IOCTL_STORAGE_QUERY_PROPERTY` for
//! `StorageDeviceNumaProperty`.
//!
//! It is per disk on purpose. A volume that spans disks has one answer from
//! [`volume_numa_node`](crate::volume_numa_node) and a node per disk from here, and a single node
//! reported for a volume whose disks sit on different nodes would be a fiction; the disks of a
//! volume come from [`volume_disk_extents`](crate::volume_disk_extents).
//!
//! # What the answer is, and is not
//!
//! The property is documented as providing "the non-uniform memory access (NUMA) node of the
//! storage device". The reply is three `u32`s, `STORAGE_DEVICE_NUMA_PROPERTY`: a version, a size
//! and the node. The node is carried **unaltered**. A device with no node it can name answers with
//! something, and no value is documented as meaning "none", so none is invented here: whether a
//! given value means no node is for the caller to decide.
//!
//! One thing is not documented, and is stated as such. The property's description does not name a
//! reply structure, and the structure's own reference page was not found; the structure is
//! declared in the public SDK header `winioctl.h`, which `windows-sys` carries, and is what is
//! read here.

use std::fmt;
use std::io;
use std::mem::offset_of;
use std::os::windows::io::AsHandle;

use windows_sys::Win32::System::Ioctl::{
    PropertyStandardQuery, STORAGE_DEVICE_NUMA_PROPERTY, StorageDeviceNumaProperty,
};

use crate::device_control::{invalid_data, u32_at};
use crate::storage_property::{FIRST_BYTES, query_input, query_property};

#[cfg(test)]
mod tests;

/// What a disk reported as the NUMA node it is attached to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StorageDeviceNuma {
    /// The version of the descriptor, as the device wrote it.
    pub version: u32,
    /// The size of the descriptor, as the device wrote it.
    pub size: u32,
    /// The node, as the device wrote it.
    ///
    /// Carried unaltered, so a value that is not a node of this machine reaches the caller as it
    /// was: whether such a value means "no node" is for the caller to decide, since the
    /// documentation does not say.
    pub node: u32,
}

impl StorageDeviceNuma {
    /// Reads `bytes`, which is the whole reply: the version, the size and the node.
    ///
    /// A reply longer than the structure, as a later version might be, is read for the fields it
    /// begins with.
    fn decode(bytes: &[u8]) -> io::Result<Self> {
        let read = |offset| {
            u32_at(bytes, offset)
                .ok_or_else(|| invalid_data("a reply shorter than the NUMA descriptor"))
        };
        Ok(Self {
            version: read(offset_of!(STORAGE_DEVICE_NUMA_PROPERTY, Version))?,
            size: read(offset_of!(STORAGE_DEVICE_NUMA_PROPERTY, Size))?,
            node: read(offset_of!(STORAGE_DEVICE_NUMA_PROPERTY, NumaNode))?,
        })
    }
}

impl fmt::Display for StorageDeviceNuma {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NUMA node {}", self.node)
    }
}

/// The NUMA node the disk `disk` names is attached to, asking again at the size the reply names if
/// the first buffer was too small.
///
/// `disk` is a handle to a disk -- `\\.\PhysicalDriveN` -- and needs no access to it: a handle
/// opened with none is enough, which an ordinary account can open.
///
/// # Example
///
/// ```no_run
/// use std::fs::OpenOptions;
/// use std::os::windows::fs::OpenOptionsExt;
///
/// use win_volume_sys::storage_device_numa;
///
/// let disk = OpenOptions::new()
///     .access_mode(0)
///     .share_mode(7)
///     .open(r"\\.\PhysicalDrive0")?;
///
/// println!("{}", storage_device_numa(&disk)?);
/// # Ok::<(), std::io::Error>(())
/// ```
///
/// # Errors
///
/// The device's own error, such as `ERROR_INVALID_FUNCTION` for a handle that is not a disk, and
/// [`io::ErrorKind::InvalidData`] for a reply shorter than the descriptor or one that does not hold
/// what its header says. An allocation that cannot be made for a size the device named is
/// `ERROR_NOT_ENOUGH_MEMORY`, and the call is not made again.
pub fn storage_device_numa(disk: &impl AsHandle) -> io::Result<StorageDeviceNuma> {
    let input = query_input(StorageDeviceNumaProperty, PropertyStandardQuery, &[])?;
    let bytes = query_property(disk, &input, FIRST_BYTES)?;
    StorageDeviceNuma::decode(&bytes)
}
