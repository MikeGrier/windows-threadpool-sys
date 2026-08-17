// Copyright (c) 2026 Mike Grier
use super::UnassociatedEndpoint;
use std::fs::OpenOptions;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, OwnedHandle};

const FILE_FLAG_OVERLAPPED: u32 = 0x4000_0000;

#[test]
fn borrows_and_reclaims_the_same_handle() {
    let path = std::env::temp_dir().join(format!(
        "windows-overlapped-io-sys-{}.tmp",
        std::process::id()
    ));
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .custom_flags(FILE_FLAG_OVERLAPPED)
        .open(&path)
        .expect("create overlapped temp file");
    let owned = OwnedHandle::from(file);
    let expected = owned.as_raw_handle();

    // SAFETY: the file was just created with FILE_FLAG_OVERLAPPED, is not
    // associated with any completion port, has no duplicates, and its
    // ownership moves into the endpoint.
    let endpoint = unsafe { UnassociatedEndpoint::assume_overlapped(owned) };
    assert_eq!(endpoint.handle().as_raw_handle(), expected);

    let recovered = endpoint.into_handle();
    assert_eq!(recovered.as_raw_handle(), expected);
    drop(recovered);

    let _ = std::fs::remove_file(&path);
}
