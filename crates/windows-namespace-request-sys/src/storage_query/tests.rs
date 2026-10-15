// Copyright (c) Mike Grier.

//! Tests for the generic storage property entry.
//!
//! The real disk shows what a device does with each kind of query and with a property it has not
//! got; what reaches the device is shown by the input the request builds, because a property that
//! takes additional parameters is not one every disk has, and one that ignores them cannot tell
//! whether they arrived.

use std::fs::File;
use std::mem::offset_of;
use std::os::windows::io::{AsHandle, AsRawHandle};
use std::sync::Arc;

use windows_sys::Win32::System::Ioctl::{
    PropertyExistsQuery, PropertyMaskQuery, PropertyStandardQuery, STORAGE_PROPERTY_QUERY,
    StorageDeviceProperty, StorageDeviceUniqueIdProperty,
};

use super::{QueryStorageProperty, StoragePropertyId, StoragePropertyReply, StorageQueryType};
use crate::device_control::u32_at;
use crate::handle::tests::{Fixture, handle_allocation};
use crate::request::Request;
use crate::storage_property::StoragePropertyError;
use crate::storage_property::tests::system_disk;
use crate::storage_unique_id::QueryStorageDeviceUniqueId;
use crate::{CapturedHandle, Win32Error};

fn handle_for(file: &File) -> CapturedHandle {
    CapturedHandle::capture(file.as_handle()).expect("capture")
}

fn property(raw: i32) -> StoragePropertyId {
    StoragePropertyId::from_raw(raw)
}

/// A property no device has.
const NO_SUCH_PROPERTY: i32 = 9999;

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

// --- What a request sends --------------------------------------------------------------------

#[test]
fn a_new_request_asks_the_standard_question_with_no_additional_parameters() {
    let _allocating = handle_allocation().read().expect("not poisoned");
    let disk = system_disk(false);

    let request = QueryStorageProperty::new(handle_for(&disk), property(StorageDeviceProperty));

    assert_eq!(request.property(), property(StorageDeviceProperty));
    assert_eq!(request.query_type(), StorageQueryType::STANDARD);
    assert!(request.additional_parameters().is_empty());
    assert_ne!(request.handle().raw(), disk.as_raw_handle());
}

#[test]
fn what_is_configured_reads_back_and_is_what_the_input_carries() {
    let _allocating = handle_allocation().read().expect("not poisoned");
    let disk = system_disk(false);
    let additional = vec![9_u8, 8, 7, 6, 5, 4, 3, 2, 1];

    let request = QueryStorageProperty::new(handle_for(&disk), property(NO_SUCH_PROPERTY))
        .with_query_type(StorageQueryType::MASK)
        .with_additional_parameters(additional.clone());

    assert_eq!(request.property(), property(NO_SUCH_PROPERTY));
    assert_eq!(request.query_type(), StorageQueryType::MASK);
    assert_eq!(request.additional_parameters(), additional.as_slice());

    let input = request.input().expect("the input");
    assert_eq!(
        u32_at(&input, offset_of!(STORAGE_PROPERTY_QUERY, PropertyId)),
        Some(u32::try_from(NO_SUCH_PROPERTY).expect("non-negative"))
    );
    assert_eq!(
        u32_at(&input, offset_of!(STORAGE_PROPERTY_QUERY, QueryType)),
        Some(u32::try_from(PropertyMaskQuery).expect("non-negative"))
    );
    let at = offset_of!(STORAGE_PROPERTY_QUERY, AdditionalParameters);
    assert_eq!(&input[at..at + additional.len()], additional.as_slice());
}

#[test]
fn additional_parameters_may_be_given_as_a_vec_a_slice_or_a_shared_buffer() {
    let _allocating = handle_allocation().read().expect("not poisoned");
    let disk = system_disk(false);
    let shared: Arc<[u8]> = Arc::from([1_u8, 2, 3]);

    let from_vec = QueryStorageProperty::new(handle_for(&disk), property(0))
        .with_additional_parameters(vec![1_u8, 2, 3]);
    let from_slice = QueryStorageProperty::new(handle_for(&disk), property(0))
        .with_additional_parameters(&[1_u8, 2, 3][..]);
    let from_arc = QueryStorageProperty::new(handle_for(&disk), property(0))
        .with_additional_parameters(shared.clone());

    assert_eq!(from_vec.additional_parameters(), &[1, 2, 3]);
    assert_eq!(from_slice.additional_parameters(), &[1, 2, 3]);
    assert_eq!(from_arc.additional_parameters(), &[1, 2, 3]);
    // The shared buffer is shared, not copied.
    assert!(std::ptr::eq(
        from_arc.additional_parameters().as_ptr(),
        shared.as_ptr()
    ));
}

