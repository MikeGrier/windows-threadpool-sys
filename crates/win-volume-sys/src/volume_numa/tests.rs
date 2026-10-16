// Copyright (c) 2026 Mike Grier
//! Tests for the volume NUMA entry.
//!
//! The machine these run on has one NUMA node, so what is testable here is the shapes: an answer,
//! the device's own failure, the same call on every kind of handle, and what the entry does with a
//! reply that is not the structure. Whether a volume on a multi-node machine reports the right node
//! is not.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::ptr;

use win_error_codes::Win32Error;
use windows_sys::Win32::Foundation::{
    ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_PARAMETER, ERROR_MORE_DATA, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED};
use windows_sys::Win32::System::IO::{
    CreateIoCompletionPort, GetQueuedCompletionStatus, OVERLAPPED,
};

use super::{OUTPUT_SIZE, decode, volume_numa_node};
use crate::test_support::ordinary_file;

/// What a query came to, in a form two of them can be compared by: the node, or the device's code.
fn shape(outcome: io::Result<u32>) -> Result<u32, u32> {
    match outcome {
        Ok(node) => Ok(node),
        Err(error) => Err(Win32Error::from_io(&error)
            .unwrap_or_else(|| panic!("an error with no OS code: {error}"))
            .code()),
    }
}

fn directory() -> File {
    OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(std::env::temp_dir())
        .expect("open the temp directory")
}

fn overlapped_file(tag: &str) -> (File, std::path::PathBuf) {
    let (file, path) = ordinary_file(tag);
    drop(file);
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OVERLAPPED)
        .open(&path)
        .expect("open the file overlapped");
    (file, path)
}

// --- The query ------------------------------------------------------------------------------

#[test]
fn a_file_handle_gets_a_node_or_the_devices_own_failure() {
    let (file, path) = ordinary_file("numa-file");

    match shape(volume_numa_node(&file)) {
        Ok(node) => eprintln!("a file on this volume: node {node}"),
        Err(code) => eprintln!("a file on this volume: no answer, error {code}"),
    }
    drop(file);
    std::fs::remove_file(path).expect("remove");
}

#[test]
fn a_failure_is_never_about_the_buffer() {
    // The call is made with exactly the room the structure needs, so a refusal for want of room
    // would mean the buffer was the wrong size, whether or not this machine's volume answers.
    let (file, path) = ordinary_file("numa-room");

    if let Err(code) = shape(volume_numa_node(&file)) {
        for about_room in [
            ERROR_INSUFFICIENT_BUFFER,
            ERROR_MORE_DATA,
            ERROR_INVALID_PARAMETER,
        ] {
            assert_ne!(code, about_room, "refused for want of room");
        }
    }
    drop(file);
    std::fs::remove_file(path).expect("remove");
}

#[test]
fn asking_twice_gives_the_same_answer() {
    let (file, path) = ordinary_file("numa-twice");

    assert_eq!(
        shape(volume_numa_node(&file)),
        shape(volume_numa_node(&file))
    );
    drop(file);
    std::fs::remove_file(path).expect("remove");
}

#[test]
fn a_directory_handle_gets_the_same_answer_as_a_file_on_that_volume() {
    // The documentation says the handle may be a file or a directory in the device, and the volume
    // is what is reported. The temp directory holds the file.
    let (file, path) = ordinary_file("numa-directory");

    assert_eq!(
        shape(volume_numa_node(&file)),
        shape(volume_numa_node(&directory()))
    );
    drop(file);
    std::fs::remove_file(path).expect("remove");
}

#[test]
fn an_overlapped_handle_gets_the_same_answer_as_a_plain_one() {
    let (plain, path) = ordinary_file("numa-plain");
    let (overlapped, overlapped_path) = overlapped_file("numa-overlapped");

    assert_eq!(
        shape(volume_numa_node(&plain)),
        shape(volume_numa_node(&overlapped))
    );
    drop((plain, overlapped));
    std::fs::remove_file(path).expect("remove");
    std::fs::remove_file(overlapped_path).expect("remove");
}

#[test]
fn a_handle_that_belongs_to_a_completion_port_gets_no_packet_for_the_query() {
    let (file, path) = overlapped_file("numa-port");

    // SAFETY: the file handle is open; a null existing port makes a new one with this handle
    // associated.
    let port = unsafe { CreateIoCompletionPort(file.as_raw_handle(), ptr::null_mut(), 1, 0) };
    assert!(!port.is_null(), "make a completion port");
    // SAFETY: the port was just created and nothing else owns it.
    let port = unsafe { OwnedHandle::from_raw_handle(port) };

    let _ = volume_numa_node(&file);

    let mut bytes = 0_u32;
    let mut key = 0_usize;
    let mut packet: *mut OVERLAPPED = ptr::null_mut();
    // SAFETY: the port is open and the three out-parameters are valid locals.
    let dequeued = unsafe {
        GetQueuedCompletionStatus(port.as_raw_handle(), &mut bytes, &mut key, &mut packet, 0)
    };
    assert_eq!(dequeued, 0, "a packet was queued to the handle's port");
    assert_eq!(
        io::Error::last_os_error().raw_os_error(),
        Some(i32::try_from(WAIT_TIMEOUT).expect("fits")),
        "nothing should be waiting"
    );
    drop((port, file));
    std::fs::remove_file(path).expect("remove");
}

#[test]
fn a_device_that_is_not_a_volume_is_the_devices_own_error() {
    let nul = File::open("NUL").expect("open the NUL device");

    let error = volume_numa_node(&nul).expect_err("the NUL device has no NUMA node");
    let code = Win32Error::from_io(&error).expect("the device's own code");
    eprintln!("the NUL device: {code}");
}

// --- What the entry does with a reply --------------------------------------------------------

#[test]
fn a_reply_of_exactly_the_structure_is_read_as_the_node() {
    for node in [0_u32, 1, 2, 63, 255, 256, 0x1234_5678, 0x8000_0000] {
        assert_eq!(
            decode(OUTPUT_SIZE, node.to_ne_bytes()).expect("whole"),
            node
        );
    }
}

#[test]
fn a_value_that_is_not_a_node_is_carried_as_it_was_written() {
    // Whether `u32::MAX` means "no node" is not documented, so it is not decided here.
    assert_eq!(
        decode(OUTPUT_SIZE, u32::MAX.to_ne_bytes()).expect("whole"),
        u32::MAX
    );
}

#[test]
fn a_reply_of_any_other_size_is_not_read() {
    for written in [0_usize, 1, 2, 3, 5, 8, usize::MAX] {
        // The buffer holds something that would read as node 7, so a read would show.
        let error = decode(written, 7_u32.to_ne_bytes()).expect_err("not the structure");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData, "{written} bytes");
        let text = error.to_string();
        assert!(
            text.contains(&format!("{written} bytes")) && text.contains("FSCTL_QUERY_VOLUME"),
            "{text}"
        );
    }
}

#[test]
fn the_output_is_one_ulong() {
    assert_eq!(OUTPUT_SIZE, 4);
}
