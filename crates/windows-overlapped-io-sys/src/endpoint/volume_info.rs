// Copyright (c) 2026 Mike Grier
//! The observer's volume query, behind the `fs` feature.

use std::io;
use std::ops::BitOr;
use std::os::windows::io::AsRawHandle;

use windows_sys::Win32::Foundation::MAX_PATH;
use windows_sys::Win32::Storage::FileSystem::GetVolumeInformationByHandleW;
use wtf_string::Wtf16String;

use super::EndpointObserver;

#[cfg(test)]
mod tests;

/// What `GetVolumeInformationByHandleW` reports about the volume a file is on:
/// [`EndpointObserver::volume_info`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct VolumeInfo {
    /// The volume's label; empty when it has none.
    pub label: Wtf16String,
    /// The volume's serial number. Measured on NTFS and ReFS: the low 32 bits of
    /// the 64-bit serial [`EndpointObserver::file_id`] reports.
    pub serial_number: u32,
    /// The longest file-name component the file system accepts, in UTF-16 units.
    pub maximum_component_length: u32,
    /// What the file system supports, as reported.
    pub file_system_flags: FileSystemFlags,
    /// The file system's name, as reported.
    pub file_system_name: Wtf16String,
}

/// The `FILE_*` bits `GetVolumeInformationByHandleW` reports, kept as reported
/// -- bits named by no constant here included.
///
/// `windows-sys` exports the constants only behind `Win32_System_SystemServices`,
/// so they are named here rather than pulling that feature in. Changing any value
/// is a breaking change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FileSystemFlags {
    /// The bits, as reported.
    pub bits: u32,
}

impl FileSystemFlags {
    /// `FILE_CASE_SENSITIVE_SEARCH`: file names can be searched case-sensitively.
    pub const CASE_SENSITIVE_SEARCH: Self = Self { bits: 0x0000_0001 };
    /// `FILE_CASE_PRESERVED_NAMES`: the case of file names is preserved.
    pub const CASE_PRESERVED_NAMES: Self = Self { bits: 0x0000_0002 };
    /// `FILE_UNICODE_ON_DISK`: file names are stored in Unicode.
    pub const UNICODE_ON_DISK: Self = Self { bits: 0x0000_0004 };
    /// `FILE_PERSISTENT_ACLS`: access control lists are preserved and enforced.
    pub const PERSISTENT_ACLS: Self = Self { bits: 0x0000_0008 };
    /// `FILE_FILE_COMPRESSION`: per-file compression.
    pub const FILE_COMPRESSION: Self = Self { bits: 0x0000_0010 };
    /// `FILE_VOLUME_QUOTAS`: disk quotas.
    pub const VOLUME_QUOTAS: Self = Self { bits: 0x0000_0020 };
    /// `FILE_SUPPORTS_SPARSE_FILES`: sparse files.
    pub const SUPPORTS_SPARSE_FILES: Self = Self { bits: 0x0000_0040 };
    /// `FILE_SUPPORTS_REPARSE_POINTS`: reparse points.
    pub const SUPPORTS_REPARSE_POINTS: Self = Self { bits: 0x0000_0080 };
    /// `FILE_SUPPORTS_REMOTE_STORAGE`: remote storage.
    pub const SUPPORTS_REMOTE_STORAGE: Self = Self { bits: 0x0000_0100 };
    /// `FILE_RETURNS_CLEANUP_RESULT_INFO`: cleanup returns result information.
    pub const RETURNS_CLEANUP_RESULT_INFO: Self = Self { bits: 0x0000_0200 };
    /// `FILE_SUPPORTS_POSIX_UNLINK_RENAME`: POSIX-style delete and rename.
    pub const SUPPORTS_POSIX_UNLINK_RENAME: Self = Self { bits: 0x0000_0400 };
    /// `FILE_SUPPORTS_BYPASS_IO`: BypassIO.
    pub const SUPPORTS_BYPASS_IO: Self = Self { bits: 0x0000_0800 };
    /// `FILE_SUPPORTS_STREAM_SNAPSHOTS`: stream snapshots.
    pub const SUPPORTS_STREAM_SNAPSHOTS: Self = Self { bits: 0x0000_1000 };
    /// `FILE_SUPPORTS_CASE_SENSITIVE_DIRS`: case-sensitive directories.
    pub const SUPPORTS_CASE_SENSITIVE_DIRS: Self = Self { bits: 0x0000_2000 };
    /// `FILE_VOLUME_IS_COMPRESSED`: the volume is compressed.
    pub const VOLUME_IS_COMPRESSED: Self = Self { bits: 0x0000_8000 };
    /// `FILE_SUPPORTS_OBJECT_IDS`: object identifiers.
    pub const SUPPORTS_OBJECT_IDS: Self = Self { bits: 0x0001_0000 };
    /// `FILE_SUPPORTS_ENCRYPTION`: the Encrypted File System.
    pub const SUPPORTS_ENCRYPTION: Self = Self { bits: 0x0002_0000 };
    /// `FILE_NAMED_STREAMS`: named streams.
    pub const NAMED_STREAMS: Self = Self { bits: 0x0004_0000 };
    /// `FILE_READ_ONLY_VOLUME`: the volume is read-only.
    pub const READ_ONLY_VOLUME: Self = Self { bits: 0x0008_0000 };
    /// `FILE_SEQUENTIAL_WRITE_ONCE`: a single sequential write.
    pub const SEQUENTIAL_WRITE_ONCE: Self = Self { bits: 0x0010_0000 };
    /// `FILE_SUPPORTS_TRANSACTIONS`: transactions.
    pub const SUPPORTS_TRANSACTIONS: Self = Self { bits: 0x0020_0000 };
    /// `FILE_SUPPORTS_HARD_LINKS`: hard links.
    pub const SUPPORTS_HARD_LINKS: Self = Self { bits: 0x0040_0000 };
    /// `FILE_SUPPORTS_EXTENDED_ATTRIBUTES`: extended attributes.
    pub const SUPPORTS_EXTENDED_ATTRIBUTES: Self = Self { bits: 0x0080_0000 };
    /// `FILE_SUPPORTS_OPEN_BY_FILE_ID`: opening by file ID.
    pub const SUPPORTS_OPEN_BY_FILE_ID: Self = Self { bits: 0x0100_0000 };
    /// `FILE_SUPPORTS_USN_JOURNAL`: update sequence number journals.
    pub const SUPPORTS_USN_JOURNAL: Self = Self { bits: 0x0200_0000 };
    /// `FILE_SUPPORTS_INTEGRITY_STREAMS`: integrity streams.
    pub const SUPPORTS_INTEGRITY_STREAMS: Self = Self { bits: 0x0400_0000 };
    /// `FILE_SUPPORTS_BLOCK_REFCOUNTING`: sharing logical clusters between files.
    pub const SUPPORTS_BLOCK_REFCOUNTING: Self = Self { bits: 0x0800_0000 };
    /// `FILE_SUPPORTS_SPARSE_VDL`: tracking whether each cluster holds valid data.
    pub const SUPPORTS_SPARSE_VDL: Self = Self { bits: 0x1000_0000 };
    /// `FILE_DAX_VOLUME`: a direct-access volume.
    pub const DAX_VOLUME: Self = Self { bits: 0x2000_0000 };
    /// `FILE_SUPPORTS_GHOSTING`: ghosting.
    pub const SUPPORTS_GHOSTING: Self = Self { bits: 0x4000_0000 };

