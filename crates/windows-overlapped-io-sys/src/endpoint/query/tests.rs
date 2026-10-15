// Copyright (c) 2026 Mike Grier
use std::io::Write;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};

use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_DISPOSITION_INFO, FILE_FLAG_BACKUP_SEMANTICS, FileDispositionInfo,
    SetFileInformationByHandle,
};

use crate::{StandardInfo, UnassociatedEndpoint};

/// `FSCTL_SET_SPARSE`, which `windows-sys` exports only behind `Win32_System_Ioctl`.
const FSCTL_SET_SPARSE: u32 = 0x0009_00C4;

fn temp_path(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "windows-overlapped-io-sys-standard-{tag}-{}.tmp",
        std::process::id()
    ))
}

fn open(path: &Path) -> UnassociatedEndpoint {
    UnassociatedEndpoint::open(path, true, false, 0).expect("open endpoint")
}

/// Mark `file` sparse, so extending it allocates nothing.
fn make_sparse(file: &std::fs::File) {
    let mut returned = 0_u32;
    // SAFETY: a live synchronous handle and no buffers, so nothing outlives the call.
    let ok = unsafe {
        windows_sys::Win32::System::IO::DeviceIoControl(
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
    assert_ne!(
        ok,
        0,
        "FSCTL_SET_SPARSE: {}",
        std::io::Error::last_os_error()
    );
}

/// Request a delete through `file`'s handle, which takes effect when the last handle closes.
fn request_delete(file: &std::fs::File) {
    let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
    let size = u32::try_from(size_of::<FILE_DISPOSITION_INFO>()).expect("small struct");
    // SAFETY: a live handle opened with DELETE access, and an input buffer of exactly the size passed.
    let ok = unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileDispositionInfo,
            (&raw const disposition).cast(),
            size,
        )
    };
    assert_ne!(
        ok,
        0,
        "FileDispositionInfo: {}",
        std::io::Error::last_os_error()
    );
}

#[test]
fn an_ordinary_file_reports_its_length_one_link_and_no_flags() {
    let path = temp_path("ordinary");
    std::fs::write(&path, vec![b'a'; 65_536]).expect("write temp file");
    let endpoint = open(&path);
    let info = endpoint.observer().standard_info().expect("standard_info");

    assert_eq!(info.len, 65_536);
    assert!(info.allocation_size >= info.len, "{info:?}");
    assert_eq!(info.link_count, 1);
    assert!(!info.delete_pending);
    assert!(!info.is_directory);

    drop(endpoint);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn file_len_is_standard_info_len_across_sizes() {
    let path = temp_path("shortcut");
    let writer = std::fs::File::create(&path).expect("create temp file");
    let endpoint = open(&path);
    let observer = endpoint.observer();

    for size in [0_u64, 1, 511, 512, 4096, 4097, 1 << 20, 3] {
        writer.set_len(size).expect("set length");
        let info = observer.standard_info().expect("standard_info");
        assert_eq!(info.len, size);
        assert_eq!(observer.file_len().expect("file_len"), info.len);
    }

    drop((observer, endpoint, writer));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_sparse_file_reports_an_allocation_smaller_than_its_length() {
    let path = temp_path("sparse");
    let writer = std::fs::File::create(&path).expect("create temp file");
    make_sparse(&writer);
    writer.set_len(1 << 30).expect("extend to 1 GiB");
    let endpoint = open(&path);
    let info = endpoint.observer().standard_info().expect("standard_info");

    assert_eq!(info.len, 1 << 30);
    assert!(info.allocation_size < info.len, "{info:?}");

    drop((endpoint, writer));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn the_link_count_follows_hard_links_made_and_removed() {
    let path = temp_path("links");
    let second = temp_path("links-second");
    let third = temp_path("links-third");
    std::fs::write(&path, b"linked").expect("write temp file");
    let endpoint = open(&path);
    let observer = endpoint.observer();
    let links = || observer.standard_info().expect("standard_info").link_count;

    assert_eq!(links(), 1);
    std::fs::hard_link(&path, &second).expect("second link");
    assert_eq!(links(), 2);
    std::fs::hard_link(&path, &third).expect("third link");
    assert_eq!(links(), 3);
    std::fs::remove_file(&second).expect("remove second link");
    assert_eq!(links(), 2);
    std::fs::remove_file(&third).expect("remove third link");
    assert_eq!(links(), 1);

    drop((observer, endpoint));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_delete_requested_while_the_observer_holds_the_file_is_pending_until_it_lets_go() {
    let path = temp_path("delete");
    std::fs::write(&path, b"doomed").expect("write temp file");
    let endpoint = open(&path);
    let observer = endpoint.observer();
    drop(endpoint);
    assert!(
        !observer
            .standard_info()
            .expect("standard_info")
            .delete_pending
    );

    let deleter = std::fs::OpenOptions::new()
        .access_mode(DELETE)
        .open(&path)
        .expect("open for delete");
    request_delete(&deleter);
    drop(deleter);

    let info = observer
        .standard_info()
        .expect("standard_info after the request");
    assert!(info.delete_pending, "{info:?}");
    assert_eq!(
        info.len, 6,
        "a pending delete leaves the contents readable through the handle"
    );

    drop(observer);
    assert!(
        !path.exists(),
        "the last holder's close completes the delete"
    );
}

#[test]
fn a_directory_reports_itself_as_one() {
    let dir = temp_path("directory");
    std::fs::create_dir(&dir).expect("create directory");
    let endpoint = UnassociatedEndpoint::open(&dir, true, false, FILE_FLAG_BACKUP_SEMANTICS)
        .expect("open directory endpoint");
    let info = endpoint.observer().standard_info().expect("standard_info");

    assert!(info.is_directory, "{info:?}");
    assert!(!info.delete_pending);

    drop(endpoint);
    let _ = std::fs::remove_dir(&dir);
}

#[test]
fn every_observer_of_a_handle_reads_the_same_standard_info() {
    let path = temp_path("agree");
    let mut writer = std::fs::File::create(&path).expect("create temp file");
    writer.write_all(b"agreed").expect("write");
    let endpoint = open(&path);
    let first = endpoint.observer();
    let clone = first.clone();
    let held = endpoint.into_endpoint_handle();
    let late = held.observer();

    let expected: StandardInfo = first.standard_info().expect("standard_info");
    for observer in [&clone, &late] {
        assert_eq!(observer.standard_info().expect("standard_info"), expected);
    }

    drop((first, clone, late, held, writer));
    let _ = std::fs::remove_file(&path);
}