#[test]
fn a_copy_keeps_the_configuration_and_shares_the_parameters() {
    let _allocating = handle_allocation().read().expect("not poisoned");
    let disk = system_disk(false);
    let original = QueryStorageProperty::new(handle_for(&disk), property(StorageDeviceProperty))
        .with_query_type(StorageQueryType::EXISTS)
        .with_additional_parameters(vec![4_u8, 5, 6]);

    let copy = original.try_clone().expect("duplicate the handle");

    assert_eq!(copy.property(), original.property());
    assert_eq!(copy.query_type(), original.query_type());
    assert_eq!(
        copy.additional_parameters(),
        original.additional_parameters()
    );
    assert!(std::ptr::eq(
        copy.additional_parameters().as_ptr(),
        original.additional_parameters().as_ptr()
    ));
    assert_ne!(copy.handle().raw(), original.handle().raw());
    drop(original);
    copy.perform()
        .expect("a copy still queries after the original is gone");
}

// --- The real disk ---------------------------------------------------------------------------

#[test]
fn a_standard_query_returns_exactly_the_bytes_its_header_says() {
    let _allocating = handle_allocation().read().expect("not poisoned");
    let disk = system_disk(false);

    let reply = QueryStorageProperty::new(handle_for(&disk), property(StorageDeviceProperty))
        .perform()
        .expect("the device property");

    assert!(!reply.as_bytes().is_empty());
    assert_eq!(
        reply
            .size()
            .map(|size| usize::try_from(size).expect("fits")),
        Some(reply.as_bytes().len())
    );
    assert!(reply.version().is_some());
    assert_eq!(reply.clone().into_bytes(), reply.as_bytes());
}

#[test]
fn the_generic_entry_returns_what_the_typed_one_does_for_the_same_property() {
    let _allocating = handle_allocation().read().expect("not poisoned");
    let disk = system_disk(false);

    let generic =
        QueryStorageProperty::new(handle_for(&disk), property(StorageDeviceUniqueIdProperty))
            .perform()
            .expect("the generic query");
    let typed = QueryStorageDeviceUniqueId::new(handle_for(&disk))
        .perform()
        .expect("the typed query");

    assert_eq!(generic.as_bytes(), typed.as_bytes());
}

#[test]
fn a_query_for_whether_a_property_exists_has_no_structure_and_success_means_it_does() {
    let _allocating = handle_allocation().read().expect("not poisoned");
    let disk = system_disk(false);

    for raw in [StorageDeviceProperty, StorageDeviceUniqueIdProperty] {
        let reply = QueryStorageProperty::new(handle_for(&disk), property(raw))
            .with_query_type(StorageQueryType::EXISTS)
            .perform()
            .unwrap_or_else(|error| panic!("property {raw} exists: {error}"));

        assert!(reply.as_bytes().is_empty(), "property {raw}");
        assert_eq!(reply.version(), None);
        assert_eq!(reply.size(), None);
        assert_eq!(reply.to_string(), "no structure");
    }
}

#[test]
fn a_property_the_device_does_not_have_is_the_devices_own_error_for_both_questions() {
    let _allocating = handle_allocation().read().expect("not poisoned");
    let disk = system_disk(false);

    for query_type in [StorageQueryType::STANDARD, StorageQueryType::EXISTS] {
        let outcome = QueryStorageProperty::new(handle_for(&disk), property(NO_SUCH_PROPERTY))
            .with_query_type(query_type)
            .perform();

        match outcome {
            Err(StoragePropertyError::Win32(error)) => {
                eprintln!("an unknown property, {query_type:?}: {error}");
            }
            other => panic!("an unknown property gave {other:?}"),
        }
    }
}

#[test]
fn a_query_type_the_property_does_not_take_is_the_devices_own_error_and_not_read() {
    let _allocating = handle_allocation().read().expect("not poisoned");
    let disk = system_disk(false);

    for query_type in [StorageQueryType::MASK, StorageQueryType::from_raw(77)] {
        let outcome =
            QueryStorageProperty::new(handle_for(&disk), property(StorageDeviceUniqueIdProperty))
                .with_query_type(query_type)
                .perform();

        match outcome {
            Err(StoragePropertyError::Win32(error)) => eprintln!("{query_type:?}: {error}"),
            other => panic!("{query_type:?} gave {other:?}"),
        }
    }
}

#[test]
fn a_property_that_reads_no_additional_parameters_gives_the_same_reply_whatever_they_hold() {
    // The device property was measured to take none.
    let _allocating = handle_allocation().read().expect("not poisoned");
    let disk = system_disk(false);
    let ask = |additional: Vec<u8>| {
        QueryStorageProperty::new(handle_for(&disk), property(StorageDeviceProperty))
            .with_additional_parameters(additional)
            .perform()
            .expect("query")
    };
    let bare = ask(Vec::new());

    for extra in [1_usize, 4, 16, 200] {
        assert_eq!(ask(vec![0xA5_u8; extra]), bare, "{extra} additional bytes");
    }
}

