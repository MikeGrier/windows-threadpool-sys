// Copyright (c) 2026 Mike Grier
//! The observer's information queries, all behind the `fs` feature.

use std::io;
use std::mem::{MaybeUninit, offset_of};
use std::os::windows::io::AsRawHandle;

use windows_sys::Win32::Storage::FileSystem::{
    FILE_ALIGNMENT_INFO, FILE_ID_128, FILE_ID_INFO, FILE_INFO_BY_HANDLE_CLASS, FILE_STANDARD_INFO,
    FILE_STORAGE_INFO, FileAlignmentInfo, FileIdInfo, FileStandardInfo, FileStorageInfo,
    GetFileInformationByHandleEx,
};

use super::EndpointObserver;

#[cfg(test)]
mod tests;

/// `FILE_STORAGE_INFO`'s flag bits and its unknown-offset sentinel.
///
/// `windows-sys` exports these only behind `Win32_System_WindowsProgramming`, so
/// they are named here rather than pulling that feature in. Changing any value is
/// a breaking change.
pub(crate) mod storage_info_values {
    /// `STORAGE_INFO_FLAGS_ALIGNED_DEVICE`.
    pub(crate) const FLAGS_ALIGNED_DEVICE: u32 = 0x1;
    /// `STORAGE_INFO_FLAGS_PARTITION_ALIGNED_ON_DEVICE`.
    pub(crate) const FLAGS_PARTITION_ALIGNED_ON_DEVICE: u32 = 0x2;
    /// `STORAGE_INFO_OFFSET_UNKNOWN`.
    pub(crate) const OFFSET_UNKNOWN: u32 = 0xFFFF_FFFF;
}

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

/// The sector geometry of the storage under a file, as
/// `GetFileInformationByHandleEx(FileStorageInfo)` reports it:
/// [`EndpointObserver::storage_info`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StorageInfo {
    /// The logical sector size: the unit unbuffered offsets and lengths are
    /// multiples of.
    pub logical_bytes_per_sector: u32,
    /// The physical sector size the device writes atomically.
    pub physical_bytes_per_sector_for_atomicity: u32,
    /// The physical sector size the device performs best at.
    pub physical_bytes_per_sector_for_performance: u32,
    /// The atomic write unit the file system honours, which may be smaller than
    /// the device's.
    pub file_system_effective_physical_bytes_per_sector_for_atomicity: u32,
    /// The `STORAGE_INFO_FLAGS_*` bits, as reported; see
    /// [`aligned_device`](Self::aligned_device) and
    /// [`partition_aligned_on_device`](Self::partition_aligned_on_device).
    pub flags: u32,
    /// Bytes from the start of the first sector to the first physical sector
    /// boundary, or `None` when the file system reports it unknown.
    pub byte_offset_for_sector_alignment: Option<u32>,
    /// Bytes from the partition's start to the first physical sector boundary,
    /// or `None` when the file system reports it unknown.
    pub byte_offset_for_partition_alignment: Option<u32>,
}

impl StorageInfo {
    /// Whether `STORAGE_INFO_FLAGS_ALIGNED_DEVICE` is set: the device's logical
    /// sectors start on physical sector boundaries.
    #[must_use]
    pub fn aligned_device(&self) -> bool {
        self.flags & storage_info_values::FLAGS_ALIGNED_DEVICE != 0
    }

    /// Whether `STORAGE_INFO_FLAGS_PARTITION_ALIGNED_ON_DEVICE` is set: the
    /// partition starts on a physical sector boundary.
    #[must_use]
    pub fn partition_aligned_on_device(&self) -> bool {
        self.flags & storage_info_values::FLAGS_PARTITION_ALIGNED_ON_DEVICE != 0
    }
}

fn storage_info_from(raw: &FILE_STORAGE_INFO) -> StorageInfo {
    let known = |offset| (offset != storage_info_values::OFFSET_UNKNOWN).then_some(offset);
    StorageInfo {
        logical_bytes_per_sector: raw.LogicalBytesPerSector,
        physical_bytes_per_sector_for_atomicity: raw.PhysicalBytesPerSectorForAtomicity,
        physical_bytes_per_sector_for_performance: raw.PhysicalBytesPerSectorForPerformance,
        file_system_effective_physical_bytes_per_sector_for_atomicity: raw
            .FileSystemEffectivePhysicalBytesPerSectorForAtomicity,
        flags: raw.Flags,
        byte_offset_for_sector_alignment: known(raw.ByteOffsetForSectorAlignment),
        byte_offset_for_partition_alignment: known(raw.ByteOffsetForPartitionAlignment),
    }
}

/// The buffer alignment a device requires for unbuffered I/O, as
/// `GetFileInformationByHandleEx(FileAlignmentInfo)` reports it:
/// [`EndpointObserver::alignment_info`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AlignmentInfo {
    /// The reported requirement, a mask: an address meets it when every bit set
    /// here is clear in the address. Win32 documents it as one less than a power
    /// of two (`FILE_BYTE_ALIGNMENT` is 0, `FILE_512_BYTE_ALIGNMENT` is `0x1FF`).
    pub requirement_mask: u32,
}

impl AlignmentInfo {
    /// Whether `address` meets the requirement.
    #[must_use]
    pub fn is_aligned(&self, address: usize) -> bool {
        address as u64 & u64::from(self.requirement_mask) == 0
    }

    /// The requirement in bytes, or `None` when the mask is not one less than a
    /// power of two and so names no single alignment.
    #[must_use]
    pub fn byte_alignment(&self) -> Option<u64> {
        let bytes = u64::from(self.requirement_mask) + 1;
        bytes.is_power_of_two().then_some(bytes)
    }
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

    /// The sector geometry of the storage under the file, read through the
    /// handle with `GetFileInformationByHandleEx(FileStorageInfo)`.
    ///
    /// Synchronous and issues no overlapped I/O, so it is safe while the endpoint
    /// has operations in flight.
    ///
    /// # Errors
    ///
    /// Returns the error `GetFileInformationByHandleEx` reports, for a handle
    /// whose file system or device does not answer the storage query.
    pub fn storage_info(&self) -> io::Result<StorageInfo> {
        // SAFETY: FILE_STORAGE_INFO is windows-sys' own definition of the class's
        // structure, and every field is a u32.
        let raw: FILE_STORAGE_INFO = unsafe { self.query_info(FileStorageInfo) }?;
        Ok(storage_info_from(&raw))
    }

    /// The buffer alignment the device requires for unbuffered I/O, read through
    /// the handle with `GetFileInformationByHandleEx(FileAlignmentInfo)`.
    ///
    /// Synchronous and issues no overlapped I/O, so it is safe while the endpoint
    /// has operations in flight.
    ///
    /// # Errors
    ///
    /// Returns the error `GetFileInformationByHandleEx` reports, for a handle
    /// whose file system or device does not answer the alignment query.
    pub fn alignment_info(&self) -> io::Result<AlignmentInfo> {
        // SAFETY: FILE_ALIGNMENT_INFO is windows-sys' own definition of the class's
        // structure: one u32.
        let raw: FILE_ALIGNMENT_INFO = unsafe { self.query_info(FileAlignmentInfo) }?;
        Ok(AlignmentInfo {
            requirement_mask: raw.AlignmentRequirement,
        })
    }
}
