// Copyright (c) 2026 Mike Grier
//! A disk's device unique identifier: `IOCTL_STORAGE_QUERY_PROPERTY` for
//! `StorageDeviceUniqueIdProperty`.
//!
//! The identifier names the device and not the slot it is in. A disk *number* can be given to a
//! different device after hot removal within one process; this does not change with the number,
//! which is why it, and not the number, is what a key for a disk is to be derived from.
//!
//! # What the reply is
//!
//! A `STORAGE_DEVICE_UNIQUE_IDENTIFIER` header (a version, the size of the whole reply, and three
//! offsets) followed by up to three sections the offsets point to: the device ID descriptor
//! (identifiers from the device's page 0x83 of vital product data), the device descriptor
//! (identifiers from non-VPD inquiry data) and the drive layout signature (the MBR signature or
//! GPT disk GUID). Each section starts with its own version and size, which is how this module
//! delimits it.
//!
//! The sections are reported as the bytes the device gave, **not parsed**: what an identifier
//! inside a descriptor is, and which of them a key is made from, is the caller's to decide. An
//! offset of zero means the section is not there. That is what was measured on a disk that
//! reports only a device ID descriptor; the documentation does not say, and an offset of zero
//! could not name a section anyway, since it is where the header is.
//!
//! The structure is declared in `storduid.h`, a driver-kit header that `windows-sys` does not
//! carry, so its five `u32` fields are laid out here from the documentation. The property id is
//! documented as intended for driver use; it is the documented route to this structure.

use std::fmt;
use std::io;
use std::ops::Range;
use std::os::windows::io::AsHandle;

use windows_sys::Win32::System::Ioctl::{PropertyStandardQuery, StorageDeviceUniqueIdProperty};

use crate::device_control::{invalid_data, u32_at};
use crate::storage_property::{FIRST_BYTES, query_input, query_property};

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
    fn decode(bytes: Vec<u8>) -> io::Result<Self> {
        // A reply shorter than the header is refused by the first of the three offset reads that
        // does not lie within it, so the header's version and size, which come first, are always
        // within the bytes once this has returned.
        let section = |offset_at: usize| -> io::Result<Option<Range<usize>>> {
            let offset =
                u32_at(&bytes, offset_at).ok_or_else(|| invalid_data("a header cut short"))?;
            let offset =
                usize::try_from(offset).map_err(|_| invalid_data("an offset too large"))?;
            if offset == 0 {
                return Ok(None);
            }
            if offset < layout::HEADER {
                return Err(invalid_data("a section that starts inside the header"));
            }
            let size = offset
                .checked_add(layout::SECTION_SIZE)
                .and_then(|at| u32_at(&bytes, at))
                .ok_or_else(|| invalid_data("a section with no header of its own"))?;
            let size = usize::try_from(size).map_err(|_| invalid_data("a section too large"))?;
            if size < layout::SECTION_HEADER {
                return Err(invalid_data(
                    "a section that says it is smaller than its header",
                ));
            }
            let end = offset
                .checked_add(size)
                .filter(|end| *end <= bytes.len())
                .ok_or_else(|| invalid_data("a section that runs past the end of the reply"))?;
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

/// The device unique identifier of the disk `disk` names, asking again at the size the reply names
/// if the first buffer was too small.
///
/// `disk` is a handle to a disk -- `\\.\PhysicalDriveN` -- and needs no access to it: a handle
/// opened with none is enough, which an ordinary account can open. The disk numbers a volume is on
/// come from [`volume_disk_extents`](crate::volume_disk_extents).
///
/// # Example
///
/// ```no_run
/// use std::fs::OpenOptions;
/// use std::os::windows::fs::OpenOptionsExt;
///
/// use win_volume_sys::storage_device_unique_id;
///
/// let disk = OpenOptions::new()
///     .access_mode(0)
///     .share_mode(7)
///     .open(r"\\.\PhysicalDrive0")?;
///
/// let id = storage_device_unique_id(&disk)?;
/// println!("{id}");
/// if let Some(descriptor) = id.device_id_descriptor() {
///     println!("{} bytes of device id descriptor", descriptor.len());
/// }
/// # Ok::<(), std::io::Error>(())
/// ```
///
/// # Errors
///
/// The device's own error, such as `ERROR_INVALID_FUNCTION` for a handle that is not a disk, and
/// [`io::ErrorKind::InvalidData`] for a reply that does not hold what it says: a header or a
/// section cut short, an offset that does not lie within the reply, a call that claims to have
/// written more than the buffer it was given, or a size that kept changing between the calls. An
/// allocation that cannot be made for a size the device named is `ERROR_NOT_ENOUGH_MEMORY`, and the
/// call is not made again.
pub fn storage_device_unique_id(disk: &impl AsHandle) -> io::Result<StorageDeviceUniqueId> {
    let input = query_input(StorageDeviceUniqueIdProperty, PropertyStandardQuery, &[])?;
    let bytes = query_property(disk, &input, FIRST_BYTES)?;
    StorageDeviceUniqueId::decode(bytes)
}
