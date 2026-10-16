// Copyright (c) 2026 Mike Grier
//! Tests for the device unique identifier.
//!
//! The real disk shows what a real device reports; the shapes a device could report and this one
//! does not (all three sections, none, an offset that lies) are had from replies built here, whose
//! layout is the documented one.

use std::io;

use super::{StorageDeviceUniqueId, layout, storage_device_unique_id};
use crate::test_support::{ordinary_file, system_disk};

/// A section of `payload.len()` bytes after its own version and size.
fn section(version: u32, payload: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&version.to_ne_bytes());
    bytes.extend_from_slice(
        &u32::try_from(layout::SECTION_HEADER + payload.len())
            .expect("small")
            .to_ne_bytes(),
    );
    bytes.extend_from_slice(payload);
    bytes
}

/// A reply with the given sections after the header, in the order id, device, signature.
fn duid(
    version: u32,
    id: Option<&[u8]>,
    device: Option<&[u8]>,
    signature: Option<&[u8]>,
) -> Vec<u8> {
    let mut body = Vec::new();
    let mut place = |section: Option<&[u8]>| -> u32 {
        section.map_or(0, |section| {
            let offset = layout::HEADER + body.len();
            body.extend_from_slice(section);
            u32::try_from(offset).expect("small")
        })
    };
    let id_at = place(id);
    let device_at = place(device);
    let signature_at = place(signature);

    let mut bytes = vec![0_u8; layout::HEADER];
    let size = u32::try_from(layout::HEADER + body.len()).expect("small");
    bytes[layout::VERSION..][..4].copy_from_slice(&version.to_ne_bytes());
    bytes[layout::SIZE..][..4].copy_from_slice(&size.to_ne_bytes());
    bytes[layout::ID_DESCRIPTOR_OFFSET..][..4].copy_from_slice(&id_at.to_ne_bytes());
    bytes[layout::DEVICE_DESCRIPTOR_OFFSET..][..4].copy_from_slice(&device_at.to_ne_bytes());
    bytes[layout::LAYOUT_SIGNATURE_OFFSET..][..4].copy_from_slice(&signature_at.to_ne_bytes());
    bytes.extend_from_slice(&body);
    bytes
}

fn put_u32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_ne_bytes());
}

fn malformed(bytes: Vec<u8>) -> bool {
    matches!(
        StorageDeviceUniqueId::decode(bytes),
        Err(error) if error.kind() == io::ErrorKind::InvalidData
    )
}

// --- The real disk --------------------------------------------------------------------------

#[test]
fn a_real_disk_reports_a_header_and_at_least_one_section_that_is_what_it_says() {
    let disk = system_disk(false);

    let id = storage_device_unique_id(&disk).expect("query the disk");

    assert_eq!(
        usize::try_from(id.size()).expect("fits"),
        id.as_bytes().len(),
        "the size in the header is the length of the reply"
    );
    let sections = [
        id.device_id_descriptor(),
        id.device_descriptor(),
        id.drive_layout_signature(),
    ];
    assert!(
        sections.iter().any(Option::is_some),
        "a disk reports at least one identifier section: {id}"
    );
    for section in sections.into_iter().flatten() {
        let own_size = u32::from_ne_bytes(
            section[layout::SECTION_SIZE..][..4]
                .try_into()
                .expect("four"),
        );
        assert_eq!(
            usize::try_from(own_size).expect("fits"),
            section.len(),
            "a section is exactly the size its own header gives"
        );
    }
    eprintln!("the system disk: {id}");
}

#[test]
fn asking_twice_gives_the_same_identifier() {
    let disk = system_disk(false);

    assert_eq!(
        storage_device_unique_id(&disk).expect("first"),
        storage_device_unique_id(&disk).expect("second")
    );
}

#[test]
fn an_overlapped_disk_handle_gets_the_same_identifier_as_a_plain_one() {
    let plain = system_disk(false);
    let overlapped = system_disk(true);

    assert_eq!(
        storage_device_unique_id(&plain).expect("plain"),
        storage_device_unique_id(&overlapped).expect("overlapped")
    );
}

#[test]
fn a_handle_that_is_not_a_disk_is_the_devices_own_error() {
    let (file, path) = ordinary_file("duid-file");
    let error = storage_device_unique_id(&file).expect_err("an ordinary file is not a disk");
    assert_ne!(error.kind(), io::ErrorKind::InvalidData, "{error}");
    assert!(error.raw_os_error().is_some(), "{error}");
    eprintln!("an ordinary file: {error}");
    drop(file);
    std::fs::remove_file(path).expect("remove");

    let nul = std::fs::File::open("NUL").expect("open the NUL device");
    let error = storage_device_unique_id(&nul).expect_err("the NUL device is not a disk");
    assert!(error.raw_os_error().is_some(), "{error}");
    eprintln!("the NUL device: {error}");
}

// --- Reading a reply -------------------------------------------------------------------------

#[test]
fn every_section_a_header_points_to_is_delimited_by_its_own_size() {
    let id = section(16, b"id-descriptor-bytes");
    let device = section(24, b"device-descriptor");
    let signature = section(5, b"sig");
    let bytes = duid(1, Some(&id), Some(&device), Some(&signature));

    let got = StorageDeviceUniqueId::decode(bytes.clone()).expect("a whole reply");

    assert_eq!(got.version(), 1);
    assert_eq!(usize::try_from(got.size()).expect("fits"), bytes.len());
    assert_eq!(got.as_bytes(), bytes.as_slice());
    assert_eq!(got.device_id_descriptor(), Some(id.as_slice()));
    assert_eq!(got.device_descriptor(), Some(device.as_slice()));
    assert_eq!(got.drive_layout_signature(), Some(signature.as_slice()));
}

