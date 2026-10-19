// Copyright (c) 2026 Mike Grier
//! An open by identifier, through `OpenFileById` (DI-15.2, DI-D-70): the file is named by a
//! [`FileIdentifier`] on the volume of a hint, and not by a path.
//!
//! The options are a type of their own, [`ByIdOptions`], and not [`FileOptions`](super::FileOptions):
//! the call's page lists no creation disposition, takes no attributes, marks the security parameter
//! reserved, and lists eight flags, of which `FILE_FLAG_DELETE_ON_CLOSE` and
//! `FILE_FLAG_POSIX_SEMANTICS` are not two. What a handle this crate opened can claim depends on how
//! it was opened and not on how it was named, so the file is the same [`OpenedFile`], and the checks
//! made on the handle are the path open's (DI-14.5).
//!
//! The hint is any handle on the volume, whatever opened it: it is never the file opened, so its mode
//! reaches nothing the result claims (`DI-D-64`'s `hVolumeHint` row).

use std::fmt;
use std::io;
use std::marker::PhantomData;

use win_shared_os_owned_handle::SharedHandle;
use windows_namespace_request_sys::open_by_id::OpenFileByIdentifier;
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_OPEN_NO_RECALL, FILE_FLAG_OPEN_REPARSE_POINT, FILE_FLAG_OVERLAPPED,
    FILE_FLAG_RANDOM_ACCESS, FILE_FLAG_SEQUENTIAL_SCAN,
};

use durable_ioring::{DataAccess, NoData, ReadWrite};

use super::{Buffering, Cached, OpenedBy, OpenedFile, data_access, data_rights, refine_sharing_by};
use crate::windows_backend::file_id::{FileId, FileIdentifier};
use crate::windows_backend::file_rights::FileRights;
use crate::windows_backend::share_mode::ShareMode;

#[cfg(test)]
mod tests;

/// What to open by identifier: the data access, the other rights, what other opens may do meanwhile,
/// whether a link is opened or followed, and the hints, which are what `OpenFileById` takes. A file is
/// shared for reading, writing and deletion until [`share`](Self::share) says otherwise, as
/// [`FileOptions`](super::FileOptions) shares it.
///
/// There is no disposition, since the call opens only what exists; no attributes, which its page
/// says it ignores; no security descriptor, which it marks reserved; and no delete-on-close or POSIX
/// semantics, which it does not list. `FILE_FLAG_BACKUP_SEMANTICS`, which a directory needs, and
/// `FILE_FLAG_WRITE_THROUGH` are not in v1, as for an open by path.
///
/// ```
/// use win_durable_ioring::windows_backend::{ByIdOptions, ReadOnly, ShareMode};
///
/// let options: ByIdOptions<ReadOnly> = ByIdOptions::new().read().share(ShareMode::READ);
/// ```
#[derive(Clone, Copy, Debug)]
#[must_use]
pub struct ByIdOptions<A: DataAccess = NoData> {
    access: PhantomData<A>,
    rights: FileRights,
    share: ShareMode,
    open_reparse_point: bool,
    open_no_recall: bool,
    overlapped: bool,
    random_access: bool,
    sequential_scan: bool,
}

impl Default for ByIdOptions<NoData> {
    fn default() -> Self {
        Self::new()
    }
}

impl ByIdOptions<NoData> {
    /// Nothing requested: no data access and no other right, shared for everything.
    pub fn new() -> Self {
        Self {
            access: PhantomData,
            rights: FileRights::NONE,
            share: ShareMode::ALL,
            open_reparse_point: false,
            open_no_recall: false,
            overlapped: false,
            random_access: false,
            sequential_scan: false,
        }
    }
}

impl<A: DataAccess> ByIdOptions<A> {
    /// Ask for the right to read the data, as [`FileOptions::read`](super::FileOptions::read) does.
    pub fn read(self) -> ByIdOptions<A::WithRead> {
        self.with_access()
    }

