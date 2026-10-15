// Copyright (c) 2026 Mike Grier
//! The observer's information queries, all behind the `fs` feature.

use std::io;
use std::mem::{MaybeUninit, offset_of};
use std::os::windows::io::AsRawHandle;

use windows_sys::Win32::Storage::FileSystem::{
    FILE_ID_128, FILE_ID_INFO, FILE_INFO_BY_HANDLE_CLASS, FILE_STANDARD_INFO, FileIdInfo,
    FileStandardInfo, GetFileInformationByHandleEx,
};

use super::EndpointObserver;

#[cfg(test)]
mod tests;

/// A file's standard information, read through an open handle.
///
/// What `GetFileInformationByHandleEx(FileStandardInfo)` reports, as of the
/// query: [`EndpointObserver::standard_info`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StandardInfo {
    /// The end-of-file position in bytes: the file's length.
    pub len: u64,
    /// The bytes the file system has allocated to the file. Rounded up to its
    /// allocation unit, and smaller than `len` for a sparse file.
    pub allocation_size: u64,
    /// The number of hard links to the file.
    pub link_count: u32,
    /// Whether a delete has been requested that takes effect once the last
    /// handle to the file closes.
    pub delete_pending: bool,
    /// Whether the handle is to a directory.
    pub is_directory: bool,
}

/// `FILE_STANDARD_INFO` with its two `BOOLEAN`s read as bytes, so a value other
/// than 0 or 1 from a driver cannot become an invalid `bool`.
#[repr(C)]
struct RawStandardInfo {
    allocation_size: i64,
    end_of_file: i64,
    number_of_links: u32,
    delete_pending: u8,
    directory: u8,
}

const _: () = {
    assert!(size_of::<RawStandardInfo>() == size_of::<FILE_STANDARD_INFO>());
    assert!(align_of::<RawStandardInfo>() == align_of::<FILE_STANDARD_INFO>());
    assert!(
        offset_of!(RawStandardInfo, allocation_size)
            == offset_of!(FILE_STANDARD_INFO, AllocationSize)
    );
    assert!(offset_of!(RawStandardInfo, end_of_file) == offset_of!(FILE_STANDARD_INFO, EndOfFile));
    assert!(
        offset_of!(RawStandardInfo, number_of_links)
            == offset_of!(FILE_STANDARD_INFO, NumberOfLinks)
    );
    assert!(
        offset_of!(RawStandardInfo, delete_pending)
            == offset_of!(FILE_STANDARD_INFO, DeletePending)
    );
    assert!(offset_of!(RawStandardInfo, directory) == offset_of!(FILE_STANDARD_INFO, Directory));
};

/// A file's identity: the volume it is on and its ID within that volume.
///
/// Two handles with equal `FileId`s are open on the same file, whatever paths
/// they were opened by -- hard links and renames included. What
/// `GetFileInformationByHandleEx(FileIdInfo)` reports:
/// [`EndpointObserver::file_id`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FileId {
    /// The serial number of the volume holding the file.
    pub volume_serial_number: u64,
    /// The file's 128-bit ID within its volume, as the file system reports it.
    pub file_id: [u8; 16],
}

impl EndpointObserver {
    /// Fill a `T` with the information class `class` through the handle.
    ///
    /// # Safety
    ///
    /// `T` must be laid out as the structure `class` returns, and every bit
    /// pattern of it must be valid.
    unsafe fn query_info<T>(&self, class: FILE_INFO_BY_HANDLE_CLASS) -> io::Result<T> {
        let mut out = MaybeUninit::<T>::uninit();
        let size = u32::try_from(size_of::<T>()).expect("an information structure fits a u32 size");
        // SAFETY: a live handle this observer holds, and an output buffer laid out
        // as `class` requires (the caller's obligation) of exactly the size passed.
        // The call is synchronous, so the buffer is borrowed only for this statement.
        let ok = unsafe {
            GetFileInformationByHandleEx(
                self.handle.as_raw_handle(),
                class,
                out.as_mut_ptr().cast(),
                size,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the call succeeded, so it filled the whole structure, and any
        // bit pattern is valid for `T` (the caller's obligation).
        Ok(unsafe { out.assume_init() })
    }
}

impl EndpointObserver {
    /// The file's standard information: its length, allocation, link count,
    /// whether a delete is pending, and whether it is a directory.
    ///
    /// Read through the handle with `GetFileInformationByHandleEx(FileStandardInfo)`.
    /// The query is synchronous and issues no overlapped I/O, so it is safe while
    /// the endpoint has operations in flight. Read through the handle, the
    /// length is current as soon as a write through any other handle has
    /// extended or truncated the file, unlike the size in the file's directory
    /// entry.
    ///
    /// # Errors
    ///
    /// Returns the error `GetFileInformationByHandleEx` reports, for a handle
    /// whose device does not answer the standard-information query.
    pub fn standard_info(&self) -> io::Result<StandardInfo> {
        // Never seen failing -- files, the NUL device and an unconnected named-pipe server
        // all answer -- so its error is tested through `file_id`, on the shared path.
        // SAFETY: laid out as FILE_STANDARD_INFO (asserted above), and all integers.
        let raw: RawStandardInfo = unsafe { self.query_info(FileStandardInfo) }?;
        // A file system never reports a negative size; reinterpret rather than check.
        Ok(StandardInfo {
            len: raw.end_of_file.cast_unsigned(),
            allocation_size: raw.allocation_size.cast_unsigned(),
            link_count: raw.number_of_links,
            delete_pending: raw.delete_pending != 0,
            is_directory: raw.directory != 0,
        })
    }

    /// The file's identity: its volume's serial number and its 128-bit ID within
    /// that volume, read through the handle with
    /// `GetFileInformationByHandleEx(FileIdInfo)`.
    ///
    /// Synchronous and issues no overlapped I/O, so it is safe while the endpoint
    /// has operations in flight.
    ///
    /// # Errors
    ///
    /// Returns the error `GetFileInformationByHandleEx` reports, for a handle
    /// whose file system or device does not answer the file-ID query.
    pub fn file_id(&self) -> io::Result<FileId> {
        // SAFETY: FILE_ID_INFO is windows-sys' own definition of the class's
        // structure, and both its fields are integers.
        let raw: FILE_ID_INFO = unsafe { self.query_info(FileIdInfo) }?;
        let FILE_ID_128 { Identifier } = raw.FileId;
        Ok(FileId {
            volume_serial_number: raw.VolumeSerialNumber,
            file_id: Identifier,
        })
    }
}
