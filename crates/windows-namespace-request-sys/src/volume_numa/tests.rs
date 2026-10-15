// Copyright (c) Mike Grier.

//! Tests for the `FSCTL_QUERY_VOLUME_NUMA_INFO` entry.
//!
//! This machine has one NUMA node, so what is testable here is the shapes: an answer, the
//! device's own failure, the same call on every kind of handle, and what the entry does with a
//! reply that is not the structure. Whether a volume on a multi-node machine reports the right
//! node is not.

use std::fs::{File, OpenOptions};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsHandle, AsRawHandle, FromRawHandle, OwnedHandle};
use std::ptr;

use windows_sys::Win32::Foundation::WAIT_TIMEOUT;
use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED};
use windows_sys::Win32::System::IO::{
    CreateIoCompletionPort, GetQueuedCompletionStatus, OVERLAPPED,
};

use super::{NumaInfoError, OUTPUT_SIZE, QueryVolumeNumaInfo, VolumeNumaInfo, decode};
use crate::handle::tests::{Fixture, handle_allocation};
use crate::request::Request;
use crate::{CapturedHandle, Win32Error};

fn request_for(file: &File) -> QueryVolumeNumaInfo {
    QueryVolumeNumaInfo::new(CapturedHandle::capture(file.as_handle()).expect("capture"))
}

/// What a query came to, in a form two of them can be compared by.
fn shape(outcome: Result<VolumeNumaInfo, NumaInfoError>) -> Result<u32, u32> {
    match outcome {
        Ok(info) => Ok(info.node()),
        Err(NumaInfoError::Win32(error)) => Err(error.code()),
        Err(other) => panic!("an unexpected failure: {other}"),
    }
}

fn overlapped(fixture: &Fixture) -> File {
    OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OVERLAPPED)
        .open(fixture.file())
        .expect("open the file overlapped")
}

// --- The query ------------------------------------------------------------------------------

#[test]
fn a_file_handle_gets_a_node_or_the_devices_own_failure() {
    let _allocating = handle_allocation().read().expect("not poisoned");
    let fixture = Fixture::new("numa-file");
    let file = fixture.open_file();

    let outcome = shape(request_for(&file).perform());
    match outcome {
        Ok(node) => eprintln!("a file on this volume: node {node}"),
        Err(code) => eprintln!("a file on this volume: no answer, error {code}"),
    }
}

#[test]
fn performing_twice_gives_the_same_answer() {
    let _allocating = handle_allocation().read().expect("not poisoned");
    let fixture = Fixture::new("numa-twice");
    let file = fixture.open_file();
    let request = request_for(&file);

    assert_eq!(shape(request.perform()), shape(request.perform()));
}

#[test]
fn a_directory_handle_gets_the_same_answer_as_a_file_on_that_volume() {
    // The documentation says the handle may be a file or a directory in the device, and the volume
    // is what is reported.
    let _allocating = handle_allocation().read().expect("not poisoned");
    let fixture = Fixture::new("numa-directory");
    let file = fixture.open_file();
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(fixture.directory())
        .expect("open the directory");

    assert_eq!(
        shape(request_for(&file).perform()),
        shape(request_for(&directory).perform())
    );
}

#[test]
fn an_overlapped_handle_gets_the_same_answer_as_a_plain_one() {
    let _allocating = handle_allocation().read().expect("not poisoned");
    let fixture = Fixture::new("numa-overlapped");
    let plain = fixture.open_file();
    let overlapped = overlapped(&fixture);

    assert_eq!(
        shape(request_for(&plain).perform()),
        shape(request_for(&overlapped).perform())
    );
}

#[test]
fn a_handle_that_belongs_to_a_completion_port_gets_no_packet_for_the_query() {
    let _allocating = handle_allocation().read().expect("not poisoned");
    let fixture = Fixture::new("numa-port");
    let file = overlapped(&fixture);

    // SAFETY: the file handle is open; a null existing port makes a new one with this handle
    // associated.
    let port = unsafe { CreateIoCompletionPort(file.as_raw_handle(), ptr::null_mut(), 1, 0) };
    assert!(!port.is_null(), "make a completion port");
    // SAFETY: the port was just created and nothing else owns it.
    let port = unsafe { OwnedHandle::from_raw_handle(port) };

    let _ = request_for(&file).perform();

    let mut bytes = 0_u32;
    let mut key = 0_usize;
    let mut packet: *mut OVERLAPPED = ptr::null_mut();
    // SAFETY: the port is open and the three out-parameters are valid locals.
    let dequeued = unsafe {
        GetQueuedCompletionStatus(port.as_raw_handle(), &mut bytes, &mut key, &mut packet, 0)
    };
    assert_eq!(dequeued, 0, "a packet was queued to the handle's port");
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(i32::try_from(WAIT_TIMEOUT).expect("fits")),
        "nothing should be waiting"
    );
}

