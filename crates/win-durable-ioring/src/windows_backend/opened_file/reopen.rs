// Copyright (c) 2026 Mike Grier
//! A reopen, through `ReOpenFile` (DI-15.4, DI-D-71): a new handle to what a handle already names, in
//! a buffering mode, with an access and a sharing this crate states anew.
//!
//! The original is any handle, an [`OpenedFile`]'s or one opened elsewhere (`DI-D-68`): the new
//! handle's mode is this crate's choice and is recorded by its type, so the original's provenance is
//! not what the result claims. What a reopen cannot convert, such as a handle opened with sharing that
//! refuses the reopen's access, is the kernel's refusal; the residue that matters is `DI-10.6`.
//!
//! The options are a type of their own, [`ReopenOptions`]: the page lists no disposition and says the
//! flags cannot include attributes. The checks made on the reopened handle are the path open's, by
//! the same function (DI-14.5), and here they can be reached: the original may be on a share, which
//! the page lists as supported, or may be a device.

use std::io;
use std::marker::PhantomData;

use win_shared_os_owned_handle::SharedHandle;
use windows_namespace_request_sys::reopen::ReOpenFile;
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_DELETE_ON_CLOSE, FILE_FLAG_OPEN_NO_RECALL, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_FLAG_OVERLAPPED, FILE_FLAG_POSIX_SEMANTICS, FILE_FLAG_RANDOM_ACCESS,
    FILE_FLAG_SEQUENTIAL_SCAN,
};

use durable_ioring::{DataAccess, NoData, ReadWrite};

use super::{Buffering, Cached, OpenedBy, OpenedFile, data_access, data_rights, refine_sharing_by};
use crate::windows_backend::file_rights::FileRights;
use crate::windows_backend::share_mode::ShareMode;

#[cfg(test)]
mod tests;

/// What to reopen with: the data access, the other rights, what other opens may do meanwhile, and the
/// flags `ReOpenFile` takes that an open by path also offers. A reopen is shared for reading, writing
/// and deletion until [`share`](Self::share) says otherwise, as [`FileOptions`](super::FileOptions)
/// shares an open.
///
/// There is no disposition, since a reopen reaches only what is open, and no attributes, which the
/// page says cannot be given. `FILE_FLAG_BACKUP_SEMANTICS` and `FILE_FLAG_WRITE_THROUGH`, which the
/// page lists, are not in v1, as for an open by path; the named-pipe security flags it lists for a
/// pipe's client are not offered either, since what this reopens is a file.
///
/// The share mode is judged against every handle still open to the file, **the original among them**:
/// a reopen that does not share what the original asked for, or asks for what the original does not
/// share, is refused, as the page says.
///
/// ```
/// use win_durable_ioring::windows_backend::{ReadOnly, ReopenOptions};
///
/// let options: ReopenOptions<ReadOnly> = ReopenOptions::new().read();
/// ```
#[derive(Clone, Copy, Debug)]
#[must_use]
pub struct ReopenOptions<A: DataAccess = NoData> {
    access: PhantomData<A>,
    rights: FileRights,
    share: ShareMode,
    open_reparse_point: bool,
    delete_on_close: bool,
    open_no_recall: bool,
    posix_semantics: bool,
    overlapped: bool,
    random_access: bool,
    sequential_scan: bool,
}

impl Default for ReopenOptions<NoData> {
    fn default() -> Self {
        Self::new()
    }
}

impl ReopenOptions<NoData> {
    /// Nothing requested: no data access and no other right, shared for everything.
    pub fn new() -> Self {
        Self {
            access: PhantomData,
            rights: FileRights::NONE,
            share: ShareMode::ALL,
            open_reparse_point: false,
            delete_on_close: false,
            open_no_recall: false,
            posix_semantics: false,
            overlapped: false,
            random_access: false,
            sequential_scan: false,
        }
    }
}

impl<A: DataAccess> ReopenOptions<A> {
    /// Ask for the right to read the data, as [`FileOptions::read`](super::FileOptions::read) does.
    pub fn read(self) -> ReopenOptions<A::WithRead> {
        self.with_access()
    }

    /// Ask for the right to write the data, as [`FileOptions::write`](super::FileOptions::write) does.
    pub fn write(self) -> ReopenOptions<A::WithWrite> {
        self.with_access()
    }

    /// The rights besides the data access, as [`FileOptions::rights`](super::FileOptions::rights)
    /// takes them. Calling this again replaces them.
    pub fn rights(mut self, rights: FileRights) -> Self {
        self.rights = rights;
        self
    }

