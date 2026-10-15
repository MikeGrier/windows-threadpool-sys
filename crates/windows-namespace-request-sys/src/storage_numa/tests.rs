// Copyright (c) Mike Grier.

//! Tests for the `StorageDeviceNumaProperty` entry.
//!
//! This machine has one NUMA node, so what is testable here is the shapes: an answer, the device's
//! own failure, the size protocol this property follows, and what the entry does with a reply that
//! is not the descriptor. Whether a disk on a multi-node machine reports the right node is not.

use std::fs::File;
use std::mem::{offset_of, size_of};
use std::os::windows::io::{AsHandle, AsRawHandle};

use windows_sys::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
use windows_sys::Win32::System::Ioctl::{
    IOCTL_STORAGE_QUERY_PROPERTY, PropertyStandardQuery, STORAGE_DEVICE_NUMA_PROPERTY,
    StorageDeviceNumaProperty,
};

use super::{QueryStorageDeviceNumaNode, StorageDeviceNuma};
use crate::device_control::device_control;
use crate::handle::tests::{Fixture, handle_allocation};
use crate::request::Request;
use crate::storage_property::tests::{open_disk, system_disk, system_disk_numbers};
use crate::storage_property::{
    FIRST_BYTES, HEADER, StoragePropertyError, query_input, query_property,
};
use crate::{CapturedHandle, Win32Error};

const DESCRIPTOR: usize = size_of::<STORAGE_DEVICE_NUMA_PROPERTY>();

fn request_for(file: &File) -> QueryStorageDeviceNumaNode {
    QueryStorageDeviceNumaNode::new(CapturedHandle::capture(file.as_handle()).expect("capture"))
}

fn input() -> Vec<u8> {
    query_input(StorageDeviceNumaProperty, PropertyStandardQuery, &[]).expect("a small input")
}

