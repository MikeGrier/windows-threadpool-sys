// Copyright (c) 2026 Mike Grier
//! Tests for the device descriptor.
//!
//! What a real disk reports depends on the machine, so the real-disk tests check shapes and
//! consistency; what the function does with a reply is tested against replies built here, in both
//! directions: what it reads, and what it refuses.

use std::io;

use windows_sys::Win32::System::Ioctl::{
    PropertyStandardQuery, STORAGE_DEVICE_DESCRIPTOR, StorageDeviceProperty,
};

use super::{StorageBusType, StorageDeviceDescriptor, layout, storage_device_descriptor};
use crate::storage_property::{FIRST_BYTES, HEADER, query_input, query_property};
use crate::test_support::{open_disk, ordinary_file, system_disk, system_disk_numbers};

/// A reply: the fixed fields, then `raw` bus-specific bytes, then each present string with its
/// terminator, its offset written into the field `layout` names.
struct Reply {
    bus_type: i32,
    raw: Vec<u8>,
    strings: [Option<&'static [u8]>; 4],
}

impl Reply {
    fn plain() -> Self {
        Self {
            bus_type: StorageBusType::NVME.raw(),
            raw: Vec::new(),
            strings: [None, None, None, None],
        }
    }

    fn bytes(&self) -> Vec<u8> {
        let mut bytes = vec![0_u8; layout::RAW_PROPERTIES];
        put(&mut bytes, layout::VERSION, 40);
        bytes[layout::COMMAND_QUEUEING] = 1;
        put(&mut bytes, layout::BUS_TYPE, self.bus_type.cast_unsigned());
        put(
            &mut bytes,
            layout::RAW_PROPERTIES_LENGTH,
            u32::try_from(self.raw.len()).expect("small"),
        );
        bytes.extend_from_slice(&self.raw);
        let fields = [
            layout::VENDOR_ID_OFFSET,
            layout::PRODUCT_ID_OFFSET,
            layout::PRODUCT_REVISION_OFFSET,
            layout::SERIAL_NUMBER_OFFSET,
        ];
        for (field, string) in fields.into_iter().zip(self.strings) {
            if let Some(string) = string {
                let at = u32::try_from(bytes.len()).expect("small");
                put(&mut bytes, field, at);
                bytes.extend_from_slice(string);
                bytes.push(0);
            }
        }
        let size = u32::try_from(bytes.len()).expect("small");
        put(&mut bytes, layout::SIZE, size);
        bytes
    }
}

fn put(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_ne_bytes());
}

// --- The real disk --------------------------------------------------------------------------

#[test]
fn a_real_disk_reports_a_descriptor_at_least_as_large_as_its_fields() {
    let disk = system_disk(false);

    let descriptor = storage_device_descriptor(&disk).expect("query the disk");

    assert!(
        usize::try_from(descriptor.size).expect("fits") >= layout::RAW_PROPERTIES,
        "{descriptor:?}"
    );
    // The page: `Version` "Contains the size of this structure, in bytes." Another property's
    // descriptor would read without error, and this is what tells it apart.
    assert_eq!(
        usize::try_from(descriptor.version).expect("fits"),
        size_of::<STORAGE_DEVICE_DESCRIPTOR>(),
        "{descriptor:?}"
    );
    eprintln!(
        "the system disk: bus {}, type {:#x}, vendor {:?}, product {:?}",
        descriptor.bus_type,
        descriptor.device_type,
        descriptor.vendor_id.as_deref().map(String::from_utf8_lossy),
        descriptor
            .product_id
            .as_deref()
            .map(String::from_utf8_lossy),
    );
}

#[test]
fn every_disk_a_volume_is_on_answers_for_itself() {
    let numbers = system_disk_numbers();
    assert!(!numbers.is_empty());
    for number in numbers {
        let disk = open_disk(number, false);
        let descriptor = storage_device_descriptor(&disk)
            .unwrap_or_else(|error| panic!("disk {number}: {error}"));
        eprintln!("disk {number}: bus {}", descriptor.bus_type);
    }
}

