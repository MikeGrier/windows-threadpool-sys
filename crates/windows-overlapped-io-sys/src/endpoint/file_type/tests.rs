// Copyright (c) 2026 Mike Grier
use std::io;
use std::os::windows::io::{FromRawHandle, OwnedHandle};

use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_INVALID_HANDLE, INVALID_HANDLE_VALUE, NO_ERROR,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED, FILE_TYPE_CHAR, FILE_TYPE_DISK,
    FILE_TYPE_PIPE, FILE_TYPE_REMOTE, FILE_TYPE_UNKNOWN, PIPE_ACCESS_DUPLEX,
};
use windows_sys::Win32::System::Pipes::{CreateNamedPipeW, PIPE_TYPE_BYTE};

use super::classify;
use crate::{FileType, UnassociatedEndpoint};

fn temp_path(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "windows-overlapped-io-sys-file-type-{tag}-{}.tmp",
        std::process::id()
    ))
}

/// An overlapped named-pipe server endpoint, and the pipe's name.
fn pipe_server(tag: &str) -> (UnassociatedEndpoint, String) {
    let name = format!(
        r"\\.\pipe\windows-overlapped-io-sys-file-type-{tag}-{}",
        std::process::id()
    );
    let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    // SAFETY: a NUL-terminated name and no security attributes.
    let raw = unsafe {
        CreateNamedPipeW(
            wide.as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
            PIPE_TYPE_BYTE,
            1,
            4096,
            4096,
            0,
            std::ptr::null(),
        )
    };
    assert_ne!(
        raw,
        INVALID_HANDLE_VALUE,
        "CreateNamedPipeW: {}",
        io::Error::last_os_error()
    );
    // SAFETY: a fresh overlapped pipe, unassociated, unduplicated, moved in exclusively.
    let endpoint =
        unsafe { UnassociatedEndpoint::assume_overlapped(OwnedHandle::from_raw_handle(raw)) };
    (endpoint, name)
}

// --- classify: every branch, including those no real handle produces on demand ---

#[test]
fn each_documented_type_maps_to_its_variant() {
    for (raw, expected) in [
        (FILE_TYPE_DISK, FileType::Disk),
        (FILE_TYPE_CHAR, FileType::Character),
        (FILE_TYPE_PIPE, FileType::Pipe),
        (FILE_TYPE_REMOTE, FileType::Remote),
    ] {
        assert_eq!(classify(raw, NO_ERROR).expect("documented type"), expected);
    }
}

#[test]
fn a_documented_type_ignores_the_last_error() {
    for raw in [
        FILE_TYPE_DISK,
        FILE_TYPE_CHAR,
        FILE_TYPE_PIPE,
        FILE_TYPE_REMOTE,
    ] {
        assert!(classify(raw, ERROR_ACCESS_DENIED).is_ok(), "raw {raw:#x}");
    }
}

#[test]
fn unknown_with_no_error_is_the_unknown_type() {
    assert_eq!(
        classify(FILE_TYPE_UNKNOWN, NO_ERROR).expect("unknown"),
        FileType::Unknown
    );
}

#[test]
fn unknown_with_an_error_is_that_error() {
    for code in [ERROR_INVALID_HANDLE, ERROR_ACCESS_DENIED, 0xFFFF_FFFF] {
        let error = classify(FILE_TYPE_UNKNOWN, code).expect_err("an error");
        assert_eq!(
            error.raw_os_error(),
            Some(code.cast_signed()),
            "code {code:#x}"
        );
    }
}

#[test]
fn an_undocumented_value_is_carried_as_reported() {
    for raw in [4_u32, 0x7FFF, 0x8001, 0x8002, u32::MAX] {
        assert_eq!(
            classify(raw, NO_ERROR).expect("other"),
            FileType::Other(raw)
        );
        assert_eq!(
            classify(raw, ERROR_ACCESS_DENIED).expect("other"),
            FileType::Other(raw)
        );
    }
}

// --- real handles ---

#[test]
fn a_disk_file_is_disk() {
    let path = temp_path("disk");
    std::fs::write(&path, b"x").expect("write temp file");
    let endpoint = UnassociatedEndpoint::open(&path, true, false, 0).expect("open endpoint");
    assert_eq!(
        endpoint.observer().file_type().expect("file_type"),
        FileType::Disk
    );
    drop(endpoint);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_directory_is_disk() {
    let dir = temp_path("directory");
    std::fs::create_dir(&dir).expect("create directory");
    let endpoint = UnassociatedEndpoint::open(&dir, true, false, FILE_FLAG_BACKUP_SEMANTICS)
        .expect("open directory endpoint");
    assert_eq!(
        endpoint.observer().file_type().expect("file_type"),
        FileType::Disk
    );
    drop(endpoint);
    let _ = std::fs::remove_dir(&dir);
}

#[test]
fn the_nul_device_is_a_character_device() {
    let endpoint = UnassociatedEndpoint::open(r"\\.\NUL", true, false, 0).expect("open NUL");
    assert_eq!(
        endpoint.observer().file_type().expect("file_type"),
        FileType::Character
    );
}

#[test]
fn both_ends_of_a_named_pipe_are_pipes() {
    let (server, name) = pipe_server("ends");
    let client = UnassociatedEndpoint::open(&name, true, true, 0).expect("open the client end");

    assert_eq!(
        server.observer().file_type().expect("server"),
        FileType::Pipe
    );
    assert_eq!(
        client.observer().file_type().expect("client"),
        FileType::Pipe
    );
}

#[test]
fn an_observer_answers_after_its_endpoint_drops_and_clones_agree() {
    let (server, _name) = pipe_server("outlives");
    let observer = server.observer();
    let clone = observer.clone();
    drop(server);

    assert_eq!(observer.file_type().expect("file_type"), FileType::Pipe);
    assert_eq!(clone.file_type().expect("file_type"), FileType::Pipe);
}
