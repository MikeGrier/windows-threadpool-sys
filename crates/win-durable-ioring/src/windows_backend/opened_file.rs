// Copyright (c) 2026 Mike Grier
//! [`OpenedFile`]: a file this crate opened, so how it was opened is known by construction
//! (DI-D-60).
//!
//! A [`Win32FileTarget`](super::Win32FileTarget) states a warranty class, an alignment and what it
//! offers, and every one of them depends on how its file was opened: no intermediate buffering
//! is what lets a file claim [`WarrantyClass::SharedDevice`], and what makes its operations need a
//! sector's alignment. A handle someone else opened cannot be asked, because the only call that
//! reports the open mode is undocumented, which this repository does not call. So the file is
//! opened here, by a type that records what it did, and a target can be made from nothing else.
//!
//! The buffering mode is a type: [`OpenedFile<Cached>`] or [`OpenedFile<Unbuffered>`]. What a mode
//! implies is stated once, on [`Buffering`]; a consumer that needs the stronger warranty asks for
//! `OpenedFile<Unbuffered>` in its own signature, and a cached file cannot be passed.

use std::fmt;
use std::fs::{self, OpenOptions};
use std::io;
use std::marker::PhantomData;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, RawHandle};
use std::path::{Component, Path, PathBuf, Prefix};

use win_shared_os_owned_handle::SharedHandle;

use durable_ioring::{Capabilities, Geometry, GeometryError, WarrantyClass};

#[cfg(test)]
mod tests;

/// `FILE_FLAG_NO_BUFFERING`, as `CreateFileW` documents it: the file is opened without the system
/// cache, so every transfer needs the device's alignment.
const FILE_FLAG_NO_BUFFERING: u32 = 0x2000_0000;

mod sealed {
    pub trait Sealed {}
}

/// How a file is buffered, which decides what a target made from it can claim. Sealed: there are
/// two modes, [`Cached`] and [`Unbuffered`], and a warranty is a statement about each of them that
/// this crate makes.
pub trait Buffering: sealed::Sealed + Copy + fmt::Debug + Send + Sync + 'static {
    /// The `CreateFileW` flags this mode opens with.
    const FLAGS: u32;

    /// The warranty class a file in this mode can claim.
    const WARRANTY: WarrantyClass;

    /// What a file in this mode offers beyond the contract's obligations.
    const CAPABILITIES: Capabilities;

    /// Whether the claim in [`WARRANTY`](Self::WARRANTY) holds only on a device of this machine, so
    /// that a file on a remote one is refused rather than given a class it cannot keep.
    const LOCAL_DEVICE_ONLY: bool;

    /// The alignment a file in this mode starts with.
    fn geometry() -> Geometry;
}

/// A file opened for cached I/O: the system cache holds its writes, and its write-back can fail
/// without telling the writer, so it claims [`WarrantyClass::BestEffort`] and needs no alignment.
#[derive(Clone, Copy, Debug)]
pub struct Cached;

/// A file opened with no intermediate buffering: its transfers reach the device, so it can claim
/// [`WarrantyClass::SharedDevice`], and need the device's sector alignment. Per-write
/// write-through is honoured only on such a file, which the kernel refuses on a cached one
/// (error 509, measured 2026-10-07 and kept as a test), so it offers that capability.
#[derive(Clone, Copy, Debug)]
pub struct Unbuffered;

impl sealed::Sealed for Cached {}
impl sealed::Sealed for Unbuffered {}

impl Buffering for Cached {
    const FLAGS: u32 = 0;
    const WARRANTY: WarrantyClass = WarrantyClass::BestEffort;
    const CAPABILITIES: Capabilities = Capabilities::NONE;
    const LOCAL_DEVICE_ONLY: bool = false;

    fn geometry() -> Geometry {
        Geometry::UNALIGNED
    }
}

impl Unbuffered {
    /// The sector size an unbuffered file is given until [`OpenedFile::with_sector_size`] says
    /// otherwise: the largest in general use, which every smaller one divides, so no device
    /// refuses it. A caller whose device allows less can say so and be offered less alignment to
    /// meet.
    pub const DEFAULT_SECTOR_SIZE: u32 = 4096;
}

impl Buffering for Unbuffered {
    const FLAGS: u32 = FILE_FLAG_NO_BUFFERING;
    const WARRANTY: WarrantyClass = WarrantyClass::SharedDevice;
    const CAPABILITIES: Capabilities = Capabilities::NONE.with_write_through();
    const LOCAL_DEVICE_ONLY: bool = true;

    fn geometry() -> Geometry {
        Geometry::uniform(Self::DEFAULT_SECTOR_SIZE).expect("4096 is a power of two")
    }
}

/// What to open: access and creation, as `std::fs::OpenOptions` names them. Only these, because
/// the open mode is what an [`OpenedFile`] vouches for, and a caller's own flags could change it.
/// A file is shared for reading, writing and deletion, as std opens one.
#[derive(Clone, Copy, Debug, Default)]
#[must_use]
pub struct FileOptions {
    read: bool,
    write: bool,
    create: bool,
    create_new: bool,
    truncate: bool,
}

impl FileOptions {
    /// Nothing requested: set at least read or write before opening.
    pub fn new() -> Self {
        Self::default()
    }

