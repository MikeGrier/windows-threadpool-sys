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
use std::fs;
use std::io;
use std::marker::PhantomData;
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, RawHandle};
use std::path::{Component, Path, PathBuf, Prefix};

use win_shared_os_owned_handle::SharedHandle;
use windows_sys::Win32::Foundation::ERROR_SHARING_VIOLATION;
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_FLAG_RANDOM_ACCESS, FILE_FLAG_SEQUENTIAL_SCAN,
    FILE_READ_DATA, FILE_WRITE_DATA,
};
use wtf_string::{Wtf16Str, Wtf16String};

use durable_ioring::{
    Access, Capabilities, DataAccess, Geometry, GeometryError, NoData, ReadWrite, WarrantyClass,
};

use super::disposition::Disposition;
use super::file_rights::FileRights;
use super::share_mode::ShareMode;

mod open_request;

#[cfg(test)]
mod share_tests;
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

    /// The alignment a raw volume or disk in this mode needs, addressed in sectors of `sector_size`
    /// bytes. Its transfers are whole sectors in either mode, so offsets and lengths are aligned to
    /// one; its buffers only when it is unbuffered, as the cache is what lets a cached transfer
    /// start anywhere.
    ///
    /// # Errors
    ///
    /// [`GeometryError`] if `sector_size` is not a power of two.
    fn sector_geometry(sector_size: u32) -> Result<Geometry, GeometryError>;
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

    fn sector_geometry(sector_size: u32) -> Result<Geometry, GeometryError> {
        Geometry::new(sector_size, sector_size, 1)
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

    fn sector_geometry(sector_size: u32) -> Result<Geometry, GeometryError> {
        Geometry::uniform(sector_size)
    }
}

/// What to open: the data access, the other rights, what other opens may do meanwhile, what the open
/// does about a file that is there or not, whether a link is opened or followed, and the cache hints.
/// Only these, because the open mode is what an [`OpenedFile`] vouches for, and a caller's own flags
/// could change it. A file is shared for reading, writing and deletion, as std opens one, until
/// [`share`](Self::share) says otherwise.
///
/// The data access is the type parameter `A`: it starts as [`NoData`], and [`read`](Self::read) and
/// [`write`](Self::write) move it, so the handle an open makes carries the access it was asked
/// for.
///
/// ```
/// use win_durable_ioring::windows_backend::{FileOptions, FileRights, ReadWrite};
///
/// let options: FileOptions<ReadWrite> = FileOptions::new()
///     .read()
///     .write()
///     .rights(FileRights::WRITE_ATTRIBUTES);
/// ```
#[derive(Clone, Copy, Debug)]
#[must_use]
pub struct FileOptions<A: DataAccess = NoData> {
    access: PhantomData<A>,
    rights: FileRights,
    share: ShareMode,
    disposition: Disposition,
    open_reparse_point: bool,
    random_access: bool,
    sequential_scan: bool,
}

impl Default for FileOptions<NoData> {
    fn default() -> Self {
        Self::new()
    }
}

impl FileOptions<NoData> {
    /// Nothing requested: no data access and no other right. Ask to [`read`](Self::read) and
    /// [`write`](Self::write) before opening a file that is to become a target.
    pub fn new() -> Self {
        Self {
            access: PhantomData,
            rights: FileRights::NONE,
            share: ShareMode::ALL,
            disposition: Disposition::OpenExisting,
            open_reparse_point: false,
            random_access: false,
            sequential_scan: false,
        }
    }
}

impl<A: DataAccess> FileOptions<A> {
    /// Ask for the right to read the data (`FILE_READ_DATA`). The options carry the access this adds
    /// to what they already had: [`NoData`] becomes [`ReadOnly`](super::ReadOnly) and
    /// [`WriteOnly`](super::WriteOnly) becomes [`ReadWrite`].
    pub fn read(self) -> FileOptions<A::WithRead> {
        self.with_access()
    }

    /// Ask for the right to write the data (`FILE_WRITE_DATA`), as [`read`](Self::read) asks to read.
    pub fn write(self) -> FileOptions<A::WithWrite> {
        self.with_access()
    }

