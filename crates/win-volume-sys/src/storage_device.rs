// Copyright (c) 2026 Mike Grier
//! What a disk says it is: `IOCTL_STORAGE_QUERY_PROPERTY` for `StorageDeviceProperty`, whose reply
//! is `STORAGE_DEVICE_DESCRIPTOR`.
//!
//! The descriptor carries the device's SCSI type, whether its media is removable, whether it
//! queues commands, the bus it is attached by, and four optional strings -- vendor, product,
//! revision and serial number -- appended after the structure, with bus-specific bytes. Everything
//! is carried **as the device wrote it**: a bus type the documentation does not name keeps its
//! number, and the strings are the bytes before their terminator, not checked to be ASCII.
//!
//! # What the documentation says, and what it does not
//!
//! The `STORAGE_DEVICE_DESCRIPTOR` page (read 2026-11-05) says each string offset is "the byte
//! offset from the beginning of the structure to a null-terminated ASCII string", and that an
//! offset of zero means the device has none; the reply's size is in its `Size` member. The
//! `STORAGE_BUS_TYPE` page names its values without describing most of them, so a value's name is
//! all this crate states about it. That page lists `BusTypeNvmeof` as 20; `windows-sys` 0.61.2
//! does not have it and calls 20 `BusTypeMax`, so it is named here from the page.
//!
//! A string or the bus-specific bytes that would lie outside the reply, a string that starts
//! inside the structure's fixed fields, or one with no terminator in the reply, is
//! [`io::ErrorKind::InvalidData`]: those are this crate's checks that a reply holds what its header
//! says, not conditions the page states.

use std::fmt;
use std::io;
use std::mem::offset_of;
use std::os::windows::io::AsHandle;

use windows_sys::Win32::Storage::FileSystem::{
    BusType1394, BusTypeAta, BusTypeAtapi, BusTypeFibre, BusTypeFileBackedVirtual, BusTypeMmc,
    BusTypeNvme, BusTypeRAID, BusTypeSCM, BusTypeSas, BusTypeSata, BusTypeScsi, BusTypeSd,
    BusTypeSpaces, BusTypeSsa, BusTypeUfs, BusTypeUnknown, BusTypeUsb, BusTypeVirtual,
    BusTypeiScsi, STORAGE_BUS_TYPE,
};
use windows_sys::Win32::System::Ioctl::{
    PropertyStandardQuery, STORAGE_DEVICE_DESCRIPTOR, StorageDeviceProperty,
};

use crate::device_control::{invalid_data, u32_at};
use crate::storage_property::{FIRST_BYTES, query_input, query_property};

#[cfg(test)]
mod tests;

/// Where the descriptor's fields lie, from the structure `windows-sys` declares.
mod layout {
    use super::{STORAGE_DEVICE_DESCRIPTOR, offset_of};

    pub(super) const VERSION: usize = offset_of!(STORAGE_DEVICE_DESCRIPTOR, Version);
    pub(super) const SIZE: usize = offset_of!(STORAGE_DEVICE_DESCRIPTOR, Size);
    pub(super) const DEVICE_TYPE: usize = offset_of!(STORAGE_DEVICE_DESCRIPTOR, DeviceType);
    pub(super) const DEVICE_TYPE_MODIFIER: usize =
        offset_of!(STORAGE_DEVICE_DESCRIPTOR, DeviceTypeModifier);
    pub(super) const REMOVABLE_MEDIA: usize = offset_of!(STORAGE_DEVICE_DESCRIPTOR, RemovableMedia);
    pub(super) const COMMAND_QUEUEING: usize =
        offset_of!(STORAGE_DEVICE_DESCRIPTOR, CommandQueueing);
    pub(super) const VENDOR_ID_OFFSET: usize =
        offset_of!(STORAGE_DEVICE_DESCRIPTOR, VendorIdOffset);
    pub(super) const PRODUCT_ID_OFFSET: usize =
        offset_of!(STORAGE_DEVICE_DESCRIPTOR, ProductIdOffset);
    pub(super) const PRODUCT_REVISION_OFFSET: usize =
        offset_of!(STORAGE_DEVICE_DESCRIPTOR, ProductRevisionOffset);
    pub(super) const SERIAL_NUMBER_OFFSET: usize =
        offset_of!(STORAGE_DEVICE_DESCRIPTOR, SerialNumberOffset);
    pub(super) const BUS_TYPE: usize = offset_of!(STORAGE_DEVICE_DESCRIPTOR, BusType);
    pub(super) const RAW_PROPERTIES_LENGTH: usize =
        offset_of!(STORAGE_DEVICE_DESCRIPTOR, RawPropertiesLength);
    /// Where the bus-specific bytes begin, and so where the fixed fields end.
    pub(super) const RAW_PROPERTIES: usize =
        offset_of!(STORAGE_DEVICE_DESCRIPTOR, RawDeviceProperties);
}

