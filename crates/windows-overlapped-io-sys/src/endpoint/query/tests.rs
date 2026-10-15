// Copyright (c) 2026 Mike Grier
use std::io::Write;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::ERROR_INVALID_PARAMETER;
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

// --- file_id ---

#[test]
fn every_observer_of_a_handle_reads_the_same_file_id() {
    let path = temp_path("id-agree");
    std::fs::write(&path, b"one file").expect("write temp file");
    let endpoint = open(&path);
    let first = endpoint.observer();
    let clone = first.clone();
    let held = endpoint.into_endpoint_handle();
    let late = held.observer();

    let expected = first.file_id().expect("file_id");
    for observer in [&clone, &late] {
        assert_eq!(observer.file_id().expect("file_id"), expected);
    }

    drop((first, clone, late, held));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_second_open_of_the_same_path_reads_the_same_file_id() {
    let path = temp_path("id-reopen");
    std::fs::write(&path, b"reopened").expect("write temp file");
    let first = open(&path);
    let second = open(&path);

    assert_eq!(
        first.observer().file_id().expect("file_id"),
        second.observer().file_id().expect("file_id")
    );

    drop((first, second));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn files_in_one_directory_differ_in_id_and_share_a_volume() {
    let paths: Vec<PathBuf> = (0..4)
        .map(|i| temp_path(&format!("id-distinct-{i}")))
        .collect();
    let mut ids = Vec::new();
    for path in &paths {
        std::fs::write(path, b"distinct").expect("write temp file");
        let endpoint = open(path);
        ids.push(endpoint.observer().file_id().expect("file_id"));
    }

    for (i, a) in ids.iter().enumerate() {
        assert_eq!(a.volume_serial_number, ids[0].volume_serial_number);
        for b in &ids[i + 1..] {
            assert_ne!(a.file_id, b.file_id, "{a:?} {b:?}");
        }
    }

    for path in &paths {
        let _ = std::fs::remove_file(path);
    }
}

#[test]
fn a_rename_while_the_observer_holds_the_file_keeps_its_id() {
    let path = temp_path("id-rename");
    let renamed = temp_path("id-renamed");
    std::fs::write(&path, b"moving").expect("write temp file");
    let endpoint = open(&path);
    let observer = endpoint.observer();
    let before = observer.file_id().expect("file_id");

    std::fs::rename(&path, &renamed).expect("rename while held");
    assert_eq!(observer.file_id().expect("file_id after rename"), before);
    let reopened = open(&renamed);
    assert_eq!(reopened.observer().file_id().expect("file_id"), before);

    drop((observer, endpoint, reopened));
    let _ = std::fs::remove_file(&renamed);
}

#[test]
fn a_hard_link_reads_the_same_file_id() {
    let path = temp_path("id-link");
    let link = temp_path("id-link-second");
    std::fs::write(&path, b"linked").expect("write temp file");
    std::fs::hard_link(&path, &link).expect("hard link");

    let through_path = open(&path);
    let through_link = open(&link);
    assert_eq!(
        through_path.observer().file_id().expect("file_id"),
        through_link.observer().file_id().expect("file_id")
    );

    drop((through_path, through_link));
    let _ = std::fs::remove_file(&link);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_file_recreated_under_the_same_name_has_a_new_id() {
    let path = temp_path("id-recreate");
    std::fs::write(&path, b"first").expect("write temp file");
    let first = open(&path);
    let first_id = first.observer().file_id().expect("file_id");
    drop(first);

    std::fs::remove_file(&path).expect("remove");
    std::fs::write(&path, b"second").expect("recreate");
    let second = open(&path);
    let second_id = second.observer().file_id().expect("file_id");

    assert_eq!(
        second_id.volume_serial_number,
        first_id.volume_serial_number
    );
    assert_ne!(second_id.file_id, first_id.file_id);

    drop(second);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_directory_has_a_file_id_distinct_from_a_file_inside_it() {
    let dir = temp_path("id-directory");
    std::fs::create_dir(&dir).expect("create directory");
    let inner = dir.join("inner.tmp");
    std::fs::write(&inner, b"inside").expect("write inner file");

    let dir_endpoint = UnassociatedEndpoint::open(&dir, true, false, FILE_FLAG_BACKUP_SEMANTICS)
        .expect("open directory endpoint");
    let file_endpoint = open(&inner);
    let dir_id = dir_endpoint.observer().file_id().expect("file_id");
    let file_id = file_endpoint.observer().file_id().expect("file_id");

    assert_eq!(dir_id.volume_serial_number, file_id.volume_serial_number);
    assert_ne!(dir_id.file_id, file_id.file_id);

    drop((dir_endpoint, file_endpoint));
    let _ = std::fs::remove_file(&inner);
    let _ = std::fs::remove_dir(&dir);
}

/// The NUL device answers the standard query but not the file-ID one, which is
/// how the shared query's error path is reached.
#[test]
fn a_device_without_file_ids_reports_the_error() {
    let nul = UnassociatedEndpoint::open(r"\\.\NUL", true, false, 0).expect("open NUL");
    let observer = nul.observer();

    let error = observer
        .file_id()
        .expect_err("the NUL device has no file ID");
    assert_eq!(
        error.raw_os_error(),
        Some(ERROR_INVALID_PARAMETER as i32),
        "{error:?}"
    );
    observer
        .standard_info()
        .expect("the same handle still answers the standard query");
}
