// Copyright (c) 2026 Mike Grier
//! The disks a volume sits on: `IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS`.

use std::io;
use std::mem::{offset_of, size_of};
use std::os::windows::io::AsHandle;

use win_error_codes::Win32Error;
use windows_sys::Win32::Foundation::ERROR_MORE_DATA;
use windows_sys::Win32::Storage::FileSystem::IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS;
use windows_sys::Win32::System::Ioctl::{DISK_EXTENT, VOLUME_DISK_EXTENTS};

use crate::device_control::{device_control, u32_at, zeroed_output};

#[cfg(test)]
mod tests;

/// How many extents the first call has room for. A volume on one disk has one, and a spanned or
/// striped one a few; a larger answer is asked for again at the size the device reports.
const FIRST_EXTENTS: usize = 8;

/// How many times a volume's answer is asked for again at a larger size, which only a volume
/// whose extents change between calls would need more than once.
const ATTEMPTS: usize = 4;

/// One run of a volume on one disk, as `IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS` reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DiskExtent {
    /// The disk's number, as in `\\.\PhysicalDriveN`.
    pub disk_number: u32,
    /// Where on the disk the run starts, in bytes.
    pub starting_offset: u64,
    /// How long the run is, in bytes.
    pub length: u64,
}

/// The runs on disks that make up `volume`, in the order the device reports them.
///
/// `volume` is a handle to a volume -- `\\.\C:` or `\\?\Volume{...}` -- and needs no access to it:
/// a handle opened with none is enough, which an ordinary account can open. A volume on a remote
/// system, or anything that is not a volume, does not answer, so success is also what shows a
/// handle names a volume on this machine's own disks.
///
/// # Errors
///
/// The device's own error, such as `ERROR_INVALID_FUNCTION` for a handle that is not a volume, and
/// [`io::ErrorKind::InvalidData`] for an answer that does not hold the extents it counts, a call
/// that claims to have written more than the buffer it was given, a negative offset or length, and
/// a volume whose extents kept changing between the calls. An allocation that cannot be made for a
/// count the device named is `ERROR_NOT_ENOUGH_MEMORY`, and the call is not made again.
pub fn volume_disk_extents(volume: &impl AsHandle) -> io::Result<Vec<DiskExtent>> {
    extents_via(FIRST_EXTENTS, |buffer| {
        device_control(volume, IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, &[], buffer)
    })
}

/// The extents `ask` gives, which fills the buffer it is lent and says how many bytes it wrote, and
/// reports `ERROR_MORE_DATA` with the count in the buffer's header when it does not fit. The first
/// answer has room for `first` extents, and always for one, as the device needs. Separate from the
/// call so that a volume with more extents than the first answer holds, which one disk's volume
/// never is, can be had from a test.
fn extents_via(
    first: usize,
    mut ask: impl FnMut(&mut [u8]) -> io::Result<usize>,
) -> io::Result<Vec<DiskExtent>> {
    let mut capacity = first.max(1);
    for _ in 0..ATTEMPTS {
        let mut buffer = zeroed_output(answer_size(capacity)?)?;
        match ask(&mut buffer) {
            Ok(written) => {
                // A byte count beyond the buffer is the call's claim and not a fact.
                let written = buffer
                    .get(..written)
                    .ok_or_else(|| invalid("a call that wrote more than it was given room for"))?;
                return parse(written);
            }
            Err(error) if is_more_data(&error) => {
                // The header is written even when the extents do not fit: it says how many. A
                // count of none still leaves room for the one extent the device requires.
                let needed = count(&buffer).ok_or_else(|| invalid("an answer with no count"))?;
                capacity = usize::try_from(needed)
                    .map_err(|_| invalid("a count too large"))?
                    .max(1);
            }
            Err(error) => return Err(error),
        }
    }
    Err(invalid("a volume whose extents kept changing"))
}

/// Whether `error` is the device's `ERROR_MORE_DATA`: the buffer held the count and not the extents.
fn is_more_data(error: &io::Error) -> bool {
    Win32Error::from_io(error).is_some_and(|code| code.code() == ERROR_MORE_DATA)
}

/// The size of an answer holding `extents` extents.
fn answer_size(extents: usize) -> io::Result<usize> {
    extents
        .checked_mul(size_of::<DISK_EXTENT>())
        .and_then(|runs| runs.checked_add(offset_of!(VOLUME_DISK_EXTENTS, Extents)))
        .ok_or_else(|| invalid("a count too large"))
}

fn invalid(what: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what)
}

fn i64_at(buffer: &[u8], offset: usize) -> Option<i64> {
    let bytes = buffer.get(offset..offset.checked_add(size_of::<i64>())?)?;
    Some(i64::from_ne_bytes(bytes.try_into().ok()?))
}

/// How many extents an answer says it holds.
fn count(buffer: &[u8]) -> Option<u32> {
    u32_at(buffer, offset_of!(VOLUME_DISK_EXTENTS, NumberOfDiskExtents))
}

/// The extents in an answer, checked against what the answer says it holds: a count the bytes
/// cannot hold is refused, so a damaged answer never reads outside its buffer.
fn parse(buffer: &[u8]) -> io::Result<Vec<DiskExtent>> {
    let declared = count(buffer).ok_or_else(|| invalid("an answer shorter than its count"))?;
    let declared = usize::try_from(declared).map_err(|_| invalid("a count too large"))?;
    if answer_size(declared)? > buffer.len() {
        return Err(invalid("an answer shorter than the extents it counts"));
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
                        .map_err(|_| invalid("a negative offset"))?,
                    length: u64::try_from(length).map_err(|_| invalid("a negative length"))?,
                }),
                _ => Err(invalid("an extent cut short")),
            }
        })
        .collect()
}
