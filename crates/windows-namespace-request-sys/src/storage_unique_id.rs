// Copyright (c) Mike Grier.

//! The `IOCTL_STORAGE_QUERY_PROPERTY` entry for `StorageDeviceUniqueIdProperty` (NR-2.3).
//!
//! A storage-locality entry, added after round one for durable-ioring's flush domains: a disk's
//! device unique identifier (DUID), which names the device and not the slot it is in. A disk
//! *number* can be given to a different device after hot removal within one process; this does
//! not change with the number, which is why it, and not the number, is what a key for a disk is
//! to be derived from.
//!
//! # What the reply is
//!
//! A `STORAGE_DEVICE_UNIQUE_IDENTIFIER` header (a version, the size of the whole reply, and three
//! offsets) followed by up to three sections the offsets point to: the device ID descriptor
//! (identifiers from the device's page 0x83 of vital product data), the device descriptor
//! (identifiers from non-VPD inquiry data) and the drive layout signature (the MBR signature or
//! GPT disk GUID). Each section starts with its own version and size, which is how this entry
//! delimits it.
//!
//! The sections are reported as the bytes the device gave, **not parsed**: what an identifier
//! inside a descriptor is, and which of them a key is made from, is the consumer's to decide. An
//! offset of zero means the section is not there. That is what was measured on a disk that
//! reports only a device ID descriptor; the documentation does not say, and an offset of zero
//! could not name a section anyway, since it is where the header is.
//!
//! The structure is declared in `storduid.h`, a driver-kit header that `windows-sys` does not
//! carry, so its five `u32` fields are laid out here from the documentation. The property id is
//! documented as intended for driver use; it is the documented route to this structure.
//!
//! A handle to a disk (`\\.\PhysicalDriveN`) needs no access to answer, which an ordinary
//! account can open. The disk number for a volume comes from [`crate::volume_extents`]. The call
//! goes through the crate's one `DeviceIoControl` helper (D-20), and the size of the reply is
//! learned as [`crate::storage_property`] describes.

use std::fmt;
use std::ops::Range;

use windows_sys::Win32::System::Ioctl::{PropertyStandardQuery, StorageDeviceUniqueIdProperty};

use crate::device_control::u32_at;
use crate::handle::{CapturedHandle, HandleCaptureError};
use crate::storage_property::{FIRST_BYTES, StoragePropertyError, query_input, query_property};

#[cfg(test)]
mod tests;

/// Where each field of `STORAGE_DEVICE_UNIQUE_IDENTIFIER` is, from `storduid.h`.
///
/// Five `u32`s. Changing any of these is a breaking change: they are the device's layout.
mod layout {
    pub(super) const VERSION: usize = 0;
    pub(super) const SIZE: usize = 4;
    pub(super) const ID_DESCRIPTOR_OFFSET: usize = 8;
    pub(super) const DEVICE_DESCRIPTOR_OFFSET: usize = 12;
    pub(super) const LAYOUT_SIGNATURE_OFFSET: usize = 16;
    pub(super) const HEADER: usize = 20;

    /// Every section starts with its own version and size, two `u32`s.
    pub(super) const SECTION_HEADER: usize = 8;
    pub(super) const SECTION_SIZE: usize = 4;
}

/// A disk's device unique identifier, as the device reported it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct StorageDeviceUniqueId {
    bytes: Vec<u8>,
    id_descriptor: Option<Range<usize>>,
    device_descriptor: Option<Range<usize>>,
    layout_signature: Option<Range<usize>>,
}

impl StorageDeviceUniqueId {
    /// The version of the identifier, as the device wrote it.
    #[must_use]
    pub fn version(&self) -> u32 {
        self.field(layout::VERSION)
    }

    /// The size of the whole reply, header and sections, which is also the length of
    /// [`as_bytes`](Self::as_bytes).
    #[must_use]
    pub fn size(&self) -> u32 {
        self.field(layout::SIZE)
    }

    /// The whole reply, header and sections, exactly as the device gave it.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The device ID descriptor (`STORAGE_DEVICE_ID_DESCRIPTOR`) as bytes, if the device reported
    /// one: the identifiers from page 0x83 of its vital product data.
    #[must_use]
    pub fn device_id_descriptor(&self) -> Option<&[u8]> {
        self.section(self.id_descriptor.as_ref())
    }

    /// The device descriptor (`STORAGE_DEVICE_DESCRIPTOR`) as bytes, if the device reported one:
    /// identifiers from non-VPD inquiry data.
    #[must_use]
    pub fn device_descriptor(&self) -> Option<&[u8]> {
        self.section(self.device_descriptor.as_ref())
    }

