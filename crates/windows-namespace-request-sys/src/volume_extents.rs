// Copyright (c) Mike Grier.

//! The `IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS` entry (NR-2.2).
//!
//! A storage-locality entry, added after round one for durable-ioring's flush domains: the runs
//! of disk a volume is made of, which is how a volume handle is traced to the disks whose caches
//! its writes depend on. Each run names a disk by number, so a handle to `\\.\PhysicalDriveN`
//! for that number is what the per-disk entries take next.
//!
//! # The call reports a count, and the caller asks again
//!
//! The output is a count followed by that many extents. When the buffer has room for the count
//! but not for every extent, the call fails with `ERROR_MORE_DATA` **and writes the count**, so
//! the entry reads it, allocates for it and asks again, rather than handing a caller a raw code
//! it must interpret. The retry is bounded for the reason [`crate::final_path`]'s is: a volume
//! whose extents keep changing between two calls a microsecond apart is pathological, and an
//! unbounded loop would spin the worker a consumer moved this call onto.
//!
//! # What the answer is, and is not
//!
//! A buffer with no room for the count and one extent is refused by the volume device as
//! `ERROR_INVALID_PARAMETER`, not `ERROR_INSUFFICIENT_BUFFER` (measured on one machine, Windows
//! 11), which is why the first attempt always has room for at least one.
//!
//! The runs are reported in the order the device gives them and are not interpreted. A volume on
//! one disk has one run. A volume that spans or stripes disks has several, which a machine of
//! one disk cannot show, so the retry is tested against a device that answers as the real one
//! does, and the real device is tested for the single run it can give.
//!
//! A handle that is not a volume, or is a volume the device does not describe (a remote one),
//! fails with the device's own code, unaltered.
//!
//! The call goes through the crate's one `DeviceIoControl` helper (D-20).

use std::fmt;
use std::mem::offset_of;

use windows_sys::Win32::Foundation::ERROR_MORE_DATA;
use windows_sys::Win32::Storage::FileSystem::IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS;
use windows_sys::Win32::System::Ioctl::{DISK_EXTENT, VOLUME_DISK_EXTENTS};

use crate::device_control::{device_control, u32_at, zeroed_output};
use crate::handle::{CapturedHandle, HandleCaptureError};
use crate::outcome::{Outcome, Win32Error};

#[cfg(test)]
mod tests;

/// How many extents the first attempt has room for.
///
/// A volume on one disk has one run and a spanned or striped one a few, so the common case costs
/// one call; a larger answer is asked for again at the size the device reports. At least one
/// extent always fits, as the device requires.
const FIRST_EXTENTS: usize = 8;

/// How many times the buffer is grown before the answer is called unstable.
const MAX_ATTEMPTS: usize = 4;

/// One run of a volume on one disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DiskExtent {
    disk_number: u32,
    starting_offset: u64,
    length: u64,
}

impl DiskExtent {
    /// The disk's number, as in `\\.\PhysicalDriveN`.
    #[must_use]
    pub fn disk_number(&self) -> u32 {
        self.disk_number
    }

    /// Where on the disk the run starts, in bytes.
    #[must_use]
    pub fn starting_offset(&self) -> u64 {
        self.starting_offset
    }

    /// How long the run is, in bytes.
    #[must_use]
    pub fn length(&self) -> u64 {
        self.length
    }
}

impl fmt::Display for DiskExtent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "disk {} at {} for {} bytes",
            self.disk_number, self.starting_offset, self.length
        )
    }
}

/// Why a volume's disk extents could not be read.
#[derive(Debug)]
#[non_exhaustive]
pub enum DiskExtentsError {
    /// Windows refused the call, with the raw code unaltered.
    ///
    /// `ERROR_NOT_ENOUGH_MEMORY` is also reported this way when the buffer for a count the device
    /// gave could not be allocated, in which case the call was not made again.
    Win32(Win32Error),
    /// The call succeeded, or reported a count, but the bytes do not hold what they say.
    ///
    /// Reported and not read, because extents taken from a buffer that does not hold them would
    /// read as disks the volume does not have.
    Malformed {
        /// What was wrong with the answer.
        what: &'static str,
    },
    /// The count kept changing, so the retry was abandoned.
    Unstable {
        /// How many attempts were made before giving up.
        attempts: usize,
    },
}

impl fmt::Display for DiskExtentsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Win32(error) => write!(f, "IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS: {error}"),
            Self::Malformed { what } => {
                write!(f, "IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS: {what}")
            }
            Self::Unstable { attempts } => write!(
                f,
                "IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS: the extent count changed on each of {attempts} attempts"
            ),
        }
    }
}

impl std::error::Error for DiskExtentsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Win32(error) => Some(error),
            Self::Malformed { .. } | Self::Unstable { .. } => None,
        }
    }
}

impl From<Win32Error> for DiskExtentsError {
    fn from(error: Win32Error) -> Self {
        Self::Win32(error)
    }
}

/// An owned, marshalable parameter set for `DeviceIoControl` with
/// `IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS`.
///
/// # Example
///
/// The handle names a volume, and needs no access to it, which an ordinary account can open:
///
/// ```no_run
/// use std::fs::OpenOptions;
/// use std::os::windows::fs::OpenOptionsExt;
/// use std::os::windows::io::AsHandle;
///
/// use windows_namespace_request_sys::volume_extents::QueryVolumeDiskExtents;
/// use windows_namespace_request_sys::CapturedHandle;
///
/// let volume = OpenOptions::new()
///     .access_mode(0)
///     .share_mode(7)
///     .open(r"\\.\C:")?;
///
/// let extents = QueryVolumeDiskExtents::new(CapturedHandle::capture(volume.as_handle())?)
///     .perform()?;
/// for extent in &extents {
///     println!("{extent}");
/// }
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug)]
#[must_use = "an unperformed request queries nothing"]
pub struct QueryVolumeDiskExtents {
    handle: CapturedHandle,
}

