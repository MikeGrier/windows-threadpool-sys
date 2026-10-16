// Copyright (c) 2026 Mike Grier
//! Tests for the generic storage property query.
//!
//! The real disk shows what a device does with each kind of query and with a property it has not
//! got, and that additional parameters reach it: the unique-id property reads the first, so its
//! answer depends on them.

use std::io;

use win_error_codes::Win32Error;
use windows_sys::Win32::Foundation::ERROR_INVALID_FUNCTION;
use windows_sys::Win32::System::Ioctl::{
    PropertyExistsQuery, PropertyMaskQuery, PropertyStandardQuery, StorageDeviceProperty,
    StorageDeviceUniqueIdProperty,
};

use super::{StoragePropertyId, StorageQueryType, query_storage_property};
use crate::storage_unique_id::storage_device_unique_id;
use crate::test_support::{ordinary_file, system_disk};

fn property(raw: i32) -> StoragePropertyId {
    StoragePropertyId::from_raw(raw)
}

/// A property no device has.
const NO_SUCH_PROPERTY: i32 = 9999;

/// The code an error carries, which every error from a control has.
fn code_of(error: &io::Error) -> u32 {
    Win32Error::from_io(error)
        .unwrap_or_else(|| panic!("an error with no OS code: {error}"))
        .code()
}

/// The first four bytes of a reply are its header's version and the next four its size.
fn header_size(reply: &[u8]) -> usize {
    usize::try_from(u32::from_ne_bytes(reply[4..8].try_into().expect("four"))).expect("fits")
}

// --- The values ------------------------------------------------------------------------------

#[test]
fn the_named_query_types_are_the_ones_windows_defines() {
    assert_eq!(StorageQueryType::STANDARD.raw(), PropertyStandardQuery);
    assert_eq!(StorageQueryType::EXISTS.raw(), PropertyExistsQuery);
    assert_eq!(StorageQueryType::MASK.raw(), PropertyMaskQuery);
    // They are three different questions, and a raw value round-trips, including one Windows does
    // not define.
    assert_ne!(StorageQueryType::STANDARD, StorageQueryType::EXISTS);
    assert_ne!(StorageQueryType::EXISTS, StorageQueryType::MASK);
    assert_ne!(StorageQueryType::STANDARD, StorageQueryType::MASK);
    assert_eq!(StorageQueryType::from_raw(77).raw(), 77);
    assert_eq!(
        StorageQueryType::from_raw(PropertyExistsQuery),
        StorageQueryType::EXISTS
    );
}

#[test]
fn a_property_id_round_trips_its_raw_value() {
    for raw in [0, 3, i32::MAX, NO_SUCH_PROPERTY, -1] {
        assert_eq!(property(raw).raw(), raw);
    }
    assert_ne!(
        property(StorageDeviceProperty),
        property(StorageDeviceUniqueIdProperty)
    );
}

// --- The real disk ---------------------------------------------------------------------------

#[test]
fn a_standard_query_returns_exactly_the_bytes_its_header_says() {
    let disk = system_disk(false);

    let reply = query_storage_property(
        &disk,
        property(StorageDeviceProperty),
        StorageQueryType::STANDARD,
        &[],
    )
    .expect("the device property");

    assert!(!reply.is_empty());
    assert_eq!(header_size(&reply), reply.len());
}

#[test]
fn the_generic_function_returns_what_the_typed_one_does_for_the_same_property() {
    let disk = system_disk(false);

    let generic = query_storage_property(
        &disk,
        property(StorageDeviceUniqueIdProperty),
        StorageQueryType::STANDARD,
        &[],
    )
    .expect("the generic query");
    let typed = storage_device_unique_id(&disk).expect("the typed query");

    assert_eq!(generic, typed.as_bytes());
}

#[test]
fn a_query_for_whether_a_property_exists_has_no_structure_and_success_means_it_does() {
    let disk = system_disk(false);

    for raw in [StorageDeviceProperty, StorageDeviceUniqueIdProperty] {
        let reply = query_storage_property(&disk, property(raw), StorageQueryType::EXISTS, &[])
            .unwrap_or_else(|error| panic!("property {raw} exists: {error}"));

        assert!(reply.is_empty(), "property {raw}");
    }
}