    /// The rights to open with besides the data access (`dwDesiredAccess`): the attribute, delete and
    /// security rights, which gate calls made on the handle this crate lends and not anything it
    /// does itself. What the file's security allows is the kernel's to say, and its refusal reaches
    /// the caller as its own error.
    ///
    /// Calling this again replaces the rights; join them with `|` to ask for several.
    pub fn rights(mut self, rights: FileRights) -> Self {
        self.rights = rights;
        self
    }

    /// What other opens of the file may request while this handle is open (`dwShareMode`). The
    /// default is [`ShareMode::ALL`]; [`ShareMode::NONE`] is an exclusive open, which a database that
    /// owns its file wants.
    ///
    /// An open the sharing of either side does not allow fails with a [`SharingViolation`], whichever
    /// of the two opens came first. Calling this again replaces the mode.
    pub fn share(mut self, share: ShareMode) -> Self {
        self.share = share;
        self
    }

    fn with_access<A2: DataAccess>(self) -> FileOptions<A2> {
        FileOptions {
            access: PhantomData,
            rights: self.rights,
            share: self.share,
            disposition: self.disposition,
            open_reparse_point: self.open_reparse_point,
            random_access: self.random_access,
            sequential_scan: self.sequential_scan,
        }
    }

    /// What the open does about a file that is there and one that is not (`dwCreationDisposition`).
    /// The default is [`Disposition::OpenExisting`]. Calling this again replaces the disposition.
    ///
    /// No disposition changes the contents of a file that exists, so none is refused for want of the
    /// right to write. A caller that wants an existing file emptied does it through the handle this
    /// crate lends, with `File::set_len(0)`, which needs the write right and is not atomic with the
    /// open.
    pub fn disposition(mut self, disposition: Disposition) -> Self {
        self.disposition = disposition;
        self
    }

    /// Open a link itself and not what it points to (`FILE_FLAG_OPEN_REPARSE_POINT`). Without it a
    /// link is followed, including by [`Disposition::CreateNew`], which then creates the file the
    /// link names. With it a link that is already there is what exists, so `CreateNew` fails and
    /// `OpenAlways` opens the link. It has no effect on a path that is not a reparse point.
    pub fn open_reparse_point(mut self, open_reparse_point: bool) -> Self {
        self.open_reparse_point = open_reparse_point;
        self
    }

    /// Hint that the file will be accessed at random (`FILE_FLAG_RANDOM_ACCESS`), which the system
    /// may use to tune its caching.
    ///
    /// It is passed to an unbuffered file as well as a cached one, and is never refused. The
    /// `CreateFileW` documentation says the flag "has no effect if the file system does not support
    /// cached I/O and `FILE_FLAG_NO_BUFFERING`", which can be read two ways, so what it does to an
    /// unbuffered file is the kernel's to say. Measured on Windows 11 build 26300: every combination
    /// of the two hints, cached and unbuffered, opens and carries a sector-aligned write and read,
    /// and an unbuffered one also runs through the ring. Whether a hint changes the system's caching
    /// was not measured, and nothing about the warranty class, the alignment or write-through
    /// depends on it. Asking for this and [`sequential_scan`](Self::sequential_scan) together is
    /// the documentation's "self-defeating", and is passed on all the same.
    pub fn random_access(mut self, random_access: bool) -> Self {
        self.random_access = random_access;
        self
    }

    /// Hint that the file will be read sequentially from beginning to end
    /// (`FILE_FLAG_SEQUENTIAL_SCAN`), which the system may use to tune its caching. The
    /// documentation says it should not be used if reverse scans will be.
    ///
    /// Passed through and documented as [`random_access`](Self::random_access) is.
    pub fn sequential_scan(mut self, sequential_scan: bool) -> Self {
        self.sequential_scan = sequential_scan;
        self
    }

