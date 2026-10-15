// Copyright (c) Mike Grier.

//! Tests for the `IOCTL_STORAGE_QUERY_PROPERTY` mechanics.
//!
//! Unlike the disk-extents entry, this one's `ERROR_MORE_DATA` half is reachable on a machine of
//! one disk: a buffer between the header and the full reply is answered with the header written.
//! So the loop is tested against the real device as well as against a fake that answers as the
//! real one was measured to.

use std::fs::{File, OpenOptions};
use std::mem::offset_of;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsHandle, AsRawHandle};

use windows_sys::Win32::Foundation::{ERROR_BAD_LENGTH, ERROR_MORE_DATA};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_OVERLAPPED, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
};
use windows_sys::Win32::System::Ioctl::{
    IOCTL_STORAGE_QUERY_PROPERTY, PropertyExistsQuery, PropertyMaskQuery, PropertyStandardQuery,
    STORAGE_PROPERTY_QUERY, StorageDeviceUniqueIdProperty,
};

use super::{
    FIRST_BYTES, HEADER, MAX_ATTEMPTS, StoragePropertyError, query_input, query_property, query_via,
};
use crate::CapturedHandle;
use crate::device_control::{device_control, u32_at};
use crate::handle::tests::handle_allocation;
use crate::outcome::Win32Error;
use crate::volume_extents::QueryVolumeDiskExtents;

/// The numbers of the disks the system volume is on, in the order the device reports them.
pub(crate) fn system_disk_numbers() -> Vec<u32> {
    let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".to_owned());
    let volume = OpenOptions::new()
        .access_mode(0)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .open(format!(r"\\.\{drive}"))
        .expect("open the system volume with no access");
    QueryVolumeDiskExtents::new(
        CapturedHandle::capture(volume.as_handle()).expect("capture the volume"),
    )
    .perform()
    .expect("the system volume's disks")
    .iter()
    .map(|extent| extent.disk_number())
    .collect()
}

/// Disk `number`, opened with no access, which an ordinary account can do.
pub(crate) fn open_disk(number: u32, overlapped: bool) -> File {
    OpenOptions::new()
        .access_mode(0)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(if overlapped { FILE_FLAG_OVERLAPPED } else { 0 })
        .open(format!(r"\\.\PhysicalDrive{number}"))
        .expect("open the disk with no access")
}

/// The first disk the system volume is on, opened with no access.
pub(crate) fn system_disk(overlapped: bool) -> File {
    open_disk(system_disk_numbers()[0], overlapped)
}

/// A reply of `size` bytes: the header (a version of 1 and the size), then a recognisable body.
fn reply(size: usize) -> Vec<u8> {
    let mut bytes: Vec<u8> = (0..size)
        .map(|index| u8::try_from(index % 251).expect("a u8"))
        .collect();
    bytes[..4].copy_from_slice(&1_u32.to_ne_bytes());
    bytes[4..8].copy_from_slice(&u32::try_from(size).expect("small").to_ne_bytes());
    bytes
}

/// A device holding `held`, which answers as the real one was measured to: a buffer smaller than
/// the header is `ERROR_BAD_LENGTH`, exactly the header gets the header alone, one between that and
/// the whole reply is `ERROR_MORE_DATA` with the header written, and otherwise the whole reply is
/// written. Records how big each buffer was.
struct Device {
    held: Vec<u8>,
    asked: Vec<usize>,
}

impl Device {
    fn new(held: Vec<u8>) -> Self {
        Self {
            held,
            asked: Vec::new(),
        }
    }

    fn answer(&mut self, buffer: &mut [u8]) -> Result<u32, Win32Error> {
        self.asked.push(buffer.len());
        if buffer.len() < HEADER {
            return Err(Win32Error::from_code(ERROR_BAD_LENGTH));
        }
        if buffer.len() < self.held.len() {
            buffer[..HEADER].copy_from_slice(&self.held[..HEADER]);
            return if buffer.len() == HEADER {
                Ok(u32::try_from(HEADER).expect("small"))
            } else {
                Err(Win32Error::from_code(ERROR_MORE_DATA))
            };
        }
        buffer[..self.held.len()].copy_from_slice(&self.held);
        Ok(u32::try_from(self.held.len()).expect("small"))
    }
}

// --- The query structure ---------------------------------------------------------------------

/// The standard query for the device unique identifier, which has no additional parameters.
fn unique_id_input() -> Vec<u8> {
    query_input(StorageDeviceUniqueIdProperty, PropertyStandardQuery, &[]).expect("a small input")
}

