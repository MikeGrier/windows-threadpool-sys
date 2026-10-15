// Copyright (c) 2026 Mike Grier
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::{ERROR_INVALID_FUNCTION, MAX_PATH};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_BACKUP_SEMANTICS, GetVolumeInformationW, GetVolumePathNameW,
};
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Ioctl::FSCTL_SET_SPARSE;
use windows_sys::Win32::System::SystemServices as win;
use wtf_string::Wtf16String;

use super::terminated;
use crate::{FileSystemFlags, UnassociatedEndpoint, VolumeInfo};

/// Every named flag beside `windows-sys`' own constant of the same name.
const NAMED: [(FileSystemFlags, u32); 30] = [
    (
        FileSystemFlags::CASE_SENSITIVE_SEARCH,
        win::FILE_CASE_SENSITIVE_SEARCH,
    ),
    (
        FileSystemFlags::CASE_PRESERVED_NAMES,
        win::FILE_CASE_PRESERVED_NAMES,
    ),
    (FileSystemFlags::UNICODE_ON_DISK, win::FILE_UNICODE_ON_DISK),
    (FileSystemFlags::PERSISTENT_ACLS, win::FILE_PERSISTENT_ACLS),
    (
        FileSystemFlags::FILE_COMPRESSION,
        win::FILE_FILE_COMPRESSION,
    ),
    (FileSystemFlags::VOLUME_QUOTAS, win::FILE_VOLUME_QUOTAS),
    (
        FileSystemFlags::SUPPORTS_SPARSE_FILES,
        win::FILE_SUPPORTS_SPARSE_FILES,
    ),
    (
        FileSystemFlags::SUPPORTS_REPARSE_POINTS,
        win::FILE_SUPPORTS_REPARSE_POINTS,
    ),
    (
        FileSystemFlags::SUPPORTS_REMOTE_STORAGE,
        win::FILE_SUPPORTS_REMOTE_STORAGE,
    ),
    (
        FileSystemFlags::RETURNS_CLEANUP_RESULT_INFO,
        win::FILE_RETURNS_CLEANUP_RESULT_INFO,
    ),
    (
        FileSystemFlags::SUPPORTS_POSIX_UNLINK_RENAME,
        win::FILE_SUPPORTS_POSIX_UNLINK_RENAME,
    ),
    (
        FileSystemFlags::SUPPORTS_BYPASS_IO,
        win::FILE_SUPPORTS_BYPASS_IO,
    ),
    (
        FileSystemFlags::SUPPORTS_STREAM_SNAPSHOTS,
        win::FILE_SUPPORTS_STREAM_SNAPSHOTS,
    ),
    (
        FileSystemFlags::SUPPORTS_CASE_SENSITIVE_DIRS,
        win::FILE_SUPPORTS_CASE_SENSITIVE_DIRS,
    ),
    (
        FileSystemFlags::VOLUME_IS_COMPRESSED,
        win::FILE_VOLUME_IS_COMPRESSED,
    ),
    (
        FileSystemFlags::SUPPORTS_OBJECT_IDS,
        win::FILE_SUPPORTS_OBJECT_IDS,
    ),
    (
        FileSystemFlags::SUPPORTS_ENCRYPTION,
        win::FILE_SUPPORTS_ENCRYPTION,
    ),
    (FileSystemFlags::NAMED_STREAMS, win::FILE_NAMED_STREAMS),
    (
        FileSystemFlags::READ_ONLY_VOLUME,
        win::FILE_READ_ONLY_VOLUME,
    ),
    (
        FileSystemFlags::SEQUENTIAL_WRITE_ONCE,
        win::FILE_SEQUENTIAL_WRITE_ONCE,
    ),
    (
        FileSystemFlags::SUPPORTS_TRANSACTIONS,
        win::FILE_SUPPORTS_TRANSACTIONS,
    ),
    (
        FileSystemFlags::SUPPORTS_HARD_LINKS,
        win::FILE_SUPPORTS_HARD_LINKS,
    ),
    (
        FileSystemFlags::SUPPORTS_EXTENDED_ATTRIBUTES,
        win::FILE_SUPPORTS_EXTENDED_ATTRIBUTES,
    ),
    (
        FileSystemFlags::SUPPORTS_OPEN_BY_FILE_ID,
        win::FILE_SUPPORTS_OPEN_BY_FILE_ID,
    ),
    (
        FileSystemFlags::SUPPORTS_USN_JOURNAL,
        win::FILE_SUPPORTS_USN_JOURNAL,
    ),
    (
        FileSystemFlags::SUPPORTS_INTEGRITY_STREAMS,
        win::FILE_SUPPORTS_INTEGRITY_STREAMS,
    ),
    (
        FileSystemFlags::SUPPORTS_BLOCK_REFCOUNTING,
        win::FILE_SUPPORTS_BLOCK_REFCOUNTING,
    ),
    (
        FileSystemFlags::SUPPORTS_SPARSE_VDL,
        win::FILE_SUPPORTS_SPARSE_VDL,
    ),
    (FileSystemFlags::DAX_VOLUME, win::FILE_DAX_VOLUME),
    (
        FileSystemFlags::SUPPORTS_GHOSTING,
        win::FILE_SUPPORTS_GHOSTING,
    ),
];