    /// What this asks of `CreateFileW`, with `flags` added to the flags it implies. Every
    /// combination of the options is a request, so nothing here is refused.
    fn parameters(self, flags: u32) -> open_request::Parameters {
        let reparse = if self.open_reparse_point {
            FILE_FLAG_OPEN_REPARSE_POINT
        } else {
            0
        };
        let mut hints = 0;
        if self.random_access {
            hints |= FILE_FLAG_RANDOM_ACCESS;
        }
        if self.sequential_scan {
            hints |= FILE_FLAG_SEQUENTIAL_SCAN;
        }
        open_request::Parameters {
            access: data_rights::<A>() | self.rights.bits(),
            disposition: self.disposition.bits(),
            flags: flags | reparse | hints,
            share: self.share.bits(),
        }
    }
}

/// What `A` asks `CreateFileW` for: `FILE_READ_DATA` as it reads and `FILE_WRITE_DATA` as it writes,
/// and no other right. The append bit is never among them, so a handle that appends and cannot write
/// at an offset cannot be asked for. The markers are the core's (DI-D-66); what they ask of Win32 is
/// stated here.
pub(super) const fn data_rights<A: DataAccess>() -> u32 {
    (if A::READ { FILE_READ_DATA } else { 0 }) | (if A::WRITE { FILE_WRITE_DATA } else { 0 })
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

/// An open failed because of how the file is already open: either this open asked for an access that
/// an open already holding the file does not share, or it did not share an access that open asked for.
/// `CreateFileW` fails both ways with `ERROR_SHARING_VIOLATION` and says nothing of which, and no
/// documented call reports the other open's access or sharing, so this carries what *this* open asked
/// for and nothing about the other.
///
/// It is the cause of an [`io::ErrorKind::ResourceBusy`] error from [`OpenedFile::open`], reached with
/// [`io::Error::get_ref`] and a downcast. The kernel's own error, which carries the Win32 code, is its
/// [`source`](std::error::Error::source).
#[derive(Debug)]
pub struct SharingViolation {
    /// The path the open was made at, as it was given.
    pub path: PathBuf,
    /// The data access the open asked for, or `None` for one that asked for no data right.
    pub data: Option<Access>,
    /// The rights the open asked for besides the data access.
    pub rights: FileRights,
    /// What this open allowed other opens to request.
    pub share: ShareMode,
    os_error: io::Error,
}

impl fmt::Display for SharingViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} is open in a way that conflicts with this open (data access {}, {:?}, sharing {:?}): \
             the other open does not share what this one asked for, or this one does not share what \
             the other holds",
            self.path.display(),
            self.data
                .map_or_else(|| "none".to_owned(), |data| data.to_string()),
            self.rights,
            self.share,
        )
    }
}

impl std::error::Error for SharingViolation {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.os_error)
    }
}

/// The data access `A` asks for, as a value, or `None` for one that asks for no data right.
pub(super) fn data_access<A: DataAccess>() -> Option<Access> {
    match (A::READ, A::WRITE) {
        (false, false) => None,
        (true, false) => Some(Access::ReadOnly),
        (false, true) => Some(Access::WriteOnly),
        (true, true) => Some(Access::ReadWrite),
    }
}

/// `error`, or the [`SharingViolation`] it is if the kernel said so.
pub(super) fn refine_sharing(
    error: io::Error,
    path: &Wtf16Str,
    data: Option<Access>,
    rights: FileRights,
    share: ShareMode,
) -> io::Error {
    if error.raw_os_error() != Some(ERROR_SHARING_VIOLATION.cast_signed()) {
        return error;
    }
    io::Error::new(
        io::ErrorKind::ResourceBusy,
        SharingViolation {
            path: PathBuf::from(path.to_os_string()),
            data,
            rights,
            share,
            os_error: error,
        },
    )
}

