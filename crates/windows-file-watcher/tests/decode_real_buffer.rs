// Copyright (c) 2026 Mike Grier
//! Integration test: decode a buffer produced by a real, blocking
//! `ReadDirectoryChangesW` call. The watch is armed on a worker thread and the
//! change is made from the test thread after a short delay, so a missed change
//! surfaces as a timeout failure rather than a wedged run.
#![cfg(windows)]

use std::ffi::OsString;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr;
use std::sync::mpsc;
use std::time::Duration;

use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE_FILE_NAME,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, ReadDirectoryChangesW,
};

use windows_file_watcher::{ChangeKind, DecodedBatch, decode_batch};

/// The NUL-terminated wide form of a path, for `CreateFileW`.
fn wide_z(p: &Path) -> Vec<u16> {
    p.as_os_str().encode_wide().chain(Some(0)).collect()
}

#[test]
fn decodes_a_real_read_directory_changes_buffer() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "windows-file-watcher-decode-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir(&dir).expect("create temp dir");

    let (tx, rx) = mpsc::channel::<Result<Vec<(ChangeKind, OsString)>, String>>();
    let watch_dir = dir.clone();
    std::thread::spawn(move || {
        let _ = tx.send(watch_once(&watch_dir));
    });

    // Give the worker time to open the directory and arm the read, then make the
    // change it must observe.
    std::thread::sleep(Duration::from_millis(500));
    std::fs::write(dir.join("created.txt"), b"hi").expect("create file");

    let changes = rx
        .recv_timeout(Duration::from_secs(30))
        .expect("the watch must report a change")
        .expect("the completion must decode to changes, not a desync");

    assert!(
        changes.iter().any(|(kind, name)| {
            *kind == ChangeKind::Added && name == &OsString::from("created.txt")
        }),
        "expected an Added record for created.txt; got {changes:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Open `dir`, arm one blocking `ReadDirectoryChangesW`, and decode the
/// completion into `(kind, name)` pairs.
fn watch_once(dir: &Path) -> Result<Vec<(ChangeKind, OsString)>, String> {
    let name = wide_z(dir);
    // SAFETY: `name` is a valid NUL-terminated wide path; the returned handle is
    // closed exactly once below before this function returns.
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            FILE_LIST_DIRECTORY,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(format!(
            "CreateFileW failed: {}",
            std::io::Error::last_os_error()
        ));
    }

    // A `Vec<u32>` guarantees the DWORD alignment `ReadDirectoryChangesW` requires.
    let mut buffer = vec![0_u32; 1024];
    let mut bytes_returned: u32 = 0;
    // SAFETY: `buffer` and the out-parameters are valid for the call; a null
    // overlapped makes the call synchronous, blocking until a change arrives.
    let ok = unsafe {
        ReadDirectoryChangesW(
            handle,
            buffer.as_mut_ptr().cast(),
            (buffer.len() * 4) as u32,
            0, // bWatchSubtree = FALSE
            FILE_NOTIFY_CHANGE_FILE_NAME,
            &mut bytes_returned,
            ptr::null_mut(),
            None,
        )
    };

    let result = if ok == 0 {
        Err(format!(
            "ReadDirectoryChangesW failed: {}",
            std::io::Error::last_os_error()
        ))
    } else {
        // SAFETY: the kernel wrote `bytes_returned` bytes into `buffer`.
        let bytes = unsafe {
            std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), bytes_returned as usize)
        };
        match decode_batch(bytes) {
            DecodedBatch::Changes(changes) => Ok(changes
                .into_iter()
                .map(|c| (c.kind, c.name.to_os_string()))
                .collect()),
            DecodedBatch::Desync(cause) => Err(format!("unexpected desync: {cause:?}")),
        }
    };

    // SAFETY: `handle` came from `CreateFileW` and is closed exactly once.
    unsafe { CloseHandle(handle) };
    result
}