#[test]
fn the_query_names_the_property_and_asks_the_standard_question() {
    let input = unique_id_input();

    assert_eq!(input.len(), size_of::<STORAGE_PROPERTY_QUERY>());
    assert_eq!(
        u32_at(&input, offset_of!(STORAGE_PROPERTY_QUERY, PropertyId)),
        Some(u32::try_from(StorageDeviceUniqueIdProperty).expect("non-negative"))
    );
    assert_eq!(
        u32_at(&input, offset_of!(STORAGE_PROPERTY_QUERY, QueryType)),
        Some(u32::try_from(PropertyStandardQuery).expect("non-negative"))
    );
    assert!(
        input[offset_of!(STORAGE_PROPERTY_QUERY, AdditionalParameters)..]
            .iter()
            .all(|byte| *byte == 0),
        "no additional parameters are sent"
    );
    // Different properties, and different questions, are different queries.
    assert_ne!(
        input,
        query_input(
            StorageDeviceUniqueIdProperty + 1,
            PropertyStandardQuery,
            &[]
        )
        .expect("input")
    );
    assert_ne!(
        input,
        query_input(StorageDeviceUniqueIdProperty, PropertyExistsQuery, &[]).expect("input")
    );
}

#[test]
fn additional_parameters_follow_the_fixed_fields_and_the_input_is_never_shorter_than_the_structure()
{
    let additional_at = offset_of!(STORAGE_PROPERTY_QUERY, AdditionalParameters);
    let structure = size_of::<STORAGE_PROPERTY_QUERY>();

    for extra in [
        0_usize,
        1,
        4,
        structure - additional_at,
        structure - additional_at + 1,
        16,
        100,
    ] {
        let additional: Vec<u8> = (1..=extra)
            .map(|byte| u8::try_from(byte % 255 + 1).expect("a u8"))
            .collect();

        let input = query_input(
            StorageDeviceUniqueIdProperty,
            PropertyStandardQuery,
            &additional,
        )
        .expect("input");

        assert_eq!(
            input.len(),
            structure.max(additional_at + extra),
            "{extra} extra bytes"
        );
        assert_eq!(
            &input[additional_at..additional_at + extra],
            additional.as_slice(),
            "{extra} extra bytes"
        );
        assert!(
            input[additional_at + extra..].iter().all(|byte| *byte == 0),
            "nothing but zeros after them: {extra} extra bytes"
        );
        // The fixed fields are where they were.
        assert_eq!(
            u32_at(&input, offset_of!(STORAGE_PROPERTY_QUERY, PropertyId)),
            Some(u32::try_from(StorageDeviceUniqueIdProperty).expect("non-negative"))
        );
    }
}

#[test]
fn the_query_type_is_written_where_the_structure_has_it() {
    for query_type in [
        PropertyStandardQuery,
        PropertyExistsQuery,
        PropertyMaskQuery,
        77,
    ] {
        let input = query_input(StorageDeviceUniqueIdProperty, query_type, &[]).expect("input");
        assert_eq!(
            u32_at(&input, offset_of!(STORAGE_PROPERTY_QUERY, QueryType)),
            Some(u32::try_from(query_type).expect("non-negative")),
            "query type {query_type}"
        );
    }
}

// --- The real device -------------------------------------------------------------------------

#[test]
fn every_first_buffer_size_gets_the_same_reply_from_a_real_disk() {
    // 8 bytes is answered with the header alone; 9 up to the reply's size, with ERROR_MORE_DATA;
    // the bigger ones with the reply. All of them end at the same bytes.
    let _allocating = handle_allocation().read().expect("not poisoned");
    let disk = system_disk(false);

    let input = unique_id_input();
    let whole = query_property(disk.as_raw_handle(), &input, FIRST_BYTES)
        .expect("the reply at the usual first size");
    assert!(whole.len() >= HEADER);
    assert_eq!(
        u32_at(
            &whole,
            offset_of!(
                windows_sys::Win32::System::Ioctl::STORAGE_DESCRIPTOR_HEADER,
                Size
            )
        ),
        Some(u32::try_from(whole.len()).expect("small")),
        "exactly the bytes the header says"
    );

    for first in [
        0,
        1,
        HEADER,
        HEADER + 1,
        whole.len() - 1,
        whole.len(),
        whole.len() + 1,
        4096,
    ] {
        let again = query_property(disk.as_raw_handle(), &input, first)
            .unwrap_or_else(|error| panic!("a first buffer of {first} bytes: {error}"));
        assert_eq!(again, whole, "a first buffer of {first} bytes");
    }
}

