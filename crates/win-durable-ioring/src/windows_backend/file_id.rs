// Copyright (c) 2026 Mike Grier
//! A file's identity, read from a handle to it (DI-15.1, DI-D-69).
//!
//! The identity is what `GetFileInformationByHandleEx` reports for its `FileIdInfo` class: the serial
//! number of the volume the file is on and a 128-bit file identifier. The `FILE_ID_INFO` page says that
//! "the file identifier and the volume serial number uniquely identify a file on a single computer".
//! The call is the request crate's `QueryFileIdInfo`, given the handle shared and not a duplicate of it.
//!
//! The 128-bit form is the one this class reports. The page for `FILE_ID_DESCRIPTOR`, which
//! `OpenFileById` takes, lists it as `ExtendedFileIdType`, beside the 64-bit `FileIdType` and
//! `ObjectIdType`, and says it is "used on ReFS file systems"; an NTFS file's 16 bytes were observed
//! to open it as that type (`D-31` in the request crate's design notes).
//!
//! [`FileIdentifier`] is what an open by identifier takes: a [`FileId`], or one of the page's two other
//! kinds (DI-15.2, DI-D-70).

use std::io;
use std::ops::Range;

use win_shared_os_owned_handle::SharedHandle;
use windows_namespace_request_sys::file_id::{FileIdInfo, QueryFileIdInfo};
use windows_namespace_request_sys::open_by_id::FileIdentifier as RequestIdentifier;

use super::{Buffering, DataAccess, OpenedFile};

#[cfg(test)]
mod tests;

/// The identity of a file on one computer: the serial number of its volume and its 128-bit file
/// identifier, as `GetFileInformationByHandleEx`'s `FileIdInfo` class reported them.
///
/// Two values are equal when both fields are. What the fields mean beyond that is the call's: the
/// identifier is carried as its 16 bytes and is not read as an integer, which would impose a byte
/// order the call does not state.
///
/// # Example
///
/// ```
/// use win_durable_ioring::windows_backend::{Cached, FileOptions, OpenedFile};
/// use win_durable_ioring::Wtf16String;
///
/// let path = std::env::temp_dir().join(format!("file-id-doc-{}.tmp", std::process::id()));
/// std::fs::write(&path, b"x")?;
/// let name = || Wtf16String::from_os_str(path.as_os_str());
///
/// let first = OpenedFile::<Cached, _>::open(name(), FileOptions::new().read())?;
/// let second = OpenedFile::<Cached, _>::open(name(), FileOptions::new().read().write())?;
///
/// // Two opens of one file report one identity.
/// assert_eq!(first.file_id()?, second.file_id()?);
/// # drop((first, second));
/// # std::fs::remove_file(&path)?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FileId {
    volume_serial_number: u64,
    id: [u8; 16],
}

impl FileId {
    /// An identity with these fields, for a caller that holds them from elsewhere, such as a
    /// directory listing.
    #[must_use]
    pub const fn new(volume_serial_number: u64, id: [u8; 16]) -> Self {
        Self {
            volume_serial_number,
            id,
        }
    }

    /// The identity of the file `handle` is open on, read on the calling thread through the handle,
    /// which is shared for the call and not duplicated.
    ///
    /// # Errors
    ///
    /// The code the call failed with, unaltered. An event, a pipe, `NUL` and a volume were observed
    /// refused; the `FILE_ID_INFO` page does not list the codes, so none is promised.
    pub fn of(handle: &SharedHandle) -> io::Result<Self> {
        QueryFileIdInfo::new(handle.clone())
            .perform()
            .map(Self::from_info)
            .map_err(|error| error.to_io_error())
    }

    /// `VolumeSerialNumber`: the serial number of the volume that contains the file.
    #[must_use]
    pub const fn volume_serial_number(&self) -> u64 {
        self.volume_serial_number
    }

    /// `FileId`: the 128-bit file identifier, as its 16 bytes.
    #[must_use]
    pub const fn id(&self) -> [u8; 16] {
        self.id
    }

    /// The request crate's value as this crate's, field for field.
    fn from_info(info: FileIdInfo) -> Self {
        Self::new(info.volume_serial_number(), info.file_id())
    }
}

/// What an open by identifier names the file by: one of the three kinds the `FILE_ID_DESCRIPTOR` page
/// lists. Each is opened on the volume of the hint the open is given, which is the only volume the
/// call is told of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FileIdentifier {
    /// `ExtendedFileIdType`: a [`FileId`]'s 128-bit identifier. A `FileId` also names a volume, which
    /// the call does not take, so the opened file's identity is read and must be this one, volume
    /// serial included; a hint on another volume is refused with
    /// [`IdentityMismatch`](super::IdentityMismatch) and not answered with another volume's file.
    Extended(FileId),
    /// `FileIdType`: a 64-bit file ID, which `GetFileInformationByHandle` reports as
    /// `nFileIndexHigh` and `nFileIndexLow`. It names no volume, so there is nothing to check.
    FileId64(u64),
    /// `ObjectIdType`: an object ID, as the 16 bytes of the `GUID` in the order Windows stores one in
    /// memory, which is how `FSCTL_CREATE_OR_GET_OBJECT_ID` reports it (`FILE_OBJECTID_BUFFER`'s
    /// `ObjectId`) and how `fsutil objectid` prints it. It names no volume.
    ObjectId([u8; 16]),
}

impl From<FileId> for FileIdentifier {
    fn from(id: FileId) -> Self {
        Self::Extended(id)
    }
}

impl FileIdentifier {
    /// The identifier as the request crate's entry takes it.
    pub(super) fn to_request(self) -> RequestIdentifier {
        match self {
            Self::Extended(id) => RequestIdentifier::ExtendedFileId(id.id()),
            Self::FileId64(id) => RequestIdentifier::FileId(id),
            Self::ObjectId(bytes) => RequestIdentifier::ObjectId(guid_value(bytes)),
        }
    }
}

/// Where each field of a `GUID` lies in its 16 bytes, as Windows lays one out: `Data1`, a `u32`, and
/// `Data2` and `Data3`, each a `u16`, stored little-endian, then `Data4`, eight bytes in order.
mod guid_layout {
    use std::ops::Range;

    pub const DATA1: Range<usize> = 0..4;
    pub const DATA2: Range<usize> = 4..6;
    pub const DATA3: Range<usize> = 6..8;
}

/// The `GUID` that `bytes` lay out, as the value the request crate's `ObjectId` takes: the `GUID`
/// read as one big-endian number in the order of its canonical text form, which is what
/// `GUID::from_u128` reads. The three little-endian fields are turned around and `Data4` is kept.
fn guid_value(bytes: [u8; 16]) -> u128 {
    let mut text_order = bytes;
    for field in [guid_layout::DATA1, guid_layout::DATA2, guid_layout::DATA3] {
        reverse(&mut text_order, field);
    }
    u128::from_be_bytes(text_order)
}

fn reverse(bytes: &mut [u8; 16], field: Range<usize>) {
    bytes[field].reverse();
}

impl<B: Buffering, A: DataAccess> OpenedFile<B, A> {
    /// The identity of the file, read through its handle on the calling thread, as [`FileId::of`]
    /// reads it. Any data access reads it, [`NoData`](super::NoData) included.
    ///
    /// # Errors
    ///
    /// Those of [`FileId::of`].
    pub fn file_id(&self) -> io::Result<FileId> {
        FileId::of(self.handle())
    }
}