fn units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

// --- terminated ---

#[test]
fn a_name_ends_at_its_first_nul() {
    let mut buffer = units("NTFS");
    buffer.extend([0, u16::from(b'x'), 0]);
    assert_eq!(terminated(&buffer).as_units(), units("NTFS"));
}

#[test]
fn a_name_without_a_nul_is_the_whole_buffer() {
    assert_eq!(terminated(&units("ReFS")).as_units(), units("ReFS"));
}

#[test]
fn a_leading_nul_or_an_empty_buffer_is_an_empty_name() {
    assert!(terminated(&[0, u16::from(b'a')]).is_empty());
    assert!(terminated(&[]).is_empty());
}

#[test]
fn a_name_keeps_its_units_exactly() {
    let name = [u16::from(b'v'), 0xD800, u16::from(b'1'), 0xDFFF];
    let mut buffer = name.to_vec();
    buffer.push(0);
    assert_eq!(terminated(&buffer).as_units(), name);
}

// --- FileSystemFlags ---

#[test]
fn every_named_flag_has_the_value_windows_sys_gives_it() {
    for (ours, theirs) in NAMED {
        assert_eq!(ours.bits, theirs, "{ours:?}");
    }
}

#[test]
fn the_named_flags_are_distinct_single_bits() {
    let mut seen = 0_u32;
    for (flag, _) in NAMED {
        assert_eq!(flag.bits.count_ones(), 1, "{flag:?}");
        assert_eq!(seen & flag.bits, 0, "{flag:?} named twice");
        seen |= flag.bits;
    }
}

#[test]
fn contains_needs_every_bit() {
    let sparse = FileSystemFlags::SUPPORTS_SPARSE_FILES;
    let links = FileSystemFlags::SUPPORTS_HARD_LINKS;
    let both = sparse | links;
    assert!(both.contains(sparse));
    assert!(both.contains(links));
    assert!(both.contains(both));
    assert!(!sparse.contains(both));
    assert!(!sparse.contains(links));
    assert!(sparse.contains(FileSystemFlags { bits: 0 }));
    assert!(!FileSystemFlags { bits: 0 }.contains(sparse));
}

#[test]
fn or_combines_and_unnamed_bits_are_kept() {
    let unnamed = FileSystemFlags { bits: 0x8000_4000 };
    let combined = unnamed | FileSystemFlags::NAMED_STREAMS;
    assert_eq!(combined.bits, 0x8000_4000 | win::FILE_NAMED_STREAMS);
    assert!(combined.contains(unnamed));
    assert!(combined.contains(FileSystemFlags::NAMED_STREAMS));
}

// --- volume_info, against real handles ---

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "windows-overlapped-io-sys-volume-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir(&dir).expect("create temp dir");
    dir
}

fn open(path: &Path) -> UnassociatedEndpoint {
    UnassociatedEndpoint::open(path, true, false, 0).expect("open endpoint")
}

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain([0]).collect()
}

/// The same query by the volume's root path, through `GetVolumeInformationW`.
fn by_root_path(path: &Path) -> VolumeInfo {
    const NAME_UNITS: usize = MAX_PATH as usize + 1;
    let size = u32::try_from(NAME_UNITS).expect("a small buffer");
    let mut root = [0_u16; NAME_UNITS];
    // SAFETY: a terminated input path and an output buffer of exactly the size passed.
    let ok = unsafe { GetVolumePathNameW(wide(path).as_ptr(), root.as_mut_ptr(), size) };
    assert_ne!(
        ok,
        0,
        "GetVolumePathNameW: {}",
        std::io::Error::last_os_error()
    );
    let mut label = [0_u16; NAME_UNITS];
    let mut file_system_name = [0_u16; NAME_UNITS];
    let (mut serial_number, mut maximum_component_length, mut flags) = (0, 0, 0);
    // SAFETY: a terminated root path, two buffers of exactly the size passed and
    // three out-integers, all alive for the synchronous call.
    let ok = unsafe {
        GetVolumeInformationW(
            root.as_ptr(),
            label.as_mut_ptr(),
            size,
            &raw mut serial_number,
            &raw mut maximum_component_length,
            &raw mut flags,
            file_system_name.as_mut_ptr(),
            size,
        )
    };
    assert_ne!(
        ok,
        0,
        "GetVolumeInformationW: {}",
        std::io::Error::last_os_error()
    );
    let name = |buffer: &[u16]| {
        Wtf16String::from_units(&buffer[..buffer.iter().position(|&u| u == 0).unwrap_or(0)])
    };
    VolumeInfo {
        label: name(&label),
        serial_number,
        maximum_component_length,
        file_system_flags: FileSystemFlags { bits: flags },
        file_system_name: name(&file_system_name),
    }
}

