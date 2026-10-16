// Copyright (c) 2026 Mike Grier
//! Tests for the `IOCTL_STORAGE_QUERY_PROPERTY` mechanics.
//!
//! The `ERROR_MORE_DATA` half is reachable on a machine of one disk: a buffer between the header
//! and the full reply is answered with the header written. So the loop is tested against the real
//! device as well as against a fake that answers as the real one was measured to.

use std::io;
use std::mem::{offset_of, size_of};

use win_error_codes::Win32Error;
use windows_sys::Win32::Foundation::{ERROR_BAD_LENGTH, ERROR_MORE_DATA};
use windows_sys::Win32::System::Ioctl::{
    IOCTL_STORAGE_QUERY_PROPERTY, PropertyExistsQuery, PropertyMaskQuery, PropertyStandardQuery,
    STORAGE_DESCRIPTOR_HEADER, STORAGE_PROPERTY_QUERY, StorageDeviceUniqueIdProperty,
};

use super::{
    FIRST_BYTES, HEADER, MAX_ATTEMPTS, query_exists, query_input, query_property, query_via,
};
use crate::device_control::{device_control, u32_at};
use crate::test_support::system_disk;

/// The code an error carries, which every error from a control has.
fn code_of(error: &io::Error) -> u32 {
    Win32Error::from_io(error)
        .unwrap_or_else(|| panic!("an error with no OS code: {error}"))
        .code()
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

    fn answer(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.asked.push(buffer.len());
        if buffer.len() < HEADER {
            return Err(Win32Error::from_code(ERROR_BAD_LENGTH).to_io_error());
        }
        if buffer.len() < self.held.len() {
            buffer[..HEADER].copy_from_slice(&self.held[..HEADER]);
            return if buffer.len() == HEADER {
                Ok(HEADER)
            } else {
                Err(Win32Error::from_code(ERROR_MORE_DATA).to_io_error())
            };
        }
        buffer[..self.held.len()].copy_from_slice(&self.held);
        Ok(self.held.len())
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

#[test]
fn a_property_id_is_not_taken_for_a_query_type() {
    // Two different values in the two fields, so a swap shows.
    let input = query_input(3, 1, &[]).expect("input");
    assert_eq!(
        u32_at(&input, offset_of!(STORAGE_PROPERTY_QUERY, PropertyId)),
        Some(3)
    );
    assert_eq!(
        u32_at(&input, offset_of!(STORAGE_PROPERTY_QUERY, QueryType)),
        Some(1)
    );
}

// --- The real device -------------------------------------------------------------------------

#[test]
fn every_first_buffer_size_gets_the_same_reply_from_a_real_disk() {
    // 8 bytes is answered with the header alone; 9 up to the reply's size, with ERROR_MORE_DATA;
    // the bigger ones with the reply. All of them end at the same bytes.
    let disk = system_disk(false);

    let input = unique_id_input();
    let whole =
        query_property(&disk, &input, FIRST_BYTES).expect("the reply at the usual first size");
    assert!(whole.len() >= HEADER);
    assert_eq!(
        u32_at(&whole, offset_of!(STORAGE_DESCRIPTOR_HEADER, Size)),
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
        let again = query_property(&disk, &input, first)
            .unwrap_or_else(|error| panic!("a first buffer of {first} bytes: {error}"));
        assert_eq!(again, whole, "a first buffer of {first} bytes");
    }
}

#[test]
fn a_buffer_smaller_than_the_header_is_a_bad_length() {
    // Measured on the machine at hand; the documentation names no code for it. It is also why the
    // loop never asks with less than the header.
    let disk = system_disk(false);
    let input = unique_id_input();

    for size in [0_usize, 1, HEADER - 1] {
        let error = device_control(
            &disk,
            IOCTL_STORAGE_QUERY_PROPERTY,
            &input,
            &mut vec![0_u8; size],
        )
        .expect_err("refused");
        assert_eq!(
            code_of(&error),
            ERROR_BAD_LENGTH,
            "an output of {size} bytes"
        );
    }
}

#[test]
fn a_query_shorter_than_its_structure_is_a_bad_length() {
    let disk = system_disk(false);
    let input = unique_id_input();

    for size in [0_usize, 4, input.len() - 1] {
        let error = device_control(
            &disk,
            IOCTL_STORAGE_QUERY_PROPERTY,
            &input[..size],
            &mut vec![0_u8; FIRST_BYTES],
        )
        .expect_err("refused");
        assert_eq!(
            code_of(&error),
            ERROR_BAD_LENGTH,
            "an input of {size} bytes"
        );
    }
}

#[test]
fn a_plain_disk_handle_and_an_overlapped_one_get_the_same_reply() {
    let plain = system_disk(false);
    let overlapped = system_disk(true);

    let input = unique_id_input();
    assert_eq!(
        query_property(&plain, &input, FIRST_BYTES).expect("plain"),
        query_property(&overlapped, &input, FIRST_BYTES).expect("overlapped")
    );
}

#[test]
fn an_exists_query_on_a_real_property_succeeds_whatever_the_output_size() {
    let disk = system_disk(false);
    let input =
        query_input(StorageDeviceUniqueIdProperty, PropertyExistsQuery, &[]).expect("input");

    query_exists(&disk, &input).expect("the property exists");
    // No output is given, and the device does not need any.
    assert_eq!(
        device_control(&disk, IOCTL_STORAGE_QUERY_PROPERTY, &input, &mut []).expect("no output"),
        0
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
fn a_size_that_keeps_growing_is_refused_after_the_bounded_attempts() {
    let mut asked = 0_usize;
    let mut size = FIRST_BYTES;

    let outcome = query_via(FIRST_BYTES, |buffer| {
        asked += 1;
        // Always more than the buffer holds, so no buffer is ever big enough.
        size += 100;
        buffer[..HEADER].copy_from_slice(&reply(size)[..HEADER]);
        Err(Win32Error::from_code(ERROR_MORE_DATA).to_io_error())
    });

    let error = outcome.expect_err("never big enough");
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("kept changing"), "{error}");
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
        Err(Win32Error::from_code(ERROR_BAD_LENGTH).to_io_error())
    });

    assert_eq!(code_of(&outcome.expect_err("refused")), ERROR_BAD_LENGTH);
    assert_eq!(asked, 1, "a refusal is not retried");
}

// --- What the loop does with a reply that is not what it says --------------------------------

fn malformed(outcome: io::Result<Vec<u8>>) -> bool {
    matches!(outcome, Err(error) if error.kind() == io::ErrorKind::InvalidData)
}

#[test]
fn a_call_that_says_it_wrote_more_than_the_buffer_held_is_not_read() {
    // The header is valid and names a size the claimed count would cover, so only the claim
    // itself can make this malformed.
    for claimed in [FIRST_BYTES + 1, usize::try_from(u32::MAX).expect("fits")] {
        let outcome = query_via(FIRST_BYTES, |buffer| {
            buffer[..HEADER].copy_from_slice(&reply(100)[..HEADER]);
            Ok(claimed)
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
                Ok(written)
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
            Ok(HEADER)
        });
        assert!(malformed(outcome), "size {size}");
    }
}

#[test]
fn a_reply_shorter_than_the_size_it_names_in_a_buffer_that_had_room_is_not_read() {
    // The device wrote 100 bytes into a buffer big enough for the 200 it says it has, so asking
    // again at 200 would get the same buffer and the same answer. A buffer of exactly 200 is the
    // edge of it.
    for first in [FIRST_BYTES, 200] {
        let mut asked = 0_usize;
        let outcome = query_via(first, |buffer| {
            asked += 1;
            buffer[..HEADER].copy_from_slice(&reply(200)[..HEADER]);
            Ok(100)
        });
        assert!(malformed(outcome), "a buffer of {first} bytes");
        assert_eq!(asked, 1, "a buffer of {first} bytes: not asked again");
    }
}

#[test]
fn a_reply_that_needs_more_room_but_names_a_size_already_given_is_not_retried_blindly() {
    // A buffer of exactly the size it names is the edge of it: it was room enough.
    for named in [100, FIRST_BYTES] {
        let mut asked = 0_usize;
        let outcome = query_via(FIRST_BYTES, |buffer| {
            asked += 1;
            buffer[..HEADER].copy_from_slice(&reply(named)[..HEADER]);
            Err(Win32Error::from_code(ERROR_MORE_DATA).to_io_error())
        });
        assert!(malformed(outcome), "named {named}");
        assert_eq!(asked, 1, "named {named}");
    }
}
