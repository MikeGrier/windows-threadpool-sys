// Copyright (c) 2026 Mike Grier
//! Tests for the disk NUMA node.
//!
//! The machine these run on has one NUMA node, so what is testable here is the shapes: an answer,
//! the device's own failure, the size protocol this property follows, and what the function does
//! with a reply that is not the descriptor. Whether a disk on a multi-node machine reports the
//! right node is not.

use std::io;
use std::mem::{offset_of, size_of};

use win_error_codes::Win32Error;
use windows_sys::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
use windows_sys::Win32::System::Ioctl::{
    IOCTL_STORAGE_QUERY_PROPERTY, PropertyStandardQuery, STORAGE_DEVICE_NUMA_PROPERTY,
    StorageDeviceNumaProperty,
};

use super::{StorageDeviceNuma, storage_device_numa};
use crate::device_control::device_control;
use crate::storage_property::{FIRST_BYTES, HEADER, query_input, query_property};
use crate::test_support::{open_disk, ordinary_file, system_disk, system_disk_numbers};

const DESCRIPTOR: usize = size_of::<STORAGE_DEVICE_NUMA_PROPERTY>();

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
    let disk = system_disk(false);

    let numa = storage_device_numa(&disk).expect("query the disk");

    // Twelve bytes on the machine measured: the structure and nothing after it. A property's reply
    // is told from another's by this, since the unique-id property's is larger.
    assert_eq!(
        usize::try_from(numa.size).expect("fits"),
        DESCRIPTOR,
        "the descriptor is the structure: {numa:?}"
    );
    eprintln!("the system disk: {numa}, version {}", numa.version);
}

#[test]
fn every_disk_a_volume_is_on_answers_for_itself() {
    // The point of asking per disk: a volume that spans disks has a node for each, and the disks
    // come from the volume's own extents.
    let numbers = system_disk_numbers();
    assert!(!numbers.is_empty());
    for number in numbers {
        let disk = open_disk(number, false);
        let numa =
            storage_device_numa(&disk).unwrap_or_else(|error| panic!("disk {number}: {error}"));
        eprintln!("disk {number}: {numa}");
    }
}

#[test]
fn every_first_buffer_size_gets_the_same_reply_from_a_real_disk() {
    // 8 bytes up to one short of the reply is answered with the header alone for this property,
    // and a larger one with the reply: all of them end at the same bytes.
    let disk = system_disk(false);
    let input = input();

    let whole = query_property(&disk, &input, FIRST_BYTES).expect("the usual first size");
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
        let again = query_property(&disk, &input, first)
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
    let disk = system_disk(false);
    let input = input();
    let ask = |size: usize| {
        device_control(
            &disk,
            IOCTL_STORAGE_QUERY_PROPERTY,
            &input,
            &mut vec![0_u8; size],
        )
        .map_err(|error| Win32Error::from_io(&error).expect("an OS code").code())
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
            Ok(HEADER),
            "an output of {size} bytes gets the header alone"
        );
    }
    assert_eq!(ask(DESCRIPTOR), Ok(DESCRIPTOR));
}

#[test]
fn a_plain_disk_handle_and_an_overlapped_one_get_the_same_answer() {
    let plain = system_disk(false);
    let overlapped = system_disk(true);

    assert_eq!(
        storage_device_numa(&plain).expect("plain"),
        storage_device_numa(&overlapped).expect("overlapped")
    );
}

#[test]
fn asking_twice_gives_the_same_answer() {
    let disk = system_disk(false);

    assert_eq!(
        storage_device_numa(&disk).expect("first"),
        storage_device_numa(&disk).expect("second")
    );
}

#[test]
fn a_handle_that_is_not_a_disk_is_the_devices_own_error() {
    let (file, path) = ordinary_file("sn-file");

    let error = storage_device_numa(&file).expect_err("an ordinary file is not a disk");
    assert_ne!(error.kind(), io::ErrorKind::InvalidData, "{error}");
    assert!(error.raw_os_error().is_some(), "{error}");
    eprintln!("an ordinary file: {error}");
    drop(file);
    std::fs::remove_file(path).expect("remove");
}

// --- What the function does with a reply -----------------------------------------------------

#[test]
fn each_field_is_read_from_where_the_structure_has_it() {
    let numa = StorageDeviceNuma::decode(&reply(0x1111_1111, 0x2222_2222, 0x3333_3333, 0))
        .expect("a whole descriptor");

    assert_eq!(numa.version, 0x1111_1111);
    assert_eq!(numa.size, 0x2222_2222);
    assert_eq!(numa.node, 0x3333_3333);
}

#[test]
fn a_node_is_carried_as_the_device_wrote_it() {
    // Including a value that is not a node of any machine: whether it means none is not decided here.
    for node in [0_u32, 1, 2, 7, 63, 255, 0x1234_5678, u32::MAX - 1, u32::MAX] {
        let numa = StorageDeviceNuma::decode(&reply(1, 12, node, 0)).expect("whole");
        assert_eq!(numa.node, node);
    }
}

#[test]
fn a_reply_longer_than_the_structure_is_read_for_the_fields_it_begins_with() {
    let numa = StorageDeviceNuma::decode(&reply(2, 20, 5, 8)).expect("a later version");

    assert_eq!((numa.version, numa.size, numa.node), (2, 20, 5));
}

#[test]
fn a_reply_shorter_than_the_structure_is_not_read() {
    let whole = reply(1, 12, 3, 0);

    for length in 0..DESCRIPTOR {
        let error = StorageDeviceNuma::decode(&whole[..length]).expect_err("cut short");
        assert_eq!(
            error.kind(),
            io::ErrorKind::InvalidData,
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