    /// What other opens may request while this handle is open (`dwShareMode`), as
    /// [`FileOptions::share`](super::FileOptions::share) takes it. Calling this again replaces it.
    pub fn share(mut self, share: ShareMode) -> Self {
        self.share = share;
        self
    }

    /// Reopen a reparse point itself (`FILE_FLAG_OPEN_REPARSE_POINT`). The page says the flag is
    /// ignored for a file that is not a reparse point.
    pub fn open_reparse_point(mut self, open_reparse_point: bool) -> Self {
        self.open_reparse_point = open_reparse_point;
        self
    }

    /// Delete the file when its last handle is closed (`FILE_FLAG_DELETE_ON_CLOSE`), as
    /// [`FileOptions::delete_on_close`](super::FileOptions::delete_on_close) describes. The page says
    /// the file goes when all of its handles are closed, the original's and its duplicates' too.
    pub fn delete_on_close(mut self, delete_on_close: bool) -> Self {
        self.delete_on_close = delete_on_close;
        self
    }

    /// Ask that the data not be brought back from remote storage (`FILE_FLAG_OPEN_NO_RECALL`), as
    /// [`FileOptions::open_no_recall`](super::FileOptions::open_no_recall) does.
    pub fn open_no_recall(mut self, open_no_recall: bool) -> Self {
        self.open_no_recall = open_no_recall;
        self
    }

    /// Access the file by POSIX rules (`FILE_FLAG_POSIX_SEMANTICS`), as
    /// [`FileOptions::posix_semantics`](super::FileOptions::posix_semantics) describes.
    pub fn posix_semantics(mut self, posix_semantics: bool) -> Self {
        self.posix_semantics = posix_semantics;
        self
    }

    /// Reopen for overlapped I/O (`FILE_FLAG_OVERLAPPED`), as
    /// [`FileOptions::overlapped`](super::FileOptions::overlapped) describes.
    pub fn overlapped(mut self, overlapped: bool) -> Self {
        self.overlapped = overlapped;
        self
    }

    /// The random-access hint (`FILE_FLAG_RANDOM_ACCESS`), as
    /// [`FileOptions::random_access`](super::FileOptions::random_access) describes.
    pub fn random_access(mut self, random_access: bool) -> Self {
        self.random_access = random_access;
        self
    }

    /// The sequential-scan hint (`FILE_FLAG_SEQUENTIAL_SCAN`), as
    /// [`FileOptions::sequential_scan`](super::FileOptions::sequential_scan) describes.
    pub fn sequential_scan(mut self, sequential_scan: bool) -> Self {
        self.sequential_scan = sequential_scan;
        self
    }

    fn with_access<A2: DataAccess>(self) -> ReopenOptions<A2> {
        ReopenOptions {
            access: PhantomData,
            rights: self.rights,
            share: self.share,
            open_reparse_point: self.open_reparse_point,
            delete_on_close: self.delete_on_close,
            open_no_recall: self.open_no_recall,
            posix_semantics: self.posix_semantics,
            overlapped: self.overlapped,
            random_access: self.random_access,
            sequential_scan: self.sequential_scan,
        }
    }

    /// `dwDesiredAccess`: the data rights `A` names and the other rights.
    fn access(self) -> u32 {
        data_rights::<A>() | self.rights.bits()
    }

    /// `dwFlagsAndAttributes`: `mode`, the buffering mode's flags, and the flags the options name.
    fn flags(self, mode: u32) -> u32 {
        let mut flags = mode;
        if self.open_reparse_point {
            flags |= FILE_FLAG_OPEN_REPARSE_POINT;
        }
        if self.delete_on_close {
            flags |= FILE_FLAG_DELETE_ON_CLOSE;
        }
        if self.open_no_recall {
            flags |= FILE_FLAG_OPEN_NO_RECALL;
        }
        if self.posix_semantics {
            flags |= FILE_FLAG_POSIX_SEMANTICS;
        }
        if self.overlapped {
            flags |= FILE_FLAG_OVERLAPPED;
        }
        if self.random_access {
            flags |= FILE_FLAG_RANDOM_ACCESS;
        }
        if self.sequential_scan {
            flags |= FILE_FLAG_SEQUENTIAL_SCAN;
        }
        flags
    }
}

