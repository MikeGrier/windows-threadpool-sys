// Copyright (c) Mike Grier.

//! The `IOCTL_STORAGE_QUERY_PROPERTY` entry for `StorageDeviceNumaProperty` (NR-2.4).
//!
//! A storage-locality entry, added after round one for durable-ioring's flush domains: the NUMA
//! node a **disk** is attached to. It is per disk on purpose. A volume that spans disks has one
//! answer from [`crate::volume_numa`] and a node per disk from here, and a single node reported
//! for a volume whose disks sit on different nodes would be a fiction; the disks of a volume come
//! from [`crate::volume_extents`].
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
//!
//! The reply's size is learned as [`crate::storage_property`] describes. A handle to a disk
//! (`\\.\PhysicalDriveN`) needs no access to answer. The call goes through the crate's one
//! `DeviceIoControl` helper (D-20).

use std::fmt;
use std::mem::offset_of;

use windows_sys::Win32::System::Ioctl::{
    PropertyStandardQuery, STORAGE_DEVICE_NUMA_PROPERTY, StorageDeviceNumaProperty,
};

use crate::device_control::u32_at;
use crate::handle::{CapturedHandle, HandleCaptureError};
use crate::storage_property::{FIRST_BYTES, StoragePropertyError, query_input, query_property};

#[cfg(test)]
mod tests;

/// What a disk reported as the NUMA node it is attached to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StorageDeviceNuma {
    version: u32,
    size: u32,
    node: u32,
}

impl StorageDeviceNuma {
    /// The node, as the device wrote it.
    ///
    /// Carried unaltered, so a value that is not a node of this machine reaches the caller as it
    /// was: whether such a value means "no node" is for the caller to decide, since the
    /// documentation does not say.
    #[must_use]
    pub fn node(&self) -> u32 {
        self.node
    }

    /// The version of the descriptor, as the device wrote it.
    #[must_use]
    pub fn version(&self) -> u32 {
        self.version
    }

    /// The size of the descriptor, as the device wrote it.
    #[must_use]
    pub fn size(&self) -> u32 {
        self.size
    }

    /// Reads `bytes`, which is the whole reply: the version, the size and the node.
    ///
    /// A reply longer than the structure, as a later version might be, is read for the fields it
    /// begins with.
    fn decode(bytes: &[u8]) -> Result<Self, StoragePropertyError> {
        let read = |offset| {
            u32_at(bytes, offset).ok_or(StoragePropertyError::Malformed {
                what: "a reply shorter than the NUMA descriptor",
            })
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

/// An owned, marshalable parameter set for `DeviceIoControl` with `IOCTL_STORAGE_QUERY_PROPERTY`
/// and `StorageDeviceNumaProperty`.
///
/// # Example
///
/// The handle names a disk, and needs no access to it, which an ordinary account can open:
///
/// ```no_run
/// use std::fs::OpenOptions;
/// use std::os::windows::fs::OpenOptionsExt;
/// use std::os::windows::io::AsHandle;
///
/// use windows_namespace_request_sys::storage_numa::QueryStorageDeviceNumaNode;
/// use windows_namespace_request_sys::CapturedHandle;
///
/// let disk = OpenOptions::new()
///     .access_mode(0)
///     .share_mode(7)
///     .open(r"\\.\PhysicalDrive0")?;
///
/// let numa = QueryStorageDeviceNumaNode::new(CapturedHandle::capture(disk.as_handle())?)
///     .perform()?;
/// println!("{numa}");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug)]
#[must_use = "an unperformed request queries nothing"]
pub struct QueryStorageDeviceNumaNode {
    handle: CapturedHandle,
}

impl QueryStorageDeviceNumaNode {
    /// Begins a request against `handle`, which names a disk.
    pub fn new(handle: CapturedHandle) -> Self {
        Self { handle }
    }

    /// The owned duplicate of the handle being queried.
    pub fn handle(&self) -> &CapturedHandle {
        &self.handle
    }

    /// Copies the request, duplicating the handle.
    ///
    /// # Errors
    ///
    /// Returns the handle-capture failure when the handle cannot be duplicated.
    pub fn try_clone(&self) -> Result<Self, HandleCaptureError> {
        Ok(Self {
            handle: self.handle.try_clone()?,
        })
    }

    /// Performs the query on the calling thread, asking again at the size the reply names if the
    /// first buffer was too small.
    ///
    /// # Errors
    ///
    /// [`StoragePropertyError::Win32`] with the raw code, unaltered;
    /// [`StoragePropertyError::Malformed`] for a reply shorter than the descriptor;
    /// [`StoragePropertyError::Unstable`] if the size kept changing.
    pub fn perform(&self) -> Result<StorageDeviceNuma, StoragePropertyError> {
        let input = query_input(StorageDeviceNumaProperty, PropertyStandardQuery, &[])?;
        let bytes = query_property(self.handle.raw(), &input, FIRST_BYTES)?;
        StorageDeviceNuma::decode(&bytes)
    }
}

impl crate::request::Request for QueryStorageDeviceNumaNode {
    type Error = StoragePropertyError;
    type Output = StorageDeviceNuma;

    fn perform(&self) -> Result<StorageDeviceNuma, StoragePropertyError> {
        Self::perform(self)
    }
}