/// The bus a disk is attached by, as `STORAGE_BUS_TYPE` numbers it.
///
/// A raw-carrying value: the documented values are named, and any other is kept as the device
/// wrote it. Changing a named value is a breaking change.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct StorageBusType(STORAGE_BUS_TYPE);

impl StorageBusType {
    /// `BusTypeUnknown`.
    pub const UNKNOWN: Self = Self(BusTypeUnknown);
    /// `BusTypeScsi`.
    pub const SCSI: Self = Self(BusTypeScsi);
    /// `BusTypeAtapi`.
    pub const ATAPI: Self = Self(BusTypeAtapi);
    /// `BusTypeAta`.
    pub const ATA: Self = Self(BusTypeAta);
    /// `BusType1394`.
    pub const IEEE_1394: Self = Self(BusType1394);
    /// `BusTypeSsa`.
    pub const SSA: Self = Self(BusTypeSsa);
    /// `BusTypeFibre`.
    pub const FIBRE: Self = Self(BusTypeFibre);
    /// `BusTypeUsb`.
    pub const USB: Self = Self(BusTypeUsb);
    /// `BusTypeRAID`.
    pub const RAID: Self = Self(BusTypeRAID);
    /// `BusTypeiScsi`.
    pub const ISCSI: Self = Self(BusTypeiScsi);
    /// `BusTypeSas`.
    pub const SAS: Self = Self(BusTypeSas);
    /// `BusTypeSata`.
    pub const SATA: Self = Self(BusTypeSata);
    /// `BusTypeSd`.
    pub const SD: Self = Self(BusTypeSd);
    /// `BusTypeMmc`.
    pub const MMC: Self = Self(BusTypeMmc);
    /// `BusTypeVirtual`.
    pub const VIRTUAL: Self = Self(BusTypeVirtual);
    /// `BusTypeFileBackedVirtual`.
    pub const FILE_BACKED_VIRTUAL: Self = Self(BusTypeFileBackedVirtual);
    /// `BusTypeSpaces`.
    pub const SPACES: Self = Self(BusTypeSpaces);
    /// `BusTypeNvme`.
    pub const NVME: Self = Self(BusTypeNvme);
    /// `BusTypeSCM`.
    pub const SCM: Self = Self(BusTypeSCM);
    /// `BusTypeUfs`.
    pub const UFS: Self = Self(BusTypeUfs);
    /// `BusTypeNvmeof`, 20 by the `STORAGE_BUS_TYPE` page; `windows-sys` 0.61.2 does not declare it.
    pub const NVMEOF: Self = Self(20);

    const NAMED: [(Self, &'static str); 21] = [
        (Self::UNKNOWN, "BusTypeUnknown"),
        (Self::SCSI, "BusTypeScsi"),
        (Self::ATAPI, "BusTypeAtapi"),
        (Self::ATA, "BusTypeAta"),
        (Self::IEEE_1394, "BusType1394"),
        (Self::SSA, "BusTypeSsa"),
        (Self::FIBRE, "BusTypeFibre"),
        (Self::USB, "BusTypeUsb"),
        (Self::RAID, "BusTypeRAID"),
        (Self::ISCSI, "BusTypeiScsi"),
        (Self::SAS, "BusTypeSas"),
        (Self::SATA, "BusTypeSata"),
        (Self::SD, "BusTypeSd"),
        (Self::MMC, "BusTypeMmc"),
        (Self::VIRTUAL, "BusTypeVirtual"),
        (Self::FILE_BACKED_VIRTUAL, "BusTypeFileBackedVirtual"),
        (Self::SPACES, "BusTypeSpaces"),
        (Self::NVME, "BusTypeNvme"),
        (Self::SCM, "BusTypeSCM"),
        (Self::UFS, "BusTypeUfs"),
        (Self::NVMEOF, "BusTypeNvmeof"),
    ];

    /// A bus type from its number, kept whether or not it is one the documentation names.
    #[must_use]
    pub const fn from_raw(raw: STORAGE_BUS_TYPE) -> Self {
        Self(raw)
    }

    /// The number.
    #[must_use]
    pub const fn raw(self) -> STORAGE_BUS_TYPE {
        self.0
    }

    /// The documentation's name for it, or `None` for a number it does not name.
    #[must_use]
    pub fn name(self) -> Option<&'static str> {
        Self::NAMED
            .iter()
            .find(|(value, _)| *value == self)
            .map(|(_, name)| *name)
    }
}

impl fmt::Debug for StorageBusType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            Some(name) => write!(f, "{name}"),
            None => write!(f, "StorageBusType({})", self.0),
        }
    }
}

impl fmt::Display for StorageBusType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