impl QueryVolumeDiskExtents {
    /// Begins a request against `handle`, which names a volume.
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

    /// Performs the query on the calling thread, asking again at the reported count if the first
    /// buffer was too small.
    ///
    /// # Errors
    ///
    /// [`DiskExtentsError::Win32`] with the raw code, unaltered; [`DiskExtentsError::Malformed`]
    /// for an answer that does not hold the extents it counts; [`DiskExtentsError::Unstable`] if
    /// the count kept changing.
    pub fn perform(&self) -> Result<Vec<DiskExtent>, DiskExtentsError> {
        extents_via(FIRST_EXTENTS, |output| {
            device_control(
                self.handle.raw(),
                IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS,
                &[],
                output,
            )
        })
    }
}

/// The extents `ask` gives, which fills the buffer it is lent and says how many bytes it wrote, or
/// reports `ERROR_MORE_DATA` with the count in the buffer's header when it does not fit.
///
/// Separate from the call so that a volume with more extents than the first answer holds, which a
/// machine of one disk cannot show, can be had from a test.
fn extents_via(
    first: usize,
    mut ask: impl FnMut(&mut [u8]) -> Outcome<u32>,
) -> Result<Vec<DiskExtent>, DiskExtentsError> {
    let mut capacity = first.max(1);

    for _ in 0..MAX_ATTEMPTS {
        let mut buffer = zeroed(answer_size(capacity)?)?;

        match ask(&mut buffer) {
            Ok(written) => return decode(&buffer, written),
            Err(error) if error.code() == ERROR_MORE_DATA => {
                // The header is written even when the extents do not fit: it says how many.
                let needed = count(&buffer).ok_or(DiskExtentsError::Malformed {
                    what: "a reply that needs more room carries no count",
                })?;
                capacity = usize::try_from(needed)
                    .map_err(|_| DiskExtentsError::Malformed {
                        what: "a count too large to hold",
                    })?
                    .max(1);
            }
            Err(error) => return Err(error.into()),
        }
    }

    Err(DiskExtentsError::Unstable {
        attempts: MAX_ATTEMPTS,
    })
}

/// A zeroed buffer of `size` bytes. The count is the device's, so it is not trusted to be
/// allocatable.
fn zeroed(size: usize) -> Result<Vec<u8>, DiskExtentsError> {
    zeroed_output(size).map_err(DiskExtentsError::Win32)
}

/// The size of an answer holding `extents` extents.
fn answer_size(extents: usize) -> Result<usize, DiskExtentsError> {
    extents
        .checked_mul(size_of::<DISK_EXTENT>())
        .and_then(|runs| runs.checked_add(offset_of!(VOLUME_DISK_EXTENTS, Extents)))
        .ok_or(DiskExtentsError::Malformed {
            what: "a count too large to hold",
        })
}

fn i64_at(buffer: &[u8], offset: usize) -> Option<i64> {
    let bytes = buffer.get(offset..offset.checked_add(size_of::<i64>())?)?;
    Some(i64::from_ne_bytes(bytes.try_into().ok()?))
}

/// How many extents an answer says it holds.
fn count(buffer: &[u8]) -> Option<u32> {
    u32_at(buffer, offset_of!(VOLUME_DISK_EXTENTS, NumberOfDiskExtents))
}

/// The extents in the first `written` bytes of `buffer`, checked against what the answer says it
/// holds: a count the bytes cannot hold is refused, so a damaged answer never reads outside what
/// the device wrote.
fn decode(buffer: &[u8], written: u32) -> Result<Vec<DiskExtent>, DiskExtentsError> {
    let malformed = |what| DiskExtentsError::Malformed { what };

    let written = usize::try_from(written)
        .ok()
        .filter(|written| *written <= buffer.len())
        .ok_or(malformed(
            "a call that wrote more than it was given room for",
        ))?;
    let buffer = &buffer[..written];

    let declared = count(buffer).ok_or(malformed("an answer shorter than its count"))?;
    let declared = usize::try_from(declared).map_err(|_| malformed("a count too large to hold"))?;
    if answer_size(declared)? > buffer.len() {
        return Err(malformed("an answer shorter than the extents it counts"));
    }

    let first = offset_of!(VOLUME_DISK_EXTENTS, Extents);
    (0..declared)
        .map(|index| {
            let at = first + index * size_of::<DISK_EXTENT>();
            let disk_number = u32_at(buffer, at + offset_of!(DISK_EXTENT, DiskNumber));
            let start = i64_at(buffer, at + offset_of!(DISK_EXTENT, StartingOffset));
            let length = i64_at(buffer, at + offset_of!(DISK_EXTENT, ExtentLength));
            match (disk_number, start, length) {
                (Some(disk_number), Some(start), Some(length)) => Ok(DiskExtent {
                    disk_number,
                    starting_offset: u64::try_from(start)
                        .map_err(|_| malformed("an extent that starts before the disk"))?,
                    length: u64::try_from(length)
                        .map_err(|_| malformed("an extent of negative length"))?,
                }),
                _ => Err(malformed("an extent cut short")),
            }
        })
        .collect()
}

impl crate::request::Request for QueryVolumeDiskExtents {
    type Error = DiskExtentsError;
    type Output = Vec<DiskExtent>;

    fn perform(&self) -> Result<Vec<DiskExtent>, DiskExtentsError> {
        Self::perform(self)
    }
}
