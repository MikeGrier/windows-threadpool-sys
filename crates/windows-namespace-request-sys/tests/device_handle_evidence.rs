// Copyright (c) Mike Grier.
//! Evidence for `DI-14.6`: what an open handle reports about its own kind, over the handle kinds a
//! path can reach.
//!
//! These are measurements, not the contract. `GetFileType`'s return values are documented; what
//! `IOCTL_STORAGE_GET_DEVICE_NUMBER` does on each kind is observed, and the assertions below pin
//! only whether it succeeds, never which error a failure carries. A refusal built on this would
//! rest on the documentation, not on this file.
//!
//! Both are asked through the crates that wrap them, [`QueryFileType`] here and
//! `win_volume_sys::storage_device_number`, so this file makes no unsafe call of its own to read a
//! handle.
//!
//! The handles come from [`OpenFile`], the opener a consumer uses, so the observations are of what
//! that opener returns. Console and serial-port handles are not here: a test host may have neither,
//! and a serial open was measured blocking indefinitely on one machine (see the crate's design
//! notes).
#![cfg(windows)]

use std::os::windows::io::{AsHandle, OwnedHandle};

use win_volume_sys::{StorageDeviceNumber, storage_device_number};
use windows_namespace_request_sys::{
    CapturedHandle, CloseRequest, FileType, OpenFile, QueryFileType,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    OPEN_EXISTING, PIPE_ACCESS_DUPLEX,
};
use windows_sys::Win32::System::Pipes::{CreateNamedPipeW, PIPE_WAIT};
use wtf_string::Wtf16String;

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const NO_ACCESS: u32 = 0;

fn open(path: &str, access: u32, flags: u32) -> OwnedHandle {
    OpenFile::new(Wtf16String::from(path))
        .expect("the path has no NUL")
        .with_desired_access(access)
        .with_share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .with_creation_disposition(OPEN_EXISTING)
        .with_flags_and_attributes(flags)
        .perform()
        .unwrap_or_else(|error| panic!("open {path}: {error:?}"))
}

fn file_type(handle: &OwnedHandle) -> FileType {
    QueryFileType::new(CapturedHandle::capture(handle.as_handle()).expect("capture the handle"))
        .perform()
        .expect("query the handle's type")
}

fn device_number(handle: &OwnedHandle) -> Option<StorageDeviceNumber> {
    storage_device_number(handle).ok()
}

fn close(handle: OwnedHandle) {
    CloseRequest::for_handle(handle).perform().expect("close");
}

#[test]
fn a_disk_file_is_disk_and_has_no_device_number() {
    let path = std::env::temp_dir().join(format!("di146-file-{}.tmp", std::process::id()));
    std::fs::write(&path, b"x").expect("create the file");
    let handle = open(
        path.to_str().expect("a UTF-8 temp path"),
        GENERIC_READ | GENERIC_WRITE,
        0,
    );

    assert_eq!(file_type(&handle), FileType::DISK);
    assert!(device_number(&handle).is_none());

    close(handle);
    std::fs::remove_file(&path).expect("remove the file");
}

#[test]
fn a_directory_is_disk_and_has_no_device_number() {
    let directory = std::env::temp_dir();
    let handle = open(
        directory.to_str().expect("a UTF-8 temp path"),
        GENERIC_READ,
        FILE_FLAG_BACKUP_SEMANTICS,
    );

    assert_eq!(file_type(&handle), FileType::DISK);
    assert!(device_number(&handle).is_none());

    close(handle);
}

#[test]
fn nul_is_char_and_has_no_device_number() {
    let handle = open(r"\\.\NUL", GENERIC_READ | GENERIC_WRITE, 0);

    assert_eq!(file_type(&handle), FileType::CHAR);
    assert!(device_number(&handle).is_none());

    close(handle);
}

#[test]
fn a_named_pipe_is_pipe_and_has_no_device_number() {
    let name = format!(r"\\.\pipe\di146-evidence-{}", std::process::id());
    let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
    // SAFETY: `wide` is NUL-terminated and outlives the call.
    let server = unsafe {
        CreateNamedPipeW(
            wide.as_ptr(),
            PIPE_ACCESS_DUPLEX,
            PIPE_WAIT,
            1,
            4096,
            4096,
            0,
            std::ptr::null(),
        )
    };
    assert_ne!(server, windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE);
    // SAFETY: `server` is a handle this call owns.
    let server = unsafe { std::os::windows::io::FromRawHandle::from_raw_handle(server) };
    let client = open(&name, GENERIC_READ | GENERIC_WRITE, 0);

    for end in [&server, &client] {
        assert_eq!(file_type(end), FileType::PIPE);
        assert!(device_number(end).is_none());
    }

    close(client);
    close(server);
}

#[test]
fn a_volume_is_disk_like_a_file_but_has_a_device_number() {
    // No access is requested: a device-number query needs none, and this keeps the test from
    // needing elevation.
    let volume = open(r"\\.\C:", NO_ACCESS, 0);

    assert_eq!(file_type(&volume), FileType::DISK);
    let number = device_number(&volume).expect("a volume reports its device number");
    assert_ne!(number.partition_number, 0, "a volume is a partition");

    close(volume);
}

#[test]
fn a_physical_disk_is_disk_like_a_file_but_has_a_device_number() {
    let volume = open(r"\\.\C:", NO_ACCESS, 0);
    let number = device_number(&volume).expect("a volume reports its device number");
    close(volume);

    let disk = open(
        &format!(r"\\.\PhysicalDrive{}", number.device_number),
        NO_ACCESS,
        0,
    );

    assert_eq!(file_type(&disk), FileType::DISK);
    let own = device_number(&disk).expect("a disk reports its device number");
    assert_eq!(own.device_number, number.device_number);

    close(disk);
}
