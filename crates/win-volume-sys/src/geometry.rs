// Copyright (c) 2026 Mike Grier
//! The sector size a volume is addressed in: `IOCTL_DISK_GET_DRIVE_GEOMETRY`.

use std::io;
use std::mem::{offset_of, size_of};
use std::os::windows::io::AsHandle;

use windows_sys::Win32::System::Ioctl::{DISK_GEOMETRY, IOCTL_DISK_GET_DRIVE_GEOMETRY};

use crate::control::control;

#[cfg(test)]
mod tests;

/// The size in bytes of the sector `volume` is addressed in, which a transfer to an unbuffered
/// volume handle must be a multiple of in offset and length, and aligned to in memory.
///
/// `volume` is a handle to a volume and needs no access to it, as for
/// [`volume_disk_extents`](crate::volume_disk_extents).
///
/// # Errors
///
/// The device's own error, and [`io::ErrorKind::InvalidData`] for an answer too short to hold the
/// size, or one that says zero.
pub fn bytes_per_sector(volume: &impl AsHandle) -> io::Result<u32> {
    let mut buffer = [0u8; size_of::<DISK_GEOMETRY>()];
    let written = control(volume, IOCTL_DISK_GET_DRIVE_GEOMETRY, &mut buffer)?;
    sector_size(&buffer[..written])
}

/// The sector size in a `DISK_GEOMETRY` answer.
fn sector_size(answer: &[u8]) -> io::Result<u32> {
    let at = offset_of!(DISK_GEOMETRY, BytesPerSector);
    let bytes = answer
        .get(at..at + size_of::<u32>())
        .and_then(|bytes| <[u8; size_of::<u32>()]>::try_from(bytes).ok())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "an answer shorter than a geometry",
            )
        })?;
    match u32::from_ne_bytes(bytes) {
        0 => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "a geometry with no sector size",
        )),
        size => Ok(size),
    }
}