/// What a disk reported in its device descriptor.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct StorageDeviceDescriptor {
    /// The version of the descriptor, as the device wrote it.
    pub version: u32,
    /// The size of the descriptor with what is appended to it, as the device wrote it.
    pub size: u32,
    /// The device type, as the SCSI specification numbers it.
    pub device_type: u8,
    /// The device type modifier, zero when there is none.
    pub device_type_modifier: u8,
    /// Whether the device's media is removable. The page says to ignore it for a device with no
    /// media.
    pub removable_media: bool,
    /// Whether the device takes several commands at once.
    pub command_queueing: bool,
    /// The bus the device is attached by.
    pub bus_type: StorageBusType,
    /// The vendor ID string's bytes, without its terminator; `None` when the device has none.
    pub vendor_id: Option<Vec<u8>>,
    /// The product ID string's bytes, without its terminator; `None` when the device has none.
    pub product_id: Option<Vec<u8>>,
    /// The product revision string's bytes, without its terminator; `None` when the device has none.
    pub product_revision: Option<Vec<u8>>,
    /// The serial number string's bytes, without its terminator; `None` when the device has none.
    pub serial_number: Option<Vec<u8>>,
    /// The bus-specific bytes appended to the descriptor, which `bus_type` says how to read.
    pub raw_properties: Vec<u8>,
}

impl StorageDeviceDescriptor {
    /// Reads `bytes`, which is the whole reply.
    fn decode(bytes: &[u8]) -> io::Result<Self> {
        if bytes.len() < layout::RAW_PROPERTIES {
            return Err(invalid_data("a reply shorter than the device descriptor"));
        }
        let word = |offset| u32_at(bytes, offset).ok_or_else(|| invalid_data("a field cut short"));
        let string = |offset_at: usize| -> io::Result<Option<Vec<u8>>> {
            let offset = usize::try_from(word(offset_at)?)
                .map_err(|_| invalid_data("a string offset too large"))?;
            if offset == 0 {
                return Ok(None);
            }
            if offset < layout::RAW_PROPERTIES {
                return Err(invalid_data(
                    "a string that starts inside the descriptor's fields",
                ));
            }
            let rest = bytes
                .get(offset..)
                .ok_or_else(|| invalid_data("a string that starts past the end of the reply"))?;
            let end = rest
                .iter()
                .position(|byte| *byte == 0)
                .ok_or_else(|| invalid_data("a string with no terminator in the reply"))?;
            Ok(Some(rest[..end].to_vec()))
        };
        let raw_length = usize::try_from(word(layout::RAW_PROPERTIES_LENGTH)?)
            .map_err(|_| invalid_data("a bus-specific length too large"))?;
        let raw_properties = layout::RAW_PROPERTIES
            .checked_add(raw_length)
            .and_then(|end| bytes.get(layout::RAW_PROPERTIES..end))
            .ok_or_else(|| invalid_data("bus-specific bytes that run past the end of the reply"))?
            .to_vec();
        Ok(Self {
            version: word(layout::VERSION)?,
            size: word(layout::SIZE)?,
            device_type: bytes[layout::DEVICE_TYPE],
            device_type_modifier: bytes[layout::DEVICE_TYPE_MODIFIER],
            removable_media: bytes[layout::REMOVABLE_MEDIA] != 0,
            command_queueing: bytes[layout::COMMAND_QUEUEING] != 0,
            bus_type: StorageBusType::from_raw(word(layout::BUS_TYPE)?.cast_signed()),
            vendor_id: string(layout::VENDOR_ID_OFFSET)?,
            product_id: string(layout::PRODUCT_ID_OFFSET)?,
            product_revision: string(layout::PRODUCT_REVISION_OFFSET)?,
            serial_number: string(layout::SERIAL_NUMBER_OFFSET)?,
            raw_properties,
        })
    }
}

/// The device descriptor of the disk `disk` names, asking again at the size the reply names if the
/// first buffer was too small.
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
/// use win_volume_sys::storage_device_descriptor;
///
/// let disk = OpenOptions::new()
///     .access_mode(0)
///     .share_mode(7)
///     .open(r"\\.\PhysicalDrive0")?;
///
/// println!("on bus {}", storage_device_descriptor(&disk)?.bus_type);
/// # Ok::<(), std::io::Error>(())
/// ```
///
/// # Errors
///
/// The device's own error, such as `ERROR_INVALID_FUNCTION` for a handle that is not a disk, and
/// [`io::ErrorKind::InvalidData`] for a reply shorter than the descriptor's fields, or one whose
/// strings or bus-specific bytes do not lie within it. An allocation that cannot be made for a size
/// the device named is `ERROR_NOT_ENOUGH_MEMORY`, and the call is not made again.
pub fn storage_device_descriptor(disk: &impl AsHandle) -> io::Result<StorageDeviceDescriptor> {
    let input = query_input(StorageDeviceProperty, PropertyStandardQuery, &[])?;
    let bytes = query_property(disk, &input, FIRST_BYTES)?;
    StorageDeviceDescriptor::decode(&bytes)
}
