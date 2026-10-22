// Copyright (c) 2026 Mike Grier
//! [`OpenedDisk`]: a whole physical disk this crate opened, addressed directly rather than through
//! a partition or a volume (DI-D-62).
//!
//! Where an [`OpenedVolume`](super::OpenedVolume) is part of a disk, found from the volume's
//! extents, an `OpenedDisk` is the disk: its path is its number, so which disk it sits on is known
//! by construction and needs no control to find it, and its one flush domain is that disk's. It is
//! opened here for the reason a file and a volume are: the warranty a target can claim follows from
//! how the handle was opened, and nothing documented reports that afterwards.
//!
//! Windows documents that it blocks a direct write to a sector of a disk that a mounted volume
//! covers, so a disk with volumes on it is written through them, or taken offline, or has them
//! locked first; a disk with no volume on it needs none of that. The tests here use a disk with no
//! partition and no volume, so that blocking is documented and not exercised by them.

use std::fmt;
use std::io;
use std::marker::PhantomData;
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, RawHandle};

use win_shared_os_owned_handle::SharedHandle;
use win_volume_sys::bytes_per_sector;

use super::opened_file::{Buffering, Unbuffered};
use super::opened_volume::disk_flush_domain;
use super::raw_open;
use super::share_mode::ShareMode;
use durable_ioring::{DataAccess, FlushDomain, Geometry, ReadWrite};

#[cfg(test)]
mod tests;

/// A path naming a physical disk by its number, `\\.\PhysicalDriveN`: the one form that does, so a
/// path that names a file, a volume or a share cannot be given where a disk is wanted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DiskPath(u32);

impl DiskPath {
    /// The disk numbered `disk_number`, as `diskmgmt` and `diskpart` number it.
    #[must_use]
    pub fn number(disk_number: u32) -> Self {
        Self(disk_number)
    }

    /// The disk's number.
    #[must_use]
    pub fn disk_number(&self) -> u32 {
        self.0
    }

    /// The path as the system takes it.
    #[must_use]
    pub fn path(&self) -> String {
        format!(r"\\.\PhysicalDrive{}", self.0)
    }
}

impl fmt::Display for DiskPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.path())
    }
}

/// How a disk is opened beyond its path: what other opens of the disk may request meanwhile, as
/// [`VolumeOptions`](super::VolumeOptions) carries for a volume.
#[derive(Clone, Copy, Debug)]
#[must_use]
pub struct DiskOptions {
    share: ShareMode,
}

impl Default for DiskOptions {
    fn default() -> Self {
        Self::new()
    }
}

impl DiskOptions {
    /// Nothing requested beyond opening the disk, which is shared for reading, writing and
    /// deletion, as `std::fs::OpenOptions` opens one.
    pub fn new() -> Self {
        Self {
            share: ShareMode::ALL,
        }
    }

    /// What other opens of the disk may request while this handle is open (`dwShareMode`), as
    /// [`FileOptions::share`](super::FileOptions::share) is for a file. [`ShareMode::NONE`] opens the
    /// disk exclusively. An open the sharing of either side does not allow fails with a
    /// [`SharingViolation`](super::SharingViolation). Calling this again replaces the mode.
    pub fn share(mut self, share: ShareMode) -> Self {
        self.share = share;
        self
    }
}

/// A whole physical disk this crate opened, in buffering mode `B` ([`Unbuffered`] unless said
/// otherwise) and with data access `A` ([`ReadWrite`] unless said otherwise): a [`SharedHandle`],
/// its number and the sector it is addressed in. Cloning shares the one handle.
///
/// It is what [`Win32FileTarget::from_disk`](super::Win32FileTarget::from_disk) is made from, when
/// `A` has a data right. The data access is the one the open was given, which no argument carries, so
/// it is named in the type at the call: `OpenedDisk::<Unbuffered, ReadOnly>::open(path, options)`.
#[derive(Clone, Debug)]
pub struct OpenedDisk<B: Buffering = Unbuffered, A: DataAccess = ReadWrite> {
    handle: SharedHandle,
    geometry: Geometry,
    disk_number: u32,
    mode: PhantomData<(B, A)>,
}

impl<B: Buffering, A: DataAccess> OpenedDisk<B, A> {
    /// Open `path` with data access `A` in buffering mode `B`, and find its sector size.
    ///
    /// Opening a disk for writing needs an account that is an administrator. Opening it with
    /// [`NoData`](super::NoData) asks for no data right, which is enough to query the disk, and the
    /// result cannot be made into a target.
    ///
    /// # Errors
    ///
    /// Whatever the open reports, such as `ERROR_FILE_NOT_FOUND` for a number no disk has, a
    /// [`SharingViolation`](super::SharingViolation) among it, and the device's own error from the
    /// sector size control.
    pub fn open(path: DiskPath, options: DiskOptions) -> io::Result<Self> {
        let file = raw_open::open::<B, A>(path.path(), options.share)?;
        let sector = bytes_per_sector(&file)?;
        let geometry = B::sector_geometry(sector)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        Ok(Self {
            handle: SharedHandle::from(file),
            geometry,
            disk_number: path.disk_number(),
            mode: PhantomData,
        })
    }

    /// The disk's handle, owned and shared.
    #[must_use]
    pub fn handle(&self) -> &SharedHandle {
        &self.handle
    }

    /// The alignment its operations need: its sector size.
    #[must_use]
    pub fn geometry(&self) -> Geometry {
        self.geometry
    }

    /// The disk's number.
    #[must_use]
    pub fn disk_number(&self) -> u32 {
        self.disk_number
    }

    /// The disk's flush domain: itself.
    #[must_use]
    pub fn flush_domain(&self) -> FlushDomain {
        disk_flush_domain(self.disk_number)
    }

    /// A disk over `handle`, with the parts a test gives it, so that what is derived from a disk
    /// can be tried without opening one, which needs an administrator.
    #[cfg(test)]
    pub(crate) fn assembled(handle: SharedHandle, disk_number: u32, sector: u32) -> Self {
        Self {
            handle,
            geometry: B::sector_geometry(sector).expect("a power of two"),
            disk_number,
            mode: PhantomData,
        }
    }
}

impl<B: Buffering, A: DataAccess> AsHandle for OpenedDisk<B, A> {
    fn as_handle(&self) -> BorrowedHandle<'_> {
        self.handle.as_handle()
    }
}

impl<B: Buffering, A: DataAccess> AsRawHandle for OpenedDisk<B, A> {
    fn as_raw_handle(&self) -> RawHandle {
        self.handle.as_raw_handle()
    }
}