#[test]
fn a_property_the_device_does_not_have_is_the_devices_own_error_for_both_questions() {
    let disk = system_disk(false);

    for query_type in [StorageQueryType::STANDARD, StorageQueryType::EXISTS] {
        let error = query_storage_property(&disk, property(NO_SUCH_PROPERTY), query_type, &[])
            .expect_err("an unknown property");

        // Measured on the machine at hand; the documentation names no code for it.
        assert_eq!(code_of(&error), ERROR_INVALID_FUNCTION, "{query_type:?}");
    }
}

#[test]
fn a_query_type_the_property_does_not_take_is_the_devices_own_error_and_not_read() {
    let disk = system_disk(false);

    for query_type in [StorageQueryType::MASK, StorageQueryType::from_raw(77)] {
        let error = query_storage_property(
            &disk,
            property(StorageDeviceUniqueIdProperty),
            query_type,
            &[],
        )
        .expect_err("refused");

        assert_ne!(
            error.kind(),
            io::ErrorKind::InvalidData,
            "{query_type:?}: {error}"
        );
        eprintln!("{query_type:?}: {error}");
    }
}

#[test]
fn a_property_that_reads_no_additional_parameters_gives_the_same_reply_whatever_they_hold() {
    // The device property was measured to take none.
    let disk = system_disk(false);
    let ask = |additional: &[u8]| {
        query_storage_property(
            &disk,
            property(StorageDeviceProperty),
            StorageQueryType::STANDARD,
            additional,
        )
        .expect("query")
    };
    let bare = ask(&[]);

    for extra in [1_usize, 4, 16, 200] {
        assert_eq!(ask(&vec![0xA5_u8; extra]), bare, "{extra} additional bytes");
    }
}

#[test]
fn additional_parameters_reach_the_device_and_what_it_does_with_them_is_its_own() {
    // The device unique identifier property reads the first additional byte: on the machine
    // measured, 0 gave the usual reply, 1 a larger one, and anything larger was refused as
    // `ERROR_INVALID_PARAMETER`. What the byte selects is not documented and is not interpreted
    // here; what is shown is that the bytes arrive, because the answer depends on them.
    let disk = system_disk(false);
    let ask = |additional: &[u8]| {
        query_storage_property(
            &disk,
            property(StorageDeviceUniqueIdProperty),
            StorageQueryType::STANDARD,
            additional,
        )
    };
    let bare = ask(&[]).expect("bare");

    assert_eq!(
        ask(&[0]).expect("a zero byte"),
        bare,
        "zero is what no parameters mean"
    );

    let refused = ask(&[0xA5]).expect_err("an additional byte of 0xA5 is refused");
    assert_ne!(refused.kind(), io::ErrorKind::InvalidData, "{refused}");
    eprintln!("an additional byte of 0xA5: {refused}");

    match ask(&[1]) {
        Ok(one) if one != bare => {
            // The reply is still a whole structure, read to the size its own header names.
            assert_eq!(header_size(&one), one.len());
        }
        Ok(_) => eprintln!("skipped: this device gives the same reply for an additional byte of 1"),
        Err(error) => eprintln!("skipped: this device refuses an additional byte of 1: {error}"),
    }
}

#[test]
fn a_plain_disk_handle_and_an_overlapped_one_get_the_same_reply() {
    let plain = system_disk(false);
    let overlapped = system_disk(true);

    let ask = |disk: &std::fs::File| {
        query_storage_property(
            disk,
            property(StorageDeviceProperty),
            StorageQueryType::STANDARD,
            &[],
        )
        .expect("query")
    };

    assert_eq!(ask(&plain), ask(&overlapped));
}

#[test]
fn a_handle_that_is_not_a_disk_is_the_devices_own_error() {
    let (file, path) = ordinary_file("sq-file");

    let error = query_storage_property(
        &file,
        property(StorageDeviceProperty),
        StorageQueryType::STANDARD,
        &[],
    )
    .expect_err("an ordinary file is not a disk");
    assert_ne!(error.kind(), io::ErrorKind::InvalidData, "{error}");
    assert!(error.raw_os_error().is_some(), "{error}");
    drop(file);
    std::fs::remove_file(path).expect("remove");
}