    /// The drive layout signature (`STORAGE_DEVICE_LAYOUT_SIGNATURE`) as bytes, if the device
    /// reported one: the MBR signature or the GPT disk GUID.
    #[must_use]
    pub fn drive_layout_signature(&self) -> Option<&[u8]> {
        self.section(self.layout_signature.as_ref())
    }

    fn field(&self, offset: usize) -> u32 {
        // The header was checked to lie within the bytes when this was built.
        u32_at(&self.bytes, offset).unwrap_or_default()
    }

    fn section(&self, range: Option<&Range<usize>>) -> Option<&[u8]> {
        range.map(|range| &self.bytes[range.clone()])
    }

    /// Reads `bytes`, which is the whole reply, delimiting each section the header points to.
    fn decode(bytes: Vec<u8>) -> Result<Self, StoragePropertyError> {
        let malformed = |what| StoragePropertyError::Malformed { what };

        // A reply shorter than the header is refused by the first of the three offset reads that
        // does not lie within it, so the header's version and size, which come first, are always
        // within the bytes once this has returned.
        let section = |offset_at: usize| -> Result<Option<Range<usize>>, StoragePropertyError> {
            let offset = u32_at(&bytes, offset_at).ok_or(malformed("a header cut short"))?;
            let offset = usize::try_from(offset).map_err(|_| malformed("an offset too large"))?;
            if offset == 0 {
                return Ok(None);
            }
            if offset < layout::HEADER {
                return Err(malformed("a section that starts inside the header"));
            }
            let size = offset
                .checked_add(layout::SECTION_SIZE)
                .and_then(|at| u32_at(&bytes, at))
                .ok_or(malformed("a section with no header of its own"))?;
            let size = usize::try_from(size).map_err(|_| malformed("a section too large"))?;
            if size < layout::SECTION_HEADER {
                return Err(malformed(
                    "a section that says it is smaller than its header",
                ));
            }
            let end = offset
                .checked_add(size)
                .filter(|end| *end <= bytes.len())
                .ok_or(malformed("a section that runs past the end of the reply"))?;
            Ok(Some(offset..end))
        };

        Ok(Self {
            id_descriptor: section(layout::ID_DESCRIPTOR_OFFSET)?,
            device_descriptor: section(layout::DEVICE_DESCRIPTOR_OFFSET)?,
            layout_signature: section(layout::LAYOUT_SIGNATURE_OFFSET)?,
            bytes,
        })
    }
}

impl fmt::Display for StorageDeviceUniqueId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let present = |section: &Option<Range<usize>>| if section.is_some() { "yes" } else { "no" };
        write!(
            f,
            "device unique identifier, version {}, {} bytes (device id {}, device descriptor {}, layout signature {})",
            self.version(),
            self.size(),
            present(&self.id_descriptor),
            present(&self.device_descriptor),
            present(&self.layout_signature)
        )
    }
}

/// An owned, marshalable parameter set for `DeviceIoControl` with `IOCTL_STORAGE_QUERY_PROPERTY`
/// and `StorageDeviceUniqueIdProperty`.
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
/// use windows_namespace_request_sys::storage_unique_id::QueryStorageDeviceUniqueId;
/// use windows_namespace_request_sys::CapturedHandle;
///
/// let disk = OpenOptions::new()
///     .access_mode(0)
///     .share_mode(7)
///     .open(r"\\.\PhysicalDrive0")?;
///
/// let id = QueryStorageDeviceUniqueId::new(CapturedHandle::capture(disk.as_handle())?)
///     .perform()?;
/// println!("{id}");
/// if let Some(descriptor) = id.device_id_descriptor() {
///     println!("{} bytes of device id descriptor", descriptor.len());
/// }
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug)]
#[must_use = "an unperformed request queries nothing"]
pub struct QueryStorageDeviceUniqueId {
    handle: CapturedHandle,
}

impl QueryStorageDeviceUniqueId {
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
    /// [`StoragePropertyError::Malformed`] for a reply that does not hold what it says;
    /// [`StoragePropertyError::Unstable`] if the size kept changing.
    pub fn perform(&self) -> Result<StorageDeviceUniqueId, StoragePropertyError> {
        let input = query_input(StorageDeviceUniqueIdProperty, PropertyStandardQuery, &[])?;
        let bytes = query_property(self.handle.raw(), &input, FIRST_BYTES)?;
        StorageDeviceUniqueId::decode(bytes)
    }
}

impl crate::request::Request for QueryStorageDeviceUniqueId {
    type Error = StoragePropertyError;
    type Output = StorageDeviceUniqueId;

    fn perform(&self) -> Result<StorageDeviceUniqueId, StoragePropertyError> {
        Self::perform(self)
    }
}