    /// Open for reading.
    pub fn read(mut self, read: bool) -> Self {
        self.read = read;
        self
    }

    /// Open for writing.
    pub fn write(mut self, write: bool) -> Self {
        self.write = write;
        self
    }

    /// Create the file if it does not exist.
    pub fn create(mut self, create: bool) -> Self {
        self.create = create;
        self
    }

    /// Create the file, and fail if it exists.
    pub fn create_new(mut self, create_new: bool) -> Self {
        self.create_new = create_new;
        self
    }

    /// Truncate the file to nothing on opening.
    pub fn truncate(mut self, truncate: bool) -> Self {
        self.truncate = truncate;
        self
    }

    fn open_options(self) -> OpenOptions {
        let mut options = OpenOptions::new();
        options
            .read(self.read)
            .write(self.write)
            .create(self.create)
            .create_new(self.create_new)
            .truncate(self.truncate);
        options
    }
}

/// An unbuffered file was asked for on a device of another machine, which cannot keep the claim
/// that makes a file unbuffered: what it reports is the remote system's, not the device's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteDevice {
    /// The path the file would have been opened at, resolved through any link or mapped drive.
    pub location: PathBuf,
}

impl fmt::Display for RemoteDevice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} is on a remote device, which cannot be opened unbuffered with the warranty that implies",
            self.location.display()
        )
    }
}

impl std::error::Error for RemoteDevice {}

/// A file this crate opened, in buffering mode `B`: a [`SharedHandle`] and what is known of it
/// because of how it was opened. Cloning shares the one handle.
///
/// It is what a [`Win32FileTarget`](super::Win32FileTarget) is made from, and the only thing it
/// can be made from; the module documentation says why.
///
/// A consumer that needs the stronger warranty says so in its signature, and a cached file cannot
/// be passed:
///
/// ```compile_fail,E0308
/// use win_durable_ioring::windows_backend::{Cached, FileOptions, OpenedFile, Unbuffered};
///
/// fn needs_an_unbuffered_file(_: OpenedFile<Unbuffered>) {}
///
/// let cached = OpenedFile::<Cached>::open("a-file", FileOptions::new().read(true)).unwrap();
/// needs_an_unbuffered_file(cached);
/// ```
#[derive(Clone, Debug)]
pub struct OpenedFile<B: Buffering = Cached> {
    handle: SharedHandle,
    geometry: Geometry,
    buffering: PhantomData<B>,
}

impl<B: Buffering> OpenedFile<B> {
    /// Open `path` as `options` says, in buffering mode `B`.
    ///
    /// # Errors
    ///
    /// Whatever the open reports, and [`RemoteDevice`] (as an [`io::ErrorKind::Unsupported`] error)
    /// when `B` can claim its warranty only on a local device and `path` is on a remote one. That
    /// is decided from the path's final location, which resolves links and mapped drives, before
    /// anything is created; the file can still be replaced by a link in the time between.
    pub fn open(path: impl AsRef<Path>, options: FileOptions) -> io::Result<Self> {
        let path = path.as_ref();
        if B::LOCAL_DEVICE_ONLY {
            let location = final_location(path)?;
            if names_a_remote_device(&location) {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    RemoteDevice { location },
                ));
            }
        }
        let file = options.open_options().custom_flags(B::FLAGS).open(path)?;
        Ok(Self {
            handle: SharedHandle::from(file),
            geometry: B::geometry(),
            buffering: PhantomData,
        })
    }

    /// The file's handle, owned and shared.
    #[must_use]
    pub fn handle(&self) -> &SharedHandle {
        &self.handle
    }

    /// The alignment its operations need.
    #[must_use]
    pub fn geometry(&self) -> Geometry {
        self.geometry
    }
}

impl OpenedFile<Unbuffered> {
    /// State the device's sector size, for a device whose sectors are smaller than
    /// [`Unbuffered::DEFAULT_SECTOR_SIZE`]. A size that is too small for the device is refused by
    /// the device on the first transfer that needs more, as an error on that operation's
    /// completion, never as silent damage.
    ///
    /// # Errors
    ///
    /// [`GeometryError`] if `sector_size` is not a power of two.
    pub fn with_sector_size(mut self, sector_size: u32) -> Result<Self, GeometryError> {
        self.geometry = Geometry::uniform(sector_size)?;
        Ok(self)
    }
}

impl<B: Buffering> AsHandle for OpenedFile<B> {
    fn as_handle(&self) -> BorrowedHandle<'_> {
        self.handle.as_handle()
    }
}

impl<B: Buffering> AsRawHandle for OpenedFile<B> {
    fn as_raw_handle(&self) -> RawHandle {
        self.handle.as_raw_handle()
    }
}

/// Where `path` finally lands: itself if it exists, else the directory it would be created in.
fn final_location(path: &Path) -> io::Result<PathBuf> {
    match fs::canonicalize(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let parent = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            fs::canonicalize(parent)
        }
        other => other,
    }
}

/// Whether a resolved path names a device of another machine: a UNC share, in either of its
/// forms. A mapped drive and a link to a share resolve to one, so they are seen as well.
fn names_a_remote_device(location: &Path) -> bool {
    matches!(
        location.components().next(),
        Some(Component::Prefix(prefix))
            if matches!(prefix.kind(), Prefix::UNC(..) | Prefix::VerbatimUNC(..))
    )
}