#[test]
fn a_file_reports_what_its_volume_root_reports() {
    let dir = temp_dir("root");
    let path = dir.join("file.txt");
    std::fs::write(&path, b"x").expect("write");
    let endpoint = open(&path);
    let info = endpoint.observer().volume_info().expect("volume_info");
    assert_eq!(info, by_root_path(&path));
    assert!(!info.file_system_name.is_empty(), "{info:?}");
    drop(endpoint);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_directory_and_the_files_in_it_report_one_volume() {
    let dir = temp_dir("same");
    let first = dir.join("first.txt");
    let second = dir.join("second.txt");
    std::fs::write(&first, b"x").expect("write");
    std::fs::write(&second, b"y").expect("write");
    let directory = UnassociatedEndpoint::open(&dir, true, false, FILE_FLAG_BACKUP_SEMANTICS)
        .expect("open directory");
    let (first_endpoint, second_endpoint) = (open(&first), open(&second));
    let of_directory = directory.observer().volume_info().expect("directory");
    assert_eq!(
        first_endpoint.observer().volume_info().expect("first"),
        of_directory
    );
    assert_eq!(
        second_endpoint.observer().volume_info().expect("second"),
        of_directory
    );
    drop((directory, first_endpoint, second_endpoint));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn every_observer_of_a_handle_reads_the_same_volume() {
    let dir = temp_dir("shared");
    let path = dir.join("shared.txt");
    std::fs::write(&path, b"x").expect("write");
    let endpoint = open(&path);
    let first = endpoint.observer();
    let second = first.clone();
    assert_eq!(
        first.volume_info().expect("first"),
        second.volume_info().expect("second")
    );
    drop((first, second, endpoint));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Measured on NTFS and ReFS: the 32-bit serial is the low half of the 64-bit one
/// `file_id` reports.
#[test]
fn the_serial_number_is_the_low_half_of_the_file_ids() {
    let dir = temp_dir("serial");
    let path = dir.join("serial.txt");
    std::fs::write(&path, b"x").expect("write");
    let endpoint = open(&path);
    let observer = endpoint.observer();
    let serial = observer.volume_info().expect("volume_info").serial_number;
    let wide_serial = observer.file_id().expect("file_id").volume_serial_number;
    assert_eq!(u64::from(serial), wide_serial & u64::from(u32::MAX));
    drop((observer, endpoint));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Whether marking a file sparse succeeds follows the sparse-files flag, in
/// whichever direction this volume reports.
#[test]
fn marking_a_file_sparse_succeeds_exactly_when_the_volume_says_it_can() {
    let dir = temp_dir("sparse");
    let path = dir.join("sparse.txt");
    std::fs::write(&path, b"x").expect("write");
    let endpoint = open(&path);
    let supported = endpoint
        .observer()
        .volume_info()
        .expect("volume_info")
        .file_system_flags
        .contains(FileSystemFlags::SUPPORTS_SPARSE_FILES);

    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .expect("open for writing");
    let mut returned = 0_u32;
    // SAFETY: a live synchronous handle and no buffers, so nothing outlives the call.
    let ok = unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            FSCTL_SET_SPARSE,
            std::ptr::null(),
            0,
            std::ptr::null_mut(),
            0,
            &raw mut returned,
            std::ptr::null_mut(),
        )
    };
    assert_eq!(
        ok != 0,
        supported,
        "FSCTL_SET_SPARSE: {}",
        std::io::Error::last_os_error()
    );

    drop((file, endpoint));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Measured: the NUL device refuses the volume query.
#[test]
fn the_nul_device_has_no_volume() {
    let nul = UnassociatedEndpoint::open(r"\\.\NUL", true, false, 0).expect("open NUL");
    let error = nul
        .observer()
        .volume_info()
        .expect_err("the NUL device has no volume");
    assert_eq!(
        error.raw_os_error(),
        Some(ERROR_INVALID_FUNCTION as i32),
        "{error:?}"
    );
}