#[test]
fn every_first_buffer_size_gets_the_same_reply_from_a_real_disk() {
    let disk = system_disk(false);
    let input =
        query_input(StorageDeviceProperty, PropertyStandardQuery, &[]).expect("a small input");

    let whole = query_property(&disk, &input, FIRST_BYTES).expect("the usual first size");
    for first in [
        0,
        1,
        HEADER - 1,
        HEADER,
        HEADER + 1,
        layout::RAW_PROPERTIES,
        4096,
    ] {
        let again = query_property(&disk, &input, first)
            .unwrap_or_else(|error| panic!("a first buffer of {first} bytes: {error}"));
        assert_eq!(again, whole, "a first buffer of {first} bytes");
    }
}

#[test]
fn a_plain_disk_handle_and_an_overlapped_one_get_the_same_answer() {
    assert_eq!(
        storage_device_descriptor(&system_disk(false)).expect("plain"),
        storage_device_descriptor(&system_disk(true)).expect("overlapped")
    );
}

#[test]
fn asking_twice_gives_the_same_answer() {
    let disk = system_disk(false);

    assert_eq!(
        storage_device_descriptor(&disk).expect("first"),
        storage_device_descriptor(&disk).expect("second")
    );
}

#[test]
fn a_handle_that_is_not_a_disk_is_the_devices_own_error() {
    let (file, path) = ordinary_file("sd-file");

    let error = storage_device_descriptor(&file).expect_err("an ordinary file is not a disk");
    assert_ne!(error.kind(), io::ErrorKind::InvalidData, "{error}");
    assert!(error.raw_os_error().is_some(), "{error}");
    drop(file);
    std::fs::remove_file(path).expect("remove");
}

// --- What the function reads ------------------------------------------------------------------

#[test]
fn each_fixed_field_is_read_from_where_the_structure_has_it() {
    let mut bytes = Reply::plain().bytes();
    bytes[layout::DEVICE_TYPE] = 0x1f;
    bytes[layout::DEVICE_TYPE_MODIFIER] = 0x22;
    bytes[layout::REMOVABLE_MEDIA] = 1;
    bytes[layout::COMMAND_QUEUEING] = 0;
    put(&mut bytes, layout::VERSION, 0x1111_1111);

    let descriptor = StorageDeviceDescriptor::decode(&bytes).expect("whole");

    assert_eq!(descriptor.version, 0x1111_1111);
    assert_eq!(
        usize::try_from(descriptor.size).expect("fits"),
        layout::RAW_PROPERTIES
    );
    assert_eq!(descriptor.device_type, 0x1f);
    assert_eq!(descriptor.device_type_modifier, 0x22);
    assert!(descriptor.removable_media);
    assert!(!descriptor.command_queueing);
    assert_eq!(descriptor.bus_type, StorageBusType::NVME);
}

#[test]
fn every_bus_type_is_carried_as_the_device_wrote_it() {
    for raw in [0, 1, 7, 15, 16, 17, 19, 20, 21, 0x7f, 0x80, -1, i32::MAX] {
        let reply = Reply {
            bus_type: raw,
            ..Reply::plain()
        };
        let descriptor = StorageDeviceDescriptor::decode(&reply.bytes()).expect("whole");
        assert_eq!(descriptor.bus_type.raw(), raw);
    }
}

#[test]
fn the_documented_bus_types_are_named_and_others_are_not() {
    assert_eq!(StorageBusType::FILE_BACKED_VIRTUAL.raw(), 15);
    assert_eq!(StorageBusType::SPACES.raw(), 16);
    assert_eq!(StorageBusType::NVMEOF.raw(), 20);
    assert_eq!(
        StorageBusType::FILE_BACKED_VIRTUAL.name(),
        Some("BusTypeFileBackedVirtual")
    );
    assert_eq!(StorageBusType::NVMEOF.name(), Some("BusTypeNvmeof"));
    for raw in 0..=20 {
        assert!(
            StorageBusType::from_raw(raw).name().is_some(),
            "{raw} is on the page"
        );
    }
    for raw in [21, 0x7f, -1] {
        assert_eq!(StorageBusType::from_raw(raw).name(), None, "{raw}");
        assert_eq!(
            StorageBusType::from_raw(raw).to_string(),
            format!("StorageBusType({raw})")
        );
    }
    assert_eq!(StorageBusType::SATA.to_string(), "BusTypeSata");
}

