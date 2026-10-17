// Copyright (c) 2026 Mike Grier
//! [`OpenedVolume`]: a raw volume this crate opened, so how it was opened, which disks it sits on
//! and the sector it is addressed in are known by construction (DI-D-61).
//!
//! A volume is a target whose regions are offsets the consumer allocates inside it: one handle, one
//! commit per volume, and every region shares the volume's flush domains. Those are found from the
//! volume's disks, which is this backing's own obligation (the contract's "Complete flush-domain
//! declarations"), by a documented device control, not declared by the consumer.
//!
//! The volume is opened here, by path, for the reason [`OpenedFile`](super::OpenedFile) is: the
//! warranty a target can claim follows from how the handle was opened, and nothing documented reports
//! it afterwards. Only the forms of path that name a volume are accepted, a drive letter or a volume
//! GUID, and a volume that does not answer the extents control -- a remote one, or not a volume --
//! is refused, so a [`Win32FileTarget`](super::Win32FileTarget) over one is over this machine's own
//! disks.

use std::fmt;
use std::io;
use std::marker::PhantomData;
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, RawHandle};

use win_shared_os_owned_handle::SharedHandle;
use win_volume_sys::{DiskExtent, bytes_per_sector, lock_volume, volume_disk_extents};

use super::opened_file::{Buffering, Unbuffered};
use super::raw_open;
use super::share_mode::ShareMode;
use durable_ioring::{DataAccess, FlushDomain, Geometry, ReadWrite};

#[cfg(test)]
mod tests;

/// The flush domain of disk `disk_number`: `\\.\PhysicalDriveN`, the name a disk answers to.
///
/// A raw volume derives one for each disk it sits on. A consumer declares the same for a file on
/// that disk (`Win32FileTarget::with_domains`), so a commit that fails on the volume reaches the
/// file's writes and the other way round: two targets share a domain exactly when their bytes
/// agree.
#[must_use]
pub fn disk_flush_domain(disk_number: u32) -> FlushDomain {
    FlushDomain::new(format!(r"\\.\PhysicalDrive{disk_number}").into_bytes())
}

/// A path naming a volume, in one of the two forms that do: a drive letter, `\\.\C:`, or a volume
/// GUID, `\\?\Volume{...}`. Nothing else is one, so a path that names a file, a directory or a
/// share cannot be given where a volume is wanted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VolumePath(String);

/// A volume path was asked for in a form that does not name a volume.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VolumePathError(String);

impl fmt::Display for VolumePathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:?} is not a drive letter or a volume GUID path",
            self.0
        )
    }
}

impl std::error::Error for VolumePathError {}

/// The text of a volume GUID path before the GUID, and after it.
const GUID_PATH_PREFIX: &str = r"\\?\Volume{";
const GUID_PATH_SUFFIX: char = '}';

/// The lengths of a GUID's five groups of hexadecimal digits, which dashes separate.
const GUID_GROUPS: [usize; 5] = [8, 4, 4, 4, 12];

impl VolumePath {
    /// The volume mounted at drive letter `letter`, `A` to `Z` in either case.
    ///
    /// # Errors
    ///
    /// [`VolumePathError`] for any other character.
    pub fn drive(letter: char) -> Result<Self, VolumePathError> {
        if letter.is_ascii_alphabetic() {
            Ok(Self(format!(r"\\.\{}:", letter.to_ascii_uppercase())))
        } else {
            Err(VolumePathError(letter.to_string()))
        }
    }

    /// The volume named by a volume GUID path, such as `\\?\Volume{01234567-89ab-cdef-0123-456789abcdef}`
    /// or the same with the trailing backslash the enumeration functions give it, which is dropped:
    /// a volume handle is opened without it.
    ///
    /// # Errors
    ///
    /// [`VolumePathError`] for any other form, including a GUID of the wrong shape.
    pub fn guid(path: &str) -> Result<Self, VolumePathError> {
        let refused = || VolumePathError(path.to_owned());
        let path = path.strip_suffix('\\').unwrap_or(path);
        let inner = path
            .strip_prefix(GUID_PATH_PREFIX)
            .and_then(|rest| rest.strip_suffix(GUID_PATH_SUFFIX))
            .ok_or_else(refused)?;
        let groups: Vec<&str> = inner.split('-').collect();
        let shaped = groups.len() == GUID_GROUPS.len()
            && groups.iter().zip(GUID_GROUPS).all(|(group, length)| {
                group.len() == length && group.chars().all(|c| c.is_ascii_hexdigit())
            });
        if shaped {
            Ok(Self(path.to_owned()))
        } else {
            Err(refused())
        }
    }

    /// The path as the system takes it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// How a volume is opened beyond its path: whether it is locked, and what other opens of the volume
/// may request meanwhile.
#[derive(Clone, Copy, Debug)]
#[must_use]
pub struct VolumeOptions {
    lock: bool,
    share: ShareMode,
}

impl Default for VolumeOptions {
    fn default() -> Self {
        Self::new()
    }
}

impl VolumeOptions {
    /// Nothing requested beyond opening the volume, which is shared for reading, writing and
    /// deletion, as `std::fs::OpenOptions` opens one.
    pub fn new() -> Self {
        Self {
            lock: false,
            share: ShareMode::ALL,
        }
    }