/// The request to reopen a handle, built by [`OpenedFile::request_reopen`] and made by
/// [`perform`](Self::perform). It holds the original, shared, so the handle it was built from can be
/// closed first and the file stays reachable; it is `Send`, so it can be performed on a thread that
/// can afford to block; and it can be performed more than once, each time making a new handle.
#[derive(Clone, Debug)]
#[must_use = "an unperformed request reopens nothing"]
pub struct ReopenRequest<B: Buffering = Cached, A: DataAccess = ReadWrite> {
    request: ReOpenFile,
    rights: FileRights,
    share: ShareMode,
    mode: PhantomData<fn() -> (B, A)>,
}

impl<B: Buffering, A: DataAccess> ReopenRequest<B, A> {
    /// The handle being reopened, shared with its other holders.
    #[must_use]
    pub fn original(&self) -> &SharedHandle {
        self.request.original()
    }

    /// Reopen, on the calling thread. **This blocks for as long as `ReOpenFile` does.**
    ///
    /// # Errors
    ///
    /// - [`SharingViolation`](super::SharingViolation) (as an [`io::ErrorKind::ResourceBusy`] error),
    ///   naming the file by [`OpenedBy::Reopen`], when the reopen's access or sharing conflicts with
    ///   a handle still open to the file, the original among them.
    /// - The path open's checks, made on the reopened handle by the same code:
    ///   [`RemoteDevice`](super::RemoteDevice) when `B` is unbuffered and the original is on a share,
    ///   and [`UnknownLocation`](super::UnknownLocation) when `B` is unbuffered and the reopened
    ///   handle has no final path, as a device's does not.
    /// - Otherwise whatever `ReOpenFile` reports, unaltered. The page says the original must have been
    ///   created by `CreateFile`, and lists no codes.
    pub fn perform(&self) -> io::Result<OpenedFile<B, A>> {
        let handle = self.request.perform().map_err(|error| {
            refine_sharing_by(
                error.to_io_error(),
                || OpenedBy::Reopen,
                data_access::<A>(),
                self.rights,
                self.share,
            )
        })?;
        OpenedFile::admit(SharedHandle::from(handle))
    }
}

impl<B: Buffering, A: DataAccess> OpenedFile<B, A> {
    /// Reopen what `original` names, in buffering mode `B` and as `options` say:
    /// [`request_reopen`](Self::request_reopen) and then [`perform`](ReopenRequest::perform), one after
    /// the other on this thread.
    ///
    /// The original is any handle: an `OpenedFile`'s, passed as `file.handle().clone()`, or one opened
    /// elsewhere. The result is a new file of its own type, with the alignment of `B` and not the
    /// original's: an unbuffered file starts at [`Unbuffered::DEFAULT_SECTOR_SIZE`](super::Unbuffered)
    /// again, whatever sector size the original was given.
    ///
    /// ```
    /// use win_durable_ioring::windows_backend::{
    ///     Cached, FileOptions, OpenedFile, ReadOnly, ReopenOptions, Unbuffered,
    /// };
    /// use win_durable_ioring::Wtf16String;
    ///
    /// let path = std::env::temp_dir().join(format!("reopen-doc-{}.tmp", std::process::id()));
    /// std::fs::write(&path, [0u8; 4096])?;
    /// let name = Wtf16String::from_os_str(path.as_os_str());
    /// let writer = OpenedFile::<Cached, _>::open(name, FileOptions::new().read().write())?;
    ///
    /// // A read-only, unbuffered handle beside the writer, for a scrubber.
    /// let scrubber = OpenedFile::<Unbuffered, ReadOnly>::reopen(
    ///     writer.handle().clone(),
    ///     ReopenOptions::new().read(),
    /// )?;
    /// assert_eq!(scrubber.file_id()?, writer.file_id()?);
    /// # drop((writer, scrubber));
    /// # std::fs::remove_file(&path)?;
    /// # Ok::<(), std::io::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Those of [`perform`](ReopenRequest::perform).
    pub fn reopen(
        original: impl Into<SharedHandle>,
        options: ReopenOptions<A>,
    ) -> io::Result<Self> {
        Self::request_reopen(original, options).perform()
    }

    /// The request to reopen what `original` names, built here and made wherever the caller chooses
    /// with [`perform`](ReopenRequest::perform). Nothing is opened and nothing blocks. The original is
    /// held, shared, and not duplicated.
    pub fn request_reopen(
        original: impl Into<SharedHandle>,
        options: ReopenOptions<A>,
    ) -> ReopenRequest<B, A> {
        let request = ReOpenFile::new(original)
            .with_desired_access(options.access())
            .with_share_mode(options.share.bits())
            .with_flags_and_attributes(options.flags(B::FLAGS));
        ReopenRequest {
            request,
            rights: options.rights,
            share: options.share,
            mode: PhantomData,
        }
    }
}
