// Copyright (c) 2026 Mike Grier
//! The observer's information queries, all behind the `fs` feature.

use std::io;
use std::mem::{MaybeUninit, offset_of};
use std::os::windows::io::AsRawHandle;

use windows_sys::Win32::Storage::FileSystem::{
    FILE_STANDARD_INFO, FileStandardInfo, GetFileInformationByHandleEx,
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
        let mut raw = MaybeUninit::<RawStandardInfo>::uninit();
        let size = u32::try_from(size_of::<RawStandardInfo>())
            .expect("FILE_STANDARD_INFO fits a u32 size");
        // SAFETY: a live handle this observer holds, and an output buffer laid out
        // as FILE_STANDARD_INFO (asserted above) of exactly the size passed. The
        // call is synchronous, so the buffer is borrowed only for this statement.
        let ok = unsafe {
            GetFileInformationByHandleEx(
                self.handle.as_raw_handle(),
                FileStandardInfo,
                raw.as_mut_ptr().cast(),
                size,
            )
        };
        // No test reaches this: files, the NUL device and an unconnected named-pipe
        // server all answer. A driver rejecting IRP_MJ_QUERY_INFORMATION would.
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the call succeeded, so it filled the whole structure, and every
        // field is an integer, valid for any bit pattern.
        let raw = unsafe { raw.assume_init() };
        // A file system never reports a negative size; reinterpret rather than check.
        Ok(StandardInfo {
            len: raw.end_of_file.cast_unsigned(),
            allocation_size: raw.allocation_size.cast_unsigned(),
            link_count: raw.number_of_links,
            delete_pending: raw.delete_pending != 0,
            is_directory: raw.directory != 0,
        })
    }
}
