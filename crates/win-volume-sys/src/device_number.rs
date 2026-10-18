// Copyright (c) 2026 Mike Grier
//! The device type, number and partition number of a volume or a disk:
//! `IOCTL_STORAGE_GET_DEVICE_NUMBER`.
//!
//! # What the answer is, and is not
//!
//! The control "returns a `STORAGE_DEVICE_NUMBER` structure that contains the `FILE_DEVICE_XXX` type,
//! device number, and, for a partitionable device, the partition number assigned to a device by the
//! driver when the device is started" (the control's page, `ms.date` 08/18/2022). The structure is
//! three `u32`s. The structure's page (03/29/2018) says the device number "is set to 0xFFFFFFFF (-1)
//! for the disks that represent the physical paths of an MPIO disk", that the partition number is
//! "-1" for a device that cannot be partitioned, and that the values "are guaranteed to remain
//! unchanged until the system is rebooted" and are "not guaranteed to be persistent across boots".
//!
//! All three are carried **unaltered**, so a `0xFFFFFFFF` reaches the caller as the device wrote it
//! and is not turned into an `Option` or a count here. What a value means to a caller is the
//! caller's to decide.
//!
//! A handle that is not a volume or a disk is refused by the device, which is how a caller can ask
//! whether a handle is one: a success shows it is, as for
//! [`volume_disk_extents`](crate::volume_disk_extents).

use std::fmt;
use std::io;
use std::mem::{offset_of, size_of};
use std::os::windows::io::AsHandle;

use windows_sys::Win32::System::Ioctl::{IOCTL_STORAGE_GET_DEVICE_NUMBER, STORAGE_DEVICE_NUMBER};

use crate::device_control::{device_control, invalid_data, u32_at};

#[cfg(test)]
mod tests;

/// What a volume or a disk reported as its device type, number and partition number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StorageDeviceNumber {
    /// The `FILE_DEVICE_XXX` type, as the device wrote it.
    pub device_type: u32,
    /// The number of the device, as the device wrote it: for a volume, the number of the disk it is
    /// on. `0xFFFFFFFF` for the disks that represent the physical paths of an MPIO disk.
    pub device_number: u32,
    /// The partition number, as the device wrote it. `0xFFFFFFFF` for a device that cannot be
    /// partitioned.
    pub partition_number: u32,
}

impl StorageDeviceNumber {
    /// Reads `bytes`, which is the whole reply: the type, the number and the partition number.
    ///
    /// A reply longer than the structure is read for the fields it begins with.
    fn decode(bytes: &[u8]) -> io::Result<Self> {
        let read = |offset| {
            u32_at(bytes, offset)
                .ok_or_else(|| invalid_data("a reply shorter than a device number"))
        };
        Ok(Self {
            device_type: read(offset_of!(STORAGE_DEVICE_NUMBER, DeviceType))?,
            device_number: read(offset_of!(STORAGE_DEVICE_NUMBER, DeviceNumber))?,
            partition_number: read(offset_of!(STORAGE_DEVICE_NUMBER, PartitionNumber))?,
        })
    }
}

impl fmt::Display for StorageDeviceNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "device type {:#x}, number {}, partition {}",
            self.device_type, self.device_number, self.partition_number
        )
    }
}

/// The device type, number and partition number the volume or disk `device` reports.
///
/// `device` is a handle to a volume or a disk and needs no access to it, as for
/// [`bytes_per_sector`](crate::bytes_per_sector): a handle opened with none is enough.
///
/// # Example
///
/// ```no_run
/// use std::fs::OpenOptions;
/// use std::os::windows::fs::OpenOptionsExt;
///
/// use win_volume_sys::storage_device_number;
///
/// let volume = OpenOptions::new()
///     .access_mode(0)
///     .share_mode(7)
///     .open(r"\\.\C:")?;
///
/// println!("{}", storage_device_number(&volume)?);
/// # Ok::<(), std::io::Error>(())
/// ```
///
/// # Errors
///
/// The device's own error, such as `ERROR_INVALID_FUNCTION` or `ERROR_INVALID_PARAMETER` for a handle
/// that is not a volume or a disk, and [`io::ErrorKind::InvalidData`] for an answer too short to hold
/// the structure.
pub fn storage_device_number(device: &impl AsHandle) -> io::Result<StorageDeviceNumber> {
    let mut buffer = [0u8; size_of::<STORAGE_DEVICE_NUMBER>()];
    let written = device_control(device, IOCTL_STORAGE_GET_DEVICE_NUMBER, &[], &mut buffer)?;
    StorageDeviceNumber::decode(&buffer[..written])
}
