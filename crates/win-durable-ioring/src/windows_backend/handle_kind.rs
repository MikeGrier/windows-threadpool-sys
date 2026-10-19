// Copyright (c) 2026 Mike Grier
//! What kind of handle an open handle is, asked of an observer (DI-14.12, DI-D-64, DI-D-67).
//!
//! The opener does not return the kind: a caller who wants it builds a [`HandleKindQuery`] from the
//! handle and performs it. It is the request crate's pattern, a request over a duplicate of the handle,
//! so it can be performed on any thread and does not hold the file open.
//!
//! The answer is what the handle answered and not a promise of what it is. Three queries are made, in
//! order, and each is made only if the one before leaves the question open: `GetFileType`, whose five
//! values are documented; `IOCTL_STORAGE_GET_DEVICE_NUMBER`, which the control's page says a volume or a
//! disk answers and which other handles were observed refusing; and `IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS`,
//! a volume control, which a physical disk was observed refusing. The pages do not list the errors a
//! handle of another kind gets, so a refusal is taken as a refusal whatever its code.

use std::io;
use std::os::windows::io::AsHandle;

use win_volume_sys::{DiskExtent, StorageDeviceNumber, storage_device_number, volume_disk_extents};
use windows_namespace_request_sys::{CapturedHandle, FileType, QueryFileType};

#[cfg(test)]
mod tests;

/// What a handle answered when it was asked its kind, by [`HandleKindQuery::perform`].
///
/// The variants name what answered. A handle that answered the same way as one of them and is something
/// else, because a device refused for a reason other than being the wrong kind, is that variant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HandleKind {
    /// `GetFileType` reported `FILE_TYPE_DISK` and the storage-device control refused the handle: a
    /// file or a directory on a file system, as far as these queries can tell.
    DiskFile,
    /// `GetFileType` reported `FILE_TYPE_DISK`, the storage-device control answered, and the
    /// volume-extents control answered: a volume, with the number it reported and the extents of the
    /// disks it is on.
    Volume {
        /// What the storage-device control reported, as the device wrote it.
        number: StorageDeviceNumber,
        /// What the volume-extents control reported: where the volume lies on its disks.
        extents: Vec<DiskExtent>,
    },
    /// `GetFileType` reported `FILE_TYPE_DISK`, the storage-device control answered, and the
    /// volume-extents control refused: a whole disk, with the number it reported.
    PhysicalDisk(StorageDeviceNumber),
    /// `GetFileType` reported `FILE_TYPE_CHAR`: "a character file, typically an LPT device or a
    /// console", and `NUL` was observed reporting it.
    CharacterDevice,
    /// `GetFileType` reported `FILE_TYPE_PIPE`: "a socket, a named pipe, or an anonymous pipe".
    Pipe,
    /// Any other value `GetFileType` returned, as it came, including `FILE_TYPE_REMOTE`, which the
    /// page calls unused, and an unknown type that was not a failure.
    Other(FileType),
}

/// A request for a handle's kind, built from the handle and made with [`perform`](Self::perform).
///
/// It owns a duplicate of the handle, so the handle it was built from can be closed first, and it is
/// `Send`, so it can be performed where the caller chooses.
///
/// # Example
///
/// ```
/// use win_durable_ioring::windows_backend::{
///     Cached, FileOptions, HandleKind, HandleKindQuery, OpenedFile,
/// };
/// use win_durable_ioring::Wtf16String;
///
/// let path = std::env::temp_dir().join(format!("kind-doc-{}.tmp", std::process::id()));
/// std::fs::write(&path, b"x")?;
/// let name = Wtf16String::from_os_str(path.as_os_str());
/// let file = OpenedFile::<Cached, _>::open(name, FileOptions::new().read().write())?;
///
/// let kind = HandleKindQuery::of(file.handle())?.perform()?;
/// assert_eq!(kind, HandleKind::DiskFile);
/// # drop(file);
/// # std::fs::remove_file(&path)?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug)]
#[must_use = "an unperformed query asks nothing"]
pub struct HandleKindQuery {
    file_type: QueryFileType,
}

impl HandleKindQuery {
    /// A query over a duplicate of `handle`, which is not changed and need not stay open.
    ///
    /// For an [`OpenedFile`](super::OpenedFile), `OpenedVolume` or `OpenedDisk`, pass its `handle()`.
    ///
    /// # Errors
    ///
    /// The error of duplicating the handle.
    pub fn of(handle: &impl AsHandle) -> io::Result<Self> {
        let captured = CapturedHandle::capture(handle.as_handle()).map_err(|error| {
            error
                .raw_os_error()
                .map_or_else(|| io::Error::other(error), io::Error::from_raw_os_error)
        })?;
        Ok(Self {
            file_type: QueryFileType::new(captured),
        })
    }

    /// Asks the handle its kind, on the calling thread.
    ///
    /// # Errors
    ///
    /// The code of `GetFileType` if it reported an unknown type with an error, and an out-of-memory
    /// error if the volume-extents control's reply was too large to hold. A refusal by either control
    /// is not an error: it is the answer, as the variants say.
    pub fn perform(&self) -> io::Result<HandleKind> {
        let handle = self.file_type.handle();
        let file_type = self
            .file_type
            .perform()
            .map_err(|error| error.to_io_error())?;
        classify(
            file_type,
            || answered(storage_device_number(handle)),
            || answered(volume_disk_extents(handle)),
        )
    }
}

/// A control's result as an answer: a refusal is `None`, and the one error that is this crate's
/// dependency's own report and not the device's, out of memory, is passed on.
fn answered<T>(result: io::Result<T>) -> io::Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == io::ErrorKind::OutOfMemory => Err(error),
        Err(_) => Ok(None),
    }
}

/// The kind `file_type` and the two controls' answers give. The controls are asked only when the
/// answer before leaves the question open: the device number only for a `DISK`, the extents only
/// when the device number answered.
fn classify(
    file_type: FileType,
    device_number: impl FnOnce() -> io::Result<Option<StorageDeviceNumber>>,
    extents: impl FnOnce() -> io::Result<Option<Vec<DiskExtent>>>,
) -> io::Result<HandleKind> {
    if file_type == FileType::CHAR {
        return Ok(HandleKind::CharacterDevice);
    }
    if file_type == FileType::PIPE {
        return Ok(HandleKind::Pipe);
    }
    if file_type != FileType::DISK {
        return Ok(HandleKind::Other(file_type));
    }
    let Some(number) = device_number()? else {
        return Ok(HandleKind::DiskFile);
    };
    Ok(match extents()? {
        Some(extents) => HandleKind::Volume { number, extents },
        None => HandleKind::PhysicalDisk(number),
    })
}