#[test]
fn additional_parameters_reach_the_device_and_what_it_does_with_them_is_its_own() {
    // The device unique identifier property reads the first additional byte: on the machine
    // measured, 0 gave the usual reply, 1 a larger one, and anything larger was refused as
    // `ERROR_INVALID_PARAMETER`. What the byte selects is not documented and is not interpreted
    // here; what is shown is that the bytes arrive, because the answer depends on them.
    let _allocating = handle_allocation().read().expect("not poisoned");
    let disk = system_disk(false);
    let ask = |additional: Vec<u8>| {
        QueryStorageProperty::new(handle_for(&disk), property(StorageDeviceUniqueIdProperty))
            .with_additional_parameters(additional)
            .perform()
    };
    let bare = ask(Vec::new()).expect("bare");

    assert_eq!(
        ask(vec![0]).expect("a zero byte"),
        bare,
        "zero is what no parameters mean"
    );

    match ask(vec![0xA5]) {
        Err(StoragePropertyError::Win32(error)) => eprintln!("an additional byte of 0xA5: {error}"),
        other => panic!("an additional byte of 0xA5 was not refused: {other:?}"),
    }

    match ask(vec![1]) {
        Ok(one) if one != bare => {
            // The reply is still a whole structure, read to the size its own header names.
            assert_eq!(
                one.size().map(|size| usize::try_from(size).expect("fits")),
                Some(one.as_bytes().len())
            );
        }
        Ok(_) => eprintln!("skipped: this device gives the same reply for an additional byte of 1"),
        Err(error) => eprintln!("skipped: this device refuses an additional byte of 1: {error}"),
    }
}

#[test]
fn a_plain_disk_handle_and_an_overlapped_one_get_the_same_reply() {
    let _allocating = handle_allocation().read().expect("not poisoned");
    let plain = system_disk(false);
    let overlapped = system_disk(true);

    let ask = |file: &File| {
        QueryStorageProperty::new(handle_for(file), property(StorageDeviceProperty))
            .perform()
            .expect("query")
    };

    assert_eq!(ask(&plain), ask(&overlapped));
}

#[test]
fn a_handle_that_is_not_a_disk_is_the_devices_own_error() {
    let _allocating = handle_allocation().read().expect("not poisoned");
    let fixture = Fixture::new("sq-file");
    let file = fixture.open_file();

    match QueryStorageProperty::new(handle_for(&file), property(StorageDeviceProperty)).perform() {
        Err(StoragePropertyError::Win32(error)) => eprintln!("an ordinary file: {error}"),
        other => panic!("an ordinary file is not a disk, but gave {other:?}"),
    }
}

#[test]
fn the_request_is_performable_through_the_trait() {
    fn through_the_seam<R: Request<Output = StoragePropertyReply, Error = StoragePropertyError>>(
        request: &R,
    ) -> StoragePropertyReply {
        request.perform().expect("perform")
    }

    let _allocating = handle_allocation().read().expect("not poisoned");
    let disk = system_disk(false);
    let request = QueryStorageProperty::new(handle_for(&disk), property(StorageDeviceProperty));

    assert_eq!(
        through_the_seam(&request),
        request.perform().expect("perform")
    );
}

// --- The reply -------------------------------------------------------------------------------

#[test]
fn a_reply_reads_its_version_and_size_from_its_header() {
    let mut bytes = vec![0xCC_u8; 24];
    bytes[0..4].copy_from_slice(&7_u32.to_ne_bytes());
    bytes[4..8].copy_from_slice(&24_u32.to_ne_bytes());
    let reply = StoragePropertyReply {
        bytes: bytes.clone(),
    };

    assert_eq!(reply.version(), Some(7));
    assert_eq!(reply.size(), Some(24));
    assert_eq!(reply.as_bytes(), bytes.as_slice());
    assert_eq!(reply.to_string(), "24 bytes");

    // A reply too short to hold the fields reports them as absent, not as zero.
    for length in 0..8 {
        let short = StoragePropertyReply {
            bytes: bytes[..length].to_vec(),
        };
        assert_eq!(short.size(), None, "length {length}");
    }
    assert_eq!(
        StoragePropertyReply {
            bytes: bytes[..4].to_vec()
        }
        .version(),
        Some(7)
    );
}

#[test]
fn an_error_from_the_helper_is_carried_unaltered() {
    let error = StoragePropertyError::from(Win32Error::from_code(50));
    assert!(matches!(error, StoragePropertyError::Win32(inner) if inner.code() == 50));
}