#[test]
fn a_buffer_smaller_than_the_header_is_a_bad_length() {
    // Measured on the machine at hand; the documentation names no code for it. It is also why the
    // entry never asks with less than the header.
    let _allocating = handle_allocation().read().expect("not poisoned");
    let disk = system_disk(false);
    let input = unique_id_input();

    for size in [0_usize, 1, HEADER - 1] {
        let outcome = device_control(
            disk.as_raw_handle(),
            IOCTL_STORAGE_QUERY_PROPERTY,
            &input,
            &mut vec![0_u8; size],
        )
        .map_err(|error| error.code());
        assert_eq!(outcome, Err(ERROR_BAD_LENGTH), "an output of {size} bytes");
    }
}

#[test]
fn a_query_shorter_than_its_structure_is_a_bad_length() {
    let _allocating = handle_allocation().read().expect("not poisoned");
    let disk = system_disk(false);
    let input = unique_id_input();

    for size in [0_usize, 4, input.len() - 1] {
        let outcome = device_control(
            disk.as_raw_handle(),
            IOCTL_STORAGE_QUERY_PROPERTY,
            &input[..size],
            &mut vec![0_u8; FIRST_BYTES],
        )
        .map_err(|error| error.code());
        assert_eq!(outcome, Err(ERROR_BAD_LENGTH), "an input of {size} bytes");
    }
}

#[test]
fn a_plain_disk_handle_and_an_overlapped_one_get_the_same_reply() {
    let _allocating = handle_allocation().read().expect("not poisoned");
    let plain = system_disk(false);
    let overlapped = system_disk(true);

    let input = unique_id_input();
    assert_eq!(
        query_property(plain.as_raw_handle(), &input, FIRST_BYTES).expect("plain"),
        query_property(overlapped.as_raw_handle(), &input, FIRST_BYTES).expect("overlapped")
    );
}

// --- A device whose reply is larger than the first buffer ------------------------------------

#[test]
fn a_reply_that_fits_the_first_buffer_costs_one_call_and_is_exactly_its_size() {
    let held = reply(112);
    let mut device = Device::new(held.clone());

    let got = query_via(FIRST_BYTES, |buffer| device.answer(buffer)).expect("answer");

    assert_eq!(
        got, held,
        "the zero padding of the larger buffer is not part of the reply"
    );
    assert_eq!(device.asked, vec![FIRST_BYTES]);
}

#[test]
fn a_reply_that_exactly_fills_the_first_buffer_is_accepted() {
    let held = reply(FIRST_BYTES);
    let mut device = Device::new(held.clone());

    assert_eq!(
        query_via(FIRST_BYTES, |buffer| device.answer(buffer)).expect("answer"),
        held
    );
    assert_eq!(device.asked, vec![FIRST_BYTES]);
}

#[test]
fn a_reply_larger_than_the_first_buffer_is_asked_again_at_the_size_it_names() {
    for size in [FIRST_BYTES + 1, FIRST_BYTES + 88, 4000] {
        let held = reply(size);
        let mut device = Device::new(held.clone());

        let got = query_via(FIRST_BYTES, |buffer| device.answer(buffer)).expect("answer");

        assert_eq!(got, held, "a reply of {size} bytes");
        assert_eq!(
            device.asked,
            vec![FIRST_BYTES, size],
            "a reply of {size} bytes"
        );
    }
}

#[test]
fn a_first_buffer_of_exactly_the_header_learns_the_size_from_the_header_alone() {
    let held = reply(300);
    let mut device = Device::new(held.clone());

    let got = query_via(HEADER, |buffer| device.answer(buffer)).expect("answer");

    assert_eq!(got, held);
    assert_eq!(device.asked, vec![HEADER, 300]);
}

#[test]
fn a_first_buffer_of_no_room_is_given_room_for_the_header() {
    let held = reply(300);
    let mut device = Device::new(held.clone());

    let got = query_via(0, |buffer| device.answer(buffer)).expect("answer");

    assert_eq!(got, held);
    assert_eq!(
        device.asked[0], HEADER,
        "a buffer smaller than the header is a bad length"
    );
}

#[test]
fn a_size_that_keeps_growing_is_unstable_after_the_bounded_attempts() {
    let mut asked = 0_usize;
    let mut size = FIRST_BYTES;

    let outcome = query_via(FIRST_BYTES, |buffer| {
        asked += 1;
        // Always more than the buffer holds, so no buffer is ever big enough.
        size += 100;
        buffer[..HEADER].copy_from_slice(&reply(size)[..HEADER]);
        Err(Win32Error::from_code(ERROR_MORE_DATA))
    });

    assert!(
        matches!(outcome, Err(StoragePropertyError::Unstable { attempts }) if attempts == MAX_ATTEMPTS),
        "{outcome:?}"
    );
    assert_eq!(
        asked, MAX_ATTEMPTS,
        "the retry is bounded, and uses its bound"
    );
}