    /// Whether every bit of `other` is set; true for an empty `other`.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.bits & other.bits == other.bits
    }
}

impl BitOr for FileSystemFlags {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        Self {
            bits: self.bits | other.bits,
        }
    }
}

/// The units of `buffer` before its first NUL, or all of them when it has none.
fn terminated(buffer: &[u16]) -> Wtf16String {
    let length = buffer
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(buffer.len());
    Wtf16String::from_units(&buffer[..length])
}

impl EndpointObserver {
    /// The volume the file is on -- its label, serial number, longest name
    /// component, file-system flags and file-system name -- read through the
    /// handle with `GetVolumeInformationByHandleW`.
    ///
    /// Synchronous and issues no overlapped I/O, so it is safe while the endpoint
    /// has operations in flight.
    ///
    /// # Errors
    ///
    /// Returns the error `GetVolumeInformationByHandleW` reports, for a handle
    /// whose device has no volume. Measured: the NUL device is refused with
    /// `ERROR_INVALID_FUNCTION`.
    pub fn volume_info(&self) -> io::Result<VolumeInfo> {
        // The documented maximum for both names, terminator included.
        const NAME_UNITS: usize = MAX_PATH as usize + 1;
        let mut label = [0_u16; NAME_UNITS];
        let mut file_system_name = [0_u16; NAME_UNITS];
        let (mut serial_number, mut maximum_component_length, mut flags) = (0, 0, 0);
        let size = u32::try_from(NAME_UNITS).expect("MAX_PATH + 1 fits a u32");
        // SAFETY: a live handle this observer holds; two name buffers of exactly
        // the size passed and three out-integers, all alive for the call, which
        // is synchronous.
        let ok = unsafe {
            GetVolumeInformationByHandleW(
                self.handle.as_raw_handle(),
                label.as_mut_ptr(),
                size,
                &raw mut serial_number,
                &raw mut maximum_component_length,
                &raw mut flags,
                file_system_name.as_mut_ptr(),
                size,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(VolumeInfo {
            label: terminated(&label),
            serial_number,
            maximum_component_length,
            file_system_flags: FileSystemFlags { bits: flags },
            file_system_name: terminated(&file_system_name),
        })
    }
}