#[test]
fn an_offset_of_zero_is_a_section_that_is_not_there() {
    let id = section(16, b"only-this");
    let signature = section(5, b"sig");

    let only_id = StorageDeviceUniqueId::decode(duid(1, Some(&id), None, None)).expect("decode");
    assert_eq!(only_id.device_id_descriptor(), Some(id.as_slice()));
    assert_eq!(only_id.device_descriptor(), None);
    assert_eq!(only_id.drive_layout_signature(), None);

    let only_signature =
        StorageDeviceUniqueId::decode(duid(1, None, None, Some(&signature))).expect("decode");
    assert_eq!(only_signature.device_id_descriptor(), None);
    assert_eq!(
        only_signature.drive_layout_signature(),
        Some(signature.as_slice())
    );

    let none = StorageDeviceUniqueId::decode(duid(1, None, None, None)).expect("decode");
    assert_eq!(none.device_id_descriptor(), None);
    assert_eq!(none.device_descriptor(), None);
    assert_eq!(none.drive_layout_signature(), None);
    assert_eq!(none.as_bytes().len(), layout::HEADER);
}

#[test]
fn each_offset_is_read_from_its_own_field_not_its_neighbours() {
    // Distinct payloads at distinct offsets, so a section read from the wrong field shows.
    let id = section(1, b"AAAA");
    let device = section(2, b"BBBBBBBB");
    let signature = section(3, b"CCCCCCCCCCCC");
    let got = StorageDeviceUniqueId::decode(duid(7, Some(&id), Some(&device), Some(&signature)))
        .expect("decode");

    assert_eq!(got.version(), 7);
    assert_ne!(got.device_id_descriptor(), got.device_descriptor());
    assert_ne!(got.device_descriptor(), got.drive_layout_signature());
    assert_eq!(got.device_id_descriptor().map(<[u8]>::len), Some(id.len()));
    assert_eq!(got.device_descriptor().map(<[u8]>::len), Some(device.len()));
    assert_eq!(
        got.drive_layout_signature().map(<[u8]>::len),
        Some(signature.len())
    );
}

#[test]
fn a_section_that_ends_exactly_at_the_end_of_the_reply_is_accepted() {
    let id = section(16, b"fills-to-the-end");
    let bytes = duid(1, Some(&id), None, None);
    assert_eq!(layout::HEADER + id.len(), bytes.len());

    assert!(StorageDeviceUniqueId::decode(bytes).is_ok());
}

#[test]
fn a_reply_that_is_not_what_it_says_is_not_read() {
    let id = section(16, b"payload-of-some-length");
    let whole = duid(1, Some(&id), None, None);

    // Shorter than the header it must have.
    for length in 0..layout::HEADER {
        assert!(malformed(whole[..length].to_vec()), "length {length}");
    }

    // An offset inside the header, which would make the header its own section.
    for offset in 1..layout::HEADER {
        let mut bytes = whole.clone();
        put_u32(
            &mut bytes,
            layout::ID_DESCRIPTOR_OFFSET,
            u32::try_from(offset).expect("small"),
        );
        assert!(malformed(bytes), "offset {offset}");
    }

    // An offset with no room for the section's own header.
    for offset in [
        whole.len() - 7,
        whole.len() - 1,
        whole.len(),
        whole.len() + 100,
    ] {
        let mut bytes = whole.clone();
        put_u32(
            &mut bytes,
            layout::ID_DESCRIPTOR_OFFSET,
            u32::try_from(offset).expect("small"),
        );
        assert!(malformed(bytes), "offset {offset}");
    }
    let mut huge = whole.clone();
    put_u32(&mut huge, layout::DEVICE_DESCRIPTOR_OFFSET, u32::MAX);
    assert!(malformed(huge));

    // A section that says it is smaller than its own header, and one that runs past the end.
    for size in 0..layout::SECTION_HEADER {
        let mut bytes = whole.clone();
        put_u32(
            &mut bytes,
            layout::HEADER + layout::SECTION_SIZE,
            u32::try_from(size).expect("small"),
        );
        assert!(malformed(bytes), "section size {size}");
    }
    let mut long = whole.clone();
    put_u32(
        &mut long,
        layout::HEADER + layout::SECTION_SIZE,
        u32::try_from(id.len() + 1).expect("small"),
    );
    assert!(malformed(long));
    let mut huge_size = whole.clone();
    put_u32(
        &mut huge_size,
        layout::HEADER + layout::SECTION_SIZE,
        u32::MAX,
    );
    assert!(malformed(huge_size));

    // And each of the three offsets is checked, not just the first.
    for field in [
        layout::ID_DESCRIPTOR_OFFSET,
        layout::DEVICE_DESCRIPTOR_OFFSET,
        layout::LAYOUT_SIGNATURE_OFFSET,
    ] {
        let mut bytes = whole.clone();
        put_u32(
            &mut bytes,
            field,
            u32::try_from(whole.len() + 1).expect("small"),
        );
        assert!(malformed(bytes), "field {field}");
    }
}

#[test]
fn an_identifier_displays_its_version_size_and_which_sections_it_has() {
    let id = section(16, b"x");
    let got = StorageDeviceUniqueId::decode(duid(1, Some(&id), None, None)).expect("decode");

    let text = got.to_string();
    assert!(text.contains("version 1"), "{text}");
    assert!(text.contains(&format!("{} bytes", got.size())), "{text}");
    assert!(text.contains("device id yes"), "{text}");
    assert!(text.contains("device descriptor no"), "{text}");
    assert!(text.contains("layout signature no"), "{text}");
}