    /// What other opens of the volume may request while this handle is open (`dwShareMode`), as
    /// [`FileOptions::share`](super::FileOptions::share) is for a file. [`ShareMode::NONE`] opens the
    /// volume exclusively. An open the sharing of either side does not allow fails with a
    /// [`SharingViolation`](super::SharingViolation). Calling this again replaces the mode.
    ///
    /// This is not the lock: [`lock`](Self::lock) is a device control made once the volume is open,
    /// and the share mode is judged by the open itself.
    pub fn share(mut self, share: ShareMode) -> Self {
        self.share = share;
        self
    }

    /// Lock the volume once it is open, so that this handle is the only one that can use it until
    /// the handle closes. A file system on the volume is flushed and writes nothing more, which is
    /// what lets a raw write to its own sectors be made; a volume with no file system needs none.
    /// Opening fails, and nothing is held, if anything else has the volume open.
    ///
    /// The `FSCTL_LOCK_VOLUME` page names no right the handle needs, so the lock is offered with any
    /// data access and the kernel's error is the answer if one is needed; which it is was not
    /// measured, since it needs an administrator.
    pub fn lock(mut self, lock: bool) -> Self {
        self.lock = lock;
        self
    }
}

/// A raw volume this crate opened, in buffering mode `B` ([`Unbuffered`] unless said otherwise) and
/// with data access `A` ([`ReadWrite`] unless said otherwise): a [`SharedHandle`], the disks it sits
/// on, the sector it is addressed in and whether it is locked. Cloning shares the one handle.
///
/// It is what [`Win32FileTarget::from_volume`](super::Win32FileTarget::from_volume) is made from, when
/// `A` has a data right. The data access is the one the open was given, which `options` do not carry,
/// so it is named in the type at the call: `OpenedVolume::<Unbuffered, ReadOnly>::open(..)`.
#[derive(Clone, Debug)]
pub struct OpenedVolume<B: Buffering = Unbuffered, A: DataAccess = ReadWrite> {
    handle: SharedHandle,
    geometry: Geometry,
    extents: Vec<DiskExtent>,
    locked: bool,
    mode: PhantomData<(B, A)>,
}

impl<B: Buffering, A: DataAccess> OpenedVolume<B, A> {
    /// Open `path` with data access `A` in buffering mode `B`, find its disks and sector size, and
    /// lock it if `options` say so.
    ///
    /// Opening a volume for writing needs an account that is an administrator. Opening it with
    /// [`NoData`](super::NoData) asks for no data right, which is enough to query the volume, and the
    /// result cannot be made into a target.
    ///
    /// # Errors
    ///
    /// Whatever the open reports, a [`SharingViolation`](super::SharingViolation) among it, and the
    /// device's own error from the extents or sector size control: a volume that is remote, or a path that is not a volume, does not answer them, so
    /// it is refused here. When the lock is asked for, the lock's own error, and then nothing is
    /// held.
    pub fn open(path: &VolumePath, options: VolumeOptions) -> io::Result<Self> {
        let file = raw_open::open::<B, A>(path.as_str(), options.share)?;
        let extents = volume_disk_extents(&file)?;
        let sector = bytes_per_sector(&file)?;
        let geometry = B::sector_geometry(sector)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if options.lock {
            lock_volume(&file)?;
        }
        Ok(Self {
            handle: SharedHandle::from(file),
            geometry,
            extents,
            locked: options.lock,
            mode: PhantomData,
        })
    }

    /// The volume's handle, owned and shared.
    #[must_use]
    pub fn handle(&self) -> &SharedHandle {
        &self.handle
    }

    /// The alignment its operations need: its sector size.
    #[must_use]
    pub fn geometry(&self) -> Geometry {
        self.geometry
    }

    /// The runs on disks that make up the volume.
    #[must_use]
    pub fn disk_extents(&self) -> &[DiskExtent] {
        &self.extents
    }

    /// The flush domains of the volume: one for each disk it sits on, once each, in disk order.
    #[must_use]
    pub fn flush_domains(&self) -> Vec<FlushDomain> {
        domains_of(&self.extents)
    }

    /// Whether the volume was locked when it was opened.
    #[must_use]
    pub fn is_locked(&self) -> bool {
        self.locked
    }

    /// A volume over `handle`, with the parts a test gives it, so that what is derived from a
    /// volume can be tried without opening one, which needs an administrator.
    #[cfg(test)]
    pub(crate) fn assembled(
        handle: SharedHandle,
        extents: Vec<DiskExtent>,
        sector: u32,
        locked: bool,
    ) -> Self {
        Self {
            handle,
            geometry: B::sector_geometry(sector).expect("a power of two"),
            extents,
            locked,
            mode: PhantomData,
        }
    }
}

impl<B: Buffering, A: DataAccess> AsHandle for OpenedVolume<B, A> {
    fn as_handle(&self) -> BorrowedHandle<'_> {
        self.handle.as_handle()
    }
}

impl<B: Buffering, A: DataAccess> AsRawHandle for OpenedVolume<B, A> {
    fn as_raw_handle(&self) -> RawHandle {
        self.handle.as_raw_handle()
    }
}

/// One flush domain for each distinct disk in `extents`, in disk order.
fn domains_of(extents: &[DiskExtent]) -> Vec<FlushDomain> {
    let mut disks: Vec<u32> = extents.iter().map(|extent| extent.disk_number).collect();
    disks.sort_unstable();
    disks.dedup();
    disks.into_iter().map(disk_flush_domain).collect()
}