    /// Ask for the right to write the data, as [`FileOptions::write`](super::FileOptions::write) does.
    pub fn write(self) -> ByIdOptions<A::WithWrite> {
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

    /// Open a reparse point itself and not what it points to (`FILE_FLAG_OPEN_REPARSE_POINT`). The
    /// page says the flag is ignored for a file that is not a reparse point.
    pub fn open_reparse_point(mut self, open_reparse_point: bool) -> Self {
        self.open_reparse_point = open_reparse_point;
        self
    }

    /// Ask that the data not be brought back from remote storage (`FILE_FLAG_OPEN_NO_RECALL`), as
    /// [`FileOptions::open_no_recall`](super::FileOptions::open_no_recall) does.
    pub fn open_no_recall(mut self, open_no_recall: bool) -> Self {
        self.open_no_recall = open_no_recall;
        self
    }

    /// Open for overlapped I/O (`FILE_FLAG_OVERLAPPED`), as
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

    fn with_access<A2: DataAccess>(self) -> ByIdOptions<A2> {
        ByIdOptions {
            access: PhantomData,
            rights: self.rights,
            share: self.share,
            open_reparse_point: self.open_reparse_point,
            open_no_recall: self.open_no_recall,
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
        if self.open_no_recall {
            flags |= FILE_FLAG_OPEN_NO_RECALL;
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

/// The request to open a file by identifier, built by [`OpenedFile::request_by_id`] and made by
/// [`perform`](Self::perform). It holds the hint, shared, so the handle it was built from can be
/// closed first; it is `Send`, so it can be performed on a thread that can afford to block; and it
/// can be performed more than once, each time opening the file again.
#[derive(Debug)]
#[must_use = "an unperformed request opens nothing"]
pub struct ByIdRequest<B: Buffering = Cached, A: DataAccess = ReadWrite> {
    request: OpenFileByIdentifier,
    identifier: FileIdentifier,
    rights: FileRights,
    share: ShareMode,
    mode: PhantomData<fn() -> (B, A)>,
}

impl<B: Buffering, A: DataAccess> ByIdRequest<B, A> {
    /// The identifier this request opens.
    #[must_use]
    pub fn identifier(&self) -> FileIdentifier {
        self.identifier
    }

    /// The hint whose volume the identifier is opened on, shared with its other holders.
    #[must_use]
    pub fn volume_hint(&self) -> &SharedHandle {
        self.request.volume_hint()
    }

    /// Open the file, on the calling thread. **This blocks for as long as `OpenFileById` does.**
    ///
    /// # Errors
    ///
    /// - [`SharingViolation`](super::SharingViolation) (as an [`io::ErrorKind::ResourceBusy`] error),
    ///   naming the file by [`OpenedBy::Identifier`], when the file is open in a way that conflicts
    ///   with this open's access or sharing.
    /// - For a [`FileIdentifier::Extended`], [`IdentityMismatch`] (as an
    ///   [`io::ErrorKind::InvalidInput`] error) when the file opened is not the identity asked for,
    ///   which is a hint on another volume; the handle is closed. An error reading the opened file's
    ///   identity is returned as it came.
    /// - The path open's checks, [`RemoteDevice`](super::RemoteDevice) and
    ///   [`UnknownLocation`](super::UnknownLocation), made on the opened handle by the same code. No
    ///   test reaches them by identifier: the page lists SMB as not supported, so an open by
    ///   identifier on a share is the kernel's to refuse, and the call opens only a file on a file
    ///   system, whose final path is readable.
    /// - Otherwise whatever `OpenFileById` reports, unaltered. The page names one, `ERROR_ACCESS_DENIED`
    ///   for a file pending deletion; an identifier that names nothing is the kernel's error, and the
    ///   page does not say which.
    pub fn perform(&self) -> io::Result<OpenedFile<B, A>> {
        let handle = self.request.perform().map_err(|error| {
            refine_sharing_by(
                error.to_io_error(),
                || OpenedBy::Identifier(self.identifier),
                data_access::<A>(),
                self.rights,
                self.share,
            )
        })?;
        let handle = SharedHandle::from(handle);
        if let FileIdentifier::Extended(asked) = self.identifier {
            let opened = FileId::of(&handle)?;
            if opened != asked {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    IdentityMismatch { asked, opened },
                ));
            }
        }
        OpenedFile::admit(handle)
    }
}

impl<B: Buffering, A: DataAccess> OpenedFile<B, A> {
    /// Open the file `identifier` names on the volume `volume_hint` is on, as `options` say, in
    /// buffering mode `B`: [`request_by_id`](Self::request_by_id) and then
    /// [`perform`](ByIdRequest::perform), one after the other on this thread.
    ///
    /// ```
    /// use win_durable_ioring::windows_backend::{ByIdOptions, Cached, FileOptions, OpenedFile};
    /// use win_durable_ioring::Wtf16String;
    ///
    /// let path = std::env::temp_dir().join(format!("by-id-doc-{}.tmp", std::process::id()));
    /// std::fs::write(&path, b"x")?;
    /// let name = Wtf16String::from_os_str(path.as_os_str());
    /// let file = OpenedFile::<Cached, _>::open(name, FileOptions::new().read())?;
    ///
    /// // Any handle on the volume is a hint; here, the file's own.
    /// let again = OpenedFile::<Cached, _>::open_by_id(
    ///     file.handle().clone(),
    ///     file.file_id()?,
    ///     ByIdOptions::new().read(),
    /// )?;
    /// assert_eq!(again.file_id()?, file.file_id()?);
    /// # drop((file, again));
    /// # std::fs::remove_file(&path)?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Those of [`perform`](ByIdRequest::perform).
    pub fn open_by_id(
        volume_hint: impl Into<SharedHandle>,
        identifier: impl Into<FileIdentifier>,
        options: ByIdOptions<A>,
    ) -> io::Result<Self> {
        Self::request_by_id(volume_hint, identifier, options).perform()
    }

    /// The request to open the file `identifier` names on the volume `volume_hint` is on, built here
    /// and made wherever the caller chooses with [`perform`](ByIdRequest::perform). Nothing is opened
    /// and nothing blocks. The hint is held, shared, and not duplicated.
    pub fn request_by_id(
        volume_hint: impl Into<SharedHandle>,
        identifier: impl Into<FileIdentifier>,
        options: ByIdOptions<A>,
    ) -> ByIdRequest<B, A> {
        let identifier = identifier.into();
        let request = OpenFileByIdentifier::new(volume_hint, identifier.to_request())
            .with_desired_access(options.access())
            .with_share_mode(options.share.bits())
            .with_flags_and_attributes(options.flags(B::FLAGS));
        ByIdRequest {
            request,
            identifier,
            rights: options.rights,
            share: options.share,
            mode: PhantomData,
        }
    }
}

/// An open by a [`FileIdentifier::Extended`] opened a file whose identity is not the one asked for:
/// the call takes the volume from the hint and not from the [`FileId`], so a hint on another volume
/// opens whatever has that identifier there, or nothing. The handle is closed.
///
/// It is the cause of an [`io::ErrorKind::InvalidInput`] error from [`OpenedFile::open_by_id`],
/// reached with [`io::Error::get_ref`] and a downcast.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IdentityMismatch {
    /// The identity the open was asked for.
    pub asked: FileId,
    /// The identity of the file it opened.
    pub opened: FileId,
}

impl fmt::Display for IdentityMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the open by identifier asked for {:?} and opened {:?}: the volume hint is not on the \
             volume the identity names",
            self.asked, self.opened
        )
    }
}

impl std::error::Error for IdentityMismatch {}