#[test]
fn present_strings_are_read_without_their_terminators_and_absent_ones_are_none() {
    let reply = Reply {
        strings: [Some(b"VENDOR"), None, Some(b"1.0"), Some(b"SN 42")],
        ..Reply::plain()
    };

    let descriptor = StorageDeviceDescriptor::decode(&reply.bytes()).expect("whole");

    assert_eq!(descriptor.vendor_id.as_deref(), Some(&b"VENDOR"[..]));
    assert_eq!(descriptor.product_id, None);
    assert_eq!(descriptor.product_revision.as_deref(), Some(&b"1.0"[..]));
    assert_eq!(descriptor.serial_number.as_deref(), Some(&b"SN 42"[..]));
}

#[test]
fn a_string_is_carried_as_bytes_not_checked_to_be_ascii() {
    let reply = Reply {
        strings: [None, Some(b"\xff\x80odd"), None, Some(b"")],
        ..Reply::plain()
    };

    let descriptor = StorageDeviceDescriptor::decode(&reply.bytes()).expect("whole");

    assert_eq!(descriptor.product_id.as_deref(), Some(&b"\xff\x80odd"[..]));
    assert_eq!(
        descriptor.serial_number.as_deref(),
        Some(&b""[..]),
        "an empty string is present, not absent"
    );
}

#[test]
fn the_bus_specific_bytes_are_carried() {
    let reply = Reply {
        raw: vec![1, 2, 3, 4, 5],
        strings: [Some(b"V"), None, None, None],
        ..Reply::plain()
    };

    let descriptor = StorageDeviceDescriptor::decode(&reply.bytes()).expect("whole");

    assert_eq!(descriptor.raw_properties, vec![1, 2, 3, 4, 5]);
    assert_eq!(descriptor.vendor_id.as_deref(), Some(&b"V"[..]));
}

// --- What the function refuses ----------------------------------------------------------------

#[test]
fn a_reply_shorter_than_the_fixed_fields_is_not_read() {
    let whole = Reply::plain().bytes();

    for length in 0..layout::RAW_PROPERTIES {
        let error = StorageDeviceDescriptor::decode(&whole[..length]).expect_err("cut short");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData, "{length} bytes");
    }
}

#[test]
fn a_string_outside_the_reply_or_inside_the_fields_or_unterminated_is_refused() {
    let base = Reply {
        strings: [Some(b"VENDOR"), None, None, None],
        ..Reply::plain()
    }
    .bytes();
    let length = u32::try_from(base.len()).expect("small");
    let inside = u32::try_from(layout::RAW_PROPERTIES - 1).expect("small");

    for (offset, what) in [
        (length, "at the end"),
        (length + 10, "past the end"),
        (u32::MAX, "far past the end"),
        (inside, "inside the fields"),
        (4, "at the size field"),
    ] {
        let mut bytes = base.clone();
        put(&mut bytes, layout::VENDOR_ID_OFFSET, offset);
        let error = StorageDeviceDescriptor::decode(&bytes).expect_err(what);
        assert_eq!(error.kind(), io::ErrorKind::InvalidData, "{what}");
    }

    let mut unterminated = base.clone();
    unterminated.pop();
    let error = StorageDeviceDescriptor::decode(&unterminated).expect_err("no terminator");
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
}

#[test]
fn bus_specific_bytes_that_run_past_the_reply_are_refused() {
    let mut bytes = Reply {
        raw: vec![9; 4],
        ..Reply::plain()
    }
    .bytes();
    put(&mut bytes, layout::RAW_PROPERTIES_LENGTH, 5);
    let error = StorageDeviceDescriptor::decode(&bytes).expect_err("one byte too many");
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);

    put(&mut bytes, layout::RAW_PROPERTIES_LENGTH, u32::MAX);
    let error = StorageDeviceDescriptor::decode(&bytes).expect_err("far too many");
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);

    put(&mut bytes, layout::RAW_PROPERTIES_LENGTH, 4);
    assert_eq!(
        StorageDeviceDescriptor::decode(&bytes)
            .expect("exactly")
            .raw_properties,
        vec![9; 4]
    );
}