/// A file this crate opened, in buffering mode `B` and with data access `A`: a [`SharedHandle`] and
/// what is known of it because of how it was opened. Cloning shares the one handle.
///
/// It is what a [`Win32FileTarget`](super::Win32FileTarget) is made from, and the only thing it
/// can be made from; the module documentation says why. A target needs a data right, so a file
/// opened with [`NoData`] cannot be made into one.
///
/// A consumer that needs the stronger warranty says so in its signature, and a cached file cannot
/// be passed:
///
/// ```compile_fail,E0308
/// use win_durable_ioring::Wtf16String;
/// use win_durable_ioring::windows_backend::{Cached, FileOptions, OpenedFile, Unbuffered};
///
/// fn needs_an_unbuffered_file(_: OpenedFile<Unbuffered>) {}
///
/// let options = FileOptions::new().read().write();
/// let name = Wtf16String::from_os_str("a-file".as_ref());
/// let cached = OpenedFile::<Cached>::open(name, options).unwrap();
/// needs_an_unbuffered_file(cached);
/// ```
///
/// Nor can one opened for reading only be given where reading and writing are asked for:
///
/// ```compile_fail,E0308
/// use win_durable_ioring::Wtf16String;
/// use win_durable_ioring::windows_backend::{Cached, FileOptions, OpenedFile, ReadWrite};
///
/// fn needs_both(_: OpenedFile<Cached, ReadWrite>) {}
///
/// let name = Wtf16String::from_os_str("a-file".as_ref());
/// let read_only = OpenedFile::<Cached, _>::open(name, FileOptions::new().read()).unwrap();
/// needs_both(read_only);
/// ```
///
/// The data access is a type parameter after the buffering mode, and a parameter written in a path
/// takes its default, [`ReadWrite`], when it is left off, so a file opened with other options names it
/// or leaves it to be inferred with `_`: `OpenedFile::<Cached, _>::open(path, options)`.
#[derive(Clone, Debug)]
pub struct OpenedFile<B: Buffering = Cached, A: DataAccess = ReadWrite> {
    handle: SharedHandle,
    geometry: Geometry,
    mode: PhantomData<(B, A)>,
}

impl<B: Buffering, A: DataAccess> OpenedFile<B, A> {
    /// Open `path` as `options` says, in buffering mode `B`. The handle has the data access the
    /// options were given, which is its type `A`.
    ///
    /// The path is handed to Win32 as it was given: this crate does not lengthen it, shorten it or
    /// refuse it, so a path longer than `MAX_PATH` without a verbatim prefix, and an empty one, are
    /// what `CreateFileW` makes of them. A relative path is fixed to the current directory as it is
    /// when this is called, by the request crate. Convert a `Path` with
    /// [`Wtf16String::from_os_str`], which loses nothing.
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::InvalidInput`] if `path` has an interior NUL, before anything is touched.
    /// Otherwise whatever the open reports: [`SharingViolation`] (as an
    /// [`io::ErrorKind::ResourceBusy`] error) when the file is open in a way that conflicts with
    /// this open's access or sharing, and [`RemoteDevice`] (as an [`io::ErrorKind::Unsupported`]
    /// error) when `B` can claim its warranty only on a local device and `path` is on a remote one.
    /// That is decided from the path's final location, which resolves links and mapped drives,
    /// before anything is created; the file can still be replaced by a link in the time between.
    pub fn open(path: Wtf16String, options: FileOptions<A>) -> io::Result<Self> {
        let (rights, share) = (options.rights, options.share);
        let parameters = options.parameters(B::FLAGS);
        if B::LOCAL_DEVICE_ONLY {
            // The one place a `Path` is needed, until `DI-14.5` moves the check to the opened handle.
            let location = final_location(Path::new(&path.to_os_string()))?;
            if names_a_remote_device(&location) {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    RemoteDevice { location },
                ));
            }
        }
        let given = path.clone();
        let handle = open_request::open(path, parameters)
            .map_err(|error| refine_sharing(error, &given, data_access::<A>(), rights, share))?;
        Ok(Self {
            handle: SharedHandle::from(handle),
            geometry: B::geometry(),
            mode: PhantomData,
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

impl<A: DataAccess> OpenedFile<Unbuffered, A> {
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

impl<B: Buffering, A: DataAccess> AsHandle for OpenedFile<B, A> {
    fn as_handle(&self) -> BorrowedHandle<'_> {
        self.handle.as_handle()
    }
}

impl<B: Buffering, A: DataAccess> AsRawHandle for OpenedFile<B, A> {
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
