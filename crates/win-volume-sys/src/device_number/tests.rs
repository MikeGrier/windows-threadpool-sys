// Copyright (c) 2026 Mike Grier
//! Tests for the device-number entry.

use std::mem::{offset_of, size_of};

use windows_sys::Win32::System::Ioctl::STORAGE_DEVICE_NUMBER;

use super::{StorageDeviceNumber, storage_device_number};
use crate::test_support::{
    open_disk, open_volume, ordinary_file, system_disk_numbers, system_volume_path,
    temp_volume_path,
};

/// A reply with the three fields set, as the device writes them.
fn reply(device_type: u32, device_number: u32, partition_number: u32) -> Vec<u8> {
    let mut bytes = vec![0u8; size_of::<STORAGE_DEVICE_NUMBER>()];
    for (offset, value) in [
        (offset_of!(STORAGE_DEVICE_NUMBER, DeviceType), device_type),
        (
            offset_of!(STORAGE_DEVICE_NUMBER, DeviceNumber),
            device_number,
        ),
        (
            offset_of!(STORAGE_DEVICE_NUMBER, PartitionNumber),
            partition_number,
        ),
    ] {
        bytes[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
    }
    bytes
}

#[test]
fn each_field_is_read_from_its_own_place() {
    for (device_type, device_number, partition_number) in [
        (7, 0, 1),
        (7, 3, 4),
        (0x2d, 12, 0),
        (1, 2, 3),
        (0x1111_1111, 0x2222_2222, 0x3333_3333),
    ] {
        let number =
            StorageDeviceNumber::decode(&reply(device_type, device_number, partition_number))
                .expect("a number");
        assert_eq!(
            number,
            StorageDeviceNumber {
                device_type,
                device_number,
                partition_number
            }
        );
    }
}

#[test]
fn a_value_that_means_none_is_carried_as_the_device_wrote_it() {
    let number = StorageDeviceNumber::decode(&reply(7, u32::MAX, u32::MAX)).expect("a number");
    assert_eq!(number.device_number, u32::MAX);
    assert_eq!(number.partition_number, u32::MAX);
}

#[test]
fn a_reply_longer_than_the_structure_is_read_for_the_fields_it_begins_with() {
    let mut longer = reply(7, 1, 2);
    longer.extend_from_slice(&[0xff; 16]);
    let number = StorageDeviceNumber::decode(&longer).expect("a number");
    assert_eq!(
        (
            number.device_type,
            number.device_number,
            number.partition_number
        ),
        (7, 1, 2)
    );
}

#[test]
fn a_reply_too_short_for_the_structure_is_refused() {
    let whole = reply(7, 1, 2);
    for length in [0, 1, 4, 8, 11] {
        let error = StorageDeviceNumber::decode(&whole[..length]).expect_err("refused");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData, "{length}");
    }
    assert!(StorageDeviceNumber::decode(&whole).is_ok());
}

#[test]
fn a_real_volume_reports_a_disk_of_the_machine_and_a_partition() {
    for path in [system_volume_path(), temp_volume_path()] {
        let number = storage_device_number(&open_volume(&path, false)).expect("a device number");
        assert_ne!(number.partition_number, u32::MAX, "{path}: {number}");
        assert!(
            crate::volume_disk_extents(&open_volume(&path, false))
                .expect("extents")
                .iter()
                .any(|extent| extent.disk_number == number.device_number),
            "{path}: {number}"
        );
    }
}

#[test]
fn a_disk_reports_the_number_it_is_given_in_its_path() {
    let disk = system_disk_numbers()[0];
    let number = storage_device_number(&open_disk(disk, false)).expect("a device number");
    assert_eq!(number.device_number, disk);
}

#[test]
fn an_overlapped_handle_gives_the_same_answer() {
    let path = system_volume_path();
    assert_eq!(
        storage_device_number(&open_volume(&path, false)).expect("plain"),
        storage_device_number(&open_volume(&path, true)).expect("overlapped")
    );
}

#[test]
fn a_handle_that_is_not_a_volume_or_a_disk_is_refused_by_the_device() {
    let (file, path) = ordinary_file("devnum");
    let error = storage_device_number(&file).expect_err("a file has no device number");
    assert!(
        win_error_codes::Win32Error::from_io(&error).is_some(),
        "the device's own code: {error}"
    );
    drop(file);
    std::fs::remove_file(path).expect("remove");
}

#[test]
fn the_display_names_each_field() {
    let text = StorageDeviceNumber {
        device_type: 7,
        device_number: 2,
        partition_number: 3,
    }
    .to_string();
    assert_eq!(text, "device type 0x7, number 2, partition 3");
}