#[test]
fn a_device_that_is_not_a_volume_is_the_devices_own_error() {
    let _allocating = handle_allocation().read().expect("not poisoned");
    let nul = File::open("NUL").expect("open the NUL device");

    match request_for(&nul).perform() {
        Err(NumaInfoError::Win32(error)) => eprintln!("the NUL device: {error}"),
        other => panic!("the NUL device has no NUMA node, but gave {other:?}"),
    }
}

// --- The request ----------------------------------------------------------------------------

#[test]
fn a_clone_answers_the_same_after_the_original_is_gone() {
    let _allocating = handle_allocation().read().expect("not poisoned");
    let fixture = Fixture::new("numa-clone");
    let file = fixture.open_file();
    let original = request_for(&file);
    let expected = shape(original.perform());

    let copy = original.try_clone().expect("duplicate the handle");
    drop(original);
    drop(file);

    assert_eq!(shape(copy.perform()), expected);
}

#[test]
fn the_request_owns_a_duplicate_of_the_handle_it_was_given() {
    let _allocating = handle_allocation().read().expect("not poisoned");
    let fixture = Fixture::new("numa-owns");
    let file = fixture.open_file();
    let request = request_for(&file);

    assert_ne!(request.handle().raw(), file.as_raw_handle());
}

#[test]
fn the_request_is_performable_through_the_trait() {
    fn through_the_seam<R: Request<Output = VolumeNumaInfo, Error = NumaInfoError>>(
        request: &R,
    ) -> Result<u32, u32> {
        shape(request.perform())
    }

    let _allocating = handle_allocation().read().expect("not poisoned");
    let fixture = Fixture::new("numa-seam");
    let file = fixture.open_file();
    let request = request_for(&file);

    assert_eq!(through_the_seam(&request), shape(request.perform()));
}

// --- What the entry does with a reply --------------------------------------------------------

#[test]
fn a_reply_of_exactly_the_structure_is_read_as_the_node() {
    for node in [0_u32, 1, 2, 63, 255, 0x1234_5678] {
        let info = decode(4, node.to_ne_bytes()).expect("a whole structure");
        assert_eq!(info.node(), node);
    }
}

#[test]
fn a_value_that_is_not_a_node_is_carried_as_it_was_written() {
    // Whether `u32::MAX` means "no node" is not documented, so it is not decided here.
    assert_eq!(
        decode(4, u32::MAX.to_ne_bytes())
            .expect("a whole structure")
            .node(),
        u32::MAX
    );
}

#[test]
fn a_reply_of_any_other_size_is_not_read() {
    for returned in [0_u32, 1, 2, 3, 5, 8, u32::MAX] {
        // The buffer holds something that would read as node 7, so a read would show.
        let outcome = decode(returned, 7_u32.to_ne_bytes());
        assert!(
            matches!(outcome, Err(NumaInfoError::Unexpected { returned: seen }) if seen == returned),
            "a reply of {returned} bytes: {outcome:?}"
        );
    }
}

#[test]
fn the_output_is_one_ulong() {
    assert_eq!(OUTPUT_SIZE, 4);
}

// --- Errors ---------------------------------------------------------------------------------

#[test]
fn errors_name_the_call_and_keep_the_raw_code() {
    let refused = NumaInfoError::from(Win32Error::from_code(1));
    assert!(
        refused
            .to_string()
            .starts_with("FSCTL_QUERY_VOLUME_NUMA_INFO: ")
    );
    let source = std::error::Error::source(&refused).expect("a Win32 source");
    assert_eq!(
        source
            .downcast_ref::<Win32Error>()
            .map(|error| error.code()),
        Some(1)
    );

    let short = NumaInfoError::Unexpected { returned: 2 };
    let text = short.to_string();
    assert!(text.contains("2 bytes") && text.contains('4'), "{text}");
    assert!(std::error::Error::source(&short).is_none());
}

#[test]
fn a_node_displays_as_its_number() {
    assert_eq!(
        decode(4, 3_u32.to_ne_bytes()).expect("whole").to_string(),
        "NUMA node 3"
    );
}