/// A reply with `version`, `size` and `node` in the places the structure has them, and `extra`
/// bytes after it.
fn reply(version: u32, size: u32, node: u32, extra: usize) -> Vec<u8> {
    let mut bytes = vec![0_u8; DESCRIPTOR + extra];
    let mut put = |offset: usize, value: u32| {
        bytes[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
    };
    put(offset_of!(STORAGE_DEVICE_NUMA_PROPERTY, Version), version);
    put(offset_of!(STORAGE_DEVICE_NUMA_PROPERTY, Size), size);
    put(offset_of!(STORAGE_DEVICE_NUMA_PROPERTY, NumaNode), node);
    bytes
}

// --- The real disk --------------------------------------------------------------------------

#[test]
fn a_real_disk_reports_a_whole_descriptor_and_a_node() {
    let _allocating = handle_allocation().read().expect("not poisoned");
    let disk = system_disk(false);

    let numa = request_for(&disk).perform().expect("query the disk");

    assert!(
        usize::try_from(numa.size()).expect("fits") >= DESCRIPTOR,
        "the descriptor says it is at least as big as the structure: {numa:?}"
    );
    eprintln!("the system disk: {numa}, version {}", numa.version());
}

#[test]
fn every_disk_a_volume_is_on_answers_for_itself() {
    // The point of asking per disk: a volume that spans disks has a node for each, and the disks
    // come from the volume's own extents.
    let _allocating = handle_allocation().read().expect("not poisoned");

    let numbers = system_disk_numbers();
    assert!(!numbers.is_empty());
    for number in numbers {
        let disk = open_disk(number, false);
        let numa = request_for(&disk)
            .perform()
            .unwrap_or_else(|error| panic!("disk {number}: {error}"));
        eprintln!("disk {number}: {numa}");
    }
}

#[test]
fn every_first_buffer_size_gets_the_same_reply_from_a_real_disk() {
    // 8 bytes up to one short of the reply is answered with the header alone for this property,
    // and a larger one with the reply: all of them end at the same bytes.
    let _allocating = handle_allocation().read().expect("not poisoned");
    let disk = system_disk(false);
    let input = input();

    let whole =
        query_property(disk.as_raw_handle(), &input, FIRST_BYTES).expect("the usual first size");
    assert!(whole.len() >= DESCRIPTOR);

    for first in [
        0,
        1,
        HEADER - 1,
        HEADER,
        HEADER + 1,
        DESCRIPTOR - 1,
        DESCRIPTOR,
        DESCRIPTOR + 1,
        4096,
    ] {
        let again = query_property(disk.as_raw_handle(), &input, first)
            .unwrap_or_else(|error| panic!("a first buffer of {first} bytes: {error}"));
        assert_eq!(again, whole, "a first buffer of {first} bytes");
    }
}

#[test]
fn this_property_answers_a_short_buffer_differently_from_the_unique_id_property() {
    // Measured, and the reason the size protocol cannot be assumed from one property: below the
    // header this is ERROR_INSUFFICIENT_BUFFER (the unique-id property says ERROR_BAD_LENGTH), and
    // between the header and the reply it is a success with the header alone (the unique-id
    // property says ERROR_MORE_DATA).
    let _allocating = handle_allocation().read().expect("not poisoned");
    let disk = system_disk(false);
    let input = input();
    let ask = |size: usize| {
        device_control(
            disk.as_raw_handle(),
            IOCTL_STORAGE_QUERY_PROPERTY,
            &input,
            &mut vec![0_u8; size],
        )
        .map_err(|error| error.code())
    };

    for size in [0_usize, 1, HEADER - 1] {
        assert_eq!(
            ask(size),
            Err(ERROR_INSUFFICIENT_BUFFER),
            "an output of {size} bytes"
        );
    }
    for size in HEADER..DESCRIPTOR {
        assert_eq!(
            ask(size),
            Ok(u32::try_from(HEADER).expect("small")),
            "an output of {size} bytes gets the header alone"
        );
    }
    assert_eq!(
        ask(DESCRIPTOR),
        Ok(u32::try_from(DESCRIPTOR).expect("small"))
    );
}

#[test]
fn a_plain_disk_handle_and_an_overlapped_one_get_the_same_answer() {
    let _allocating = handle_allocation().read().expect("not poisoned");
    let plain = system_disk(false);
    let overlapped = system_disk(true);

    assert_eq!(
        request_for(&plain).perform().expect("plain"),
        request_for(&overlapped).perform().expect("overlapped")
    );
}

#[test]
fn performing_twice_and_through_a_clone_gives_the_same_answer() {
    let _allocating = handle_allocation().read().expect("not poisoned");
    let disk = system_disk(false);
    let request = request_for(&disk);

    let first = request.perform().expect("query");
    let copy = request.try_clone().expect("duplicate the handle");
    drop(request);
    drop(disk);

    assert_eq!(copy.perform().expect("query the copy"), first);
}

#[test]
fn a_handle_that_is_not_a_disk_is_the_devices_own_error() {
    let _allocating = handle_allocation().read().expect("not poisoned");
    let fixture = Fixture::new("sn-file");
    let file = fixture.open_file();

    match request_for(&file).perform() {
        Err(StoragePropertyError::Win32(error)) => eprintln!("an ordinary file: {error}"),
        other => panic!("an ordinary file is not a disk, but gave {other:?}"),
    }
}

#[test]
fn the_request_owns_its_handle_and_is_performable_through_the_trait() {
    fn through_the_seam<R: Request<Output = StorageDeviceNuma, Error = StoragePropertyError>>(
        request: &R,
    ) -> StorageDeviceNuma {
        request.perform().expect("perform")
    }

    let _allocating = handle_allocation().read().expect("not poisoned");
    let disk = system_disk(false);
    let request = request_for(&disk);

    assert_ne!(request.handle().raw(), disk.as_raw_handle());
    assert_eq!(
        through_the_seam(&request),
        request.perform().expect("perform")
    );
}

// --- What the entry does with a reply --------------------------------------------------------

#[test]
fn each_field_is_read_from_where_the_structure_has_it() {
    let numa = StorageDeviceNuma::decode(&reply(0x1111_1111, 0x2222_2222, 0x3333_3333, 0))
        .expect("a whole descriptor");

    assert_eq!(numa.version(), 0x1111_1111);
    assert_eq!(numa.size(), 0x2222_2222);
    assert_eq!(numa.node(), 0x3333_3333);
}

#[test]
fn a_node_is_carried_as_the_device_wrote_it() {
    // Including a value that is not a node of any machine: whether it means none is not decided here.
    for node in [0_u32, 1, 2, 7, 63, 255, 0x1234_5678, u32::MAX - 1, u32::MAX] {
        let numa = StorageDeviceNuma::decode(&reply(1, 12, node, 0)).expect("whole");
        assert_eq!(numa.node(), node);
    }
}

#[test]
fn a_reply_longer_than_the_structure_is_read_for_the_fields_it_begins_with() {
    let numa = StorageDeviceNuma::decode(&reply(2, 20, 5, 8)).expect("a later version");

    assert_eq!((numa.version(), numa.size(), numa.node()), (2, 20, 5));
}

#[test]
fn a_reply_shorter_than_the_structure_is_not_read() {
    let whole = reply(1, 12, 3, 0);

    for length in 0..DESCRIPTOR {
        assert!(
            matches!(
                StorageDeviceNuma::decode(&whole[..length]),
                Err(StoragePropertyError::Malformed { .. })
            ),
            "a reply of {length} bytes"
        );
    }
}

#[test]
fn a_node_displays_as_its_number() {
    assert_eq!(
        StorageDeviceNuma::decode(&reply(1, 12, 3, 0))
            .expect("whole")
            .to_string(),
        "NUMA node 3"
    );
}

#[test]
fn an_error_from_the_helper_is_carried_unaltered() {
    let error = StoragePropertyError::from(Win32Error::from_code(122));
    assert!(matches!(error, StoragePropertyError::Win32(inner) if inner.code() == 122));
}