#[test]
fn any_other_failure_is_the_devices_own_code_and_ends_the_attempts() {
    let mut asked = 0_usize;

    let outcome = query_via(FIRST_BYTES, |_| {
        asked += 1;
        Err(Win32Error::from_code(ERROR_BAD_LENGTH))
    });

    match outcome {
        Err(StoragePropertyError::Win32(error)) => assert_eq!(error.code(), ERROR_BAD_LENGTH),
        other => panic!("expected the device's own code, got {other:?}"),
    }
    assert_eq!(asked, 1, "a refusal is not retried");
}

// --- What the loop does with a reply that is not what it says --------------------------------

fn malformed(outcome: Result<Vec<u8>, StoragePropertyError>) -> bool {
    matches!(outcome, Err(StoragePropertyError::Malformed { .. }))
}

#[test]
fn a_call_that_says_it_wrote_more_than_the_buffer_held_is_not_read() {
    // The header is valid and names a size the claimed count would cover, so only the claim
    // itself can make this malformed.
    for claimed in [FIRST_BYTES + 1, usize::try_from(u32::MAX).expect("fits")] {
        let outcome = query_via(FIRST_BYTES, |buffer| {
            buffer[..HEADER].copy_from_slice(&reply(100)[..HEADER]);
            Ok(u32::try_from(claimed).expect("fits"))
        });
        assert!(malformed(outcome), "claimed {claimed}");
    }
}

#[test]
fn a_reply_shorter_than_its_header_is_not_read() {
    // With a first buffer of exactly the header, a reply of fewer bytes that named a bigger size
    // would otherwise be asked for again, so the number of calls is what shows it was refused.
    for first in [HEADER, FIRST_BYTES] {
        for written in 0..HEADER {
            let mut asked = 0_usize;
            let outcome = query_via(first, |buffer| {
                asked += 1;
                buffer[..HEADER].copy_from_slice(&reply(600)[..HEADER]);
                Ok(u32::try_from(written).expect("small"))
            });
            assert!(malformed(outcome), "first {first}, written {written}");
            assert_eq!(
                asked, 1,
                "first {first}, written {written}: not asked again"
            );
        }
    }
}

#[test]
fn a_reply_that_says_it_is_smaller_than_its_own_header_is_not_read() {
    for size in 0..HEADER {
        let outcome = query_via(FIRST_BYTES, |buffer| {
            buffer[..4].copy_from_slice(&1_u32.to_ne_bytes());
            buffer[4..8].copy_from_slice(&u32::try_from(size).expect("small").to_ne_bytes());
            Ok(u32::try_from(HEADER).expect("small"))
        });
        assert!(malformed(outcome), "size {size}");
    }
}

#[test]
fn a_reply_shorter_than_the_size_it_names_in_a_buffer_that_had_room_is_not_read() {
    // The device wrote 100 bytes into a buffer big enough for the 200 it says it has, so asking
    // again at 200 would get the same buffer and the same answer.
    let outcome = query_via(FIRST_BYTES, |buffer| {
        buffer[..HEADER].copy_from_slice(&reply(200)[..HEADER]);
        Ok(100)
    });
    assert!(malformed(outcome));
}

#[test]
fn a_reply_that_needs_more_room_but_names_a_size_already_given_is_not_retried_blindly() {
    let mut asked = 0_usize;
    let outcome = query_via(FIRST_BYTES, |buffer| {
        asked += 1;
        buffer[..HEADER].copy_from_slice(&reply(100)[..HEADER]);
        Err(Win32Error::from_code(ERROR_MORE_DATA))
    });
    assert!(malformed(outcome));
    assert_eq!(asked, 1);
}

// --- Errors ----------------------------------------------------------------------------------

#[test]
fn errors_name_the_call_and_keep_the_raw_code() {
    let refused = StoragePropertyError::from(Win32Error::from_code(1));
    assert!(
        refused
            .to_string()
            .starts_with("IOCTL_STORAGE_QUERY_PROPERTY: ")
    );
    let source = std::error::Error::source(&refused).expect("a Win32 source");
    assert_eq!(
        source
            .downcast_ref::<Win32Error>()
            .map(|error| error.code()),
        Some(1)
    );

    let bad = StoragePropertyError::Malformed {
        what: "a reply that lies",
    };
    assert!(bad.to_string().contains("a reply that lies"));
    assert!(std::error::Error::source(&bad).is_none());

    let unstable = StoragePropertyError::Unstable { attempts: 4 };
    assert!(unstable.to_string().contains('4'));
    assert!(std::error::Error::source(&unstable).is_none());
}
