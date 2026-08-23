// Copyright (c) 2026 Mike Grier
//! End-to-end test of the blocking `GetOverlappedResult` backend.

#![cfg(windows)]

use std::io;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, OwnedHandle};

use windows_overlapped_io_sys::{
    BlockingEndpoint, Operation, OperationState, UnassociatedEndpoint,
};
use windows_sys::Win32::Foundation::ERROR_IO_PENDING;
use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_OVERLAPPED, ReadFile};

#[test]
fn reads_a_file_synchronously_via_get_overlapped_result() {
    let content = b"hello overlapped world";
    let path = std::env::temp_dir().join(format!(
        "windows-overlapped-io-sys-blocking-{}.tmp",
        std::process::id()
    ));
    std::fs::write(&path, content).expect("write file");

    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OVERLAPPED)
        .open(&path)
        .expect("open overlapped");
    let owned = OwnedHandle::from(file);
    // SAFETY: opened with FILE_FLAG_OVERLAPPED, unassociated, unique, exclusive.
    let endpoint = BlockingEndpoint::new(unsafe { UnassociatedEndpoint::assume_overlapped(owned) })
        .expect("no incompatible notification mode");

    let mut operation = Operation::new(());
    operation.set_offset(0);
    let mut buffer = [0_u8; 64];

    // SAFETY: issues exactly one overlapped ReadFile into `buffer`, which stays
    // valid for the whole blocking call; no other operation is outstanding.
    let read = unsafe {
        endpoint.run(&mut operation, |handle, overlapped| {
            let ok = ReadFile(
                handle.as_raw_handle(),
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                std::ptr::null_mut(),
                overlapped,
            );
            if ok != 0 {
                return Ok(());
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_IO_PENDING as i32) {
                Ok(())
            } else {
                Err(error)
            }
        })
    }
    .expect("run read");

    assert_eq!(read, content.len());
    assert_eq!(&buffer[..read], content);
    assert_eq!(operation.state(), OperationState::Completed);

    let _ = std::fs::remove_file(&path);
}
