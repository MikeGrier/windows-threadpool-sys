// Copyright (c) Mike Grier.

//! Tests for the `GetStorageDependencyInformation` entry.
//!
//! What a machine with no attached virtual disk and an unelevated token can show is the call's
//! refusals and its empty answer: no entry was ever observed, because attaching one needs an
//! elevated token. So the entries, the names, and the growth on `ERROR_INSUFFICIENT_BUFFER` are
//! tested against replies built here, laid out as the structure is declared, and against a fake
//! device that checks what it was asked. Whether a real entry's names lie inside the buffer the
//! caller gave is the part nothing here can show.

use std::fs::{File, OpenOptions};
use std::mem::offset_of;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsHandle, AsRawHandle};

use io_buffers::AlignedBuf;
use windows_sys::Win32::Foundation::{
    ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_LEVEL, ERROR_INVALID_PARAMETER, ERROR_MORE_DATA,
    ERROR_NOT_ENOUGH_MEMORY,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
};
use windows_sys::Win32::Storage::Vhd::{
    STORAGE_DEPENDENCY_INFO, STORAGE_DEPENDENCY_INFO_TYPE_1, STORAGE_DEPENDENCY_INFO_TYPE_2,
    VIRTUAL_STORAGE_TYPE,
};
use wtf_string::Wtf16String;

use super::{
    ALIGNMENT, DependentDiskFlags, FIRST_BYTES, MAX_ATTEMPTS, MIN_BYTES,
    QueryStorageDependencyInformation, StorageDependencyError, StorageDependencyFlags,
    StorageDependencyInfo, StorageDependencyVersion, decode, dependencies_via,
};
use crate::handle::tests::handle_allocation;
use crate::request::Request;
use crate::storage_property::tests::system_disk;
use crate::{CapturedHandle, Win32Error};

/// The status a handle that is not on a virtual disk gets for `HOST_VOLUMES`: an `NTSTATUS`
/// value, which the call returns as it is.
const NOT_A_VIRTUAL_DISK: u32 = 0xC03A_0015;

const FIRST_ENTRY: usize = offset_of!(STORAGE_DEPENDENCY_INFO, Anonymous);
const ENTRY_1: usize = size_of::<STORAGE_DEPENDENCY_INFO_TYPE_1>();
const ENTRY_2: usize = size_of::<STORAGE_DEPENDENCY_INFO_TYPE_2>();

fn system_drive() -> String {
    std::env::var("SystemDrive").unwrap_or_else(|_| "C:".to_owned())
}

/// The system volume as a device path, opened with no access, which an ordinary account can do.
fn system_volume() -> File {
    OpenOptions::new()
        .access_mode(0)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .open(format!(r"\\.\{}", system_drive()))
        .expect("open the system volume with no access")
}

/// The system volume's root directory, which the call also takes.
fn system_root() -> File {
    OpenOptions::new()
        .access_mode(0)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(format!(r"{}\", system_drive()))
        .expect("open the system volume's root with no access")
}

fn request_for(file: &File) -> QueryStorageDependencyInformation {
    QueryStorageDependencyInformation::new(
        CapturedHandle::capture(file.as_handle()).expect("capture"),
    )
}

fn win32_code(error: &StorageDependencyError) -> u32 {
    match error {
        StorageDependencyError::Win32(error) => error.code(),
        other => panic!("expected a Win32 failure, got {other:?}"),
    }
}

// --- A reply, laid out as the structure is declared -----------------------------------------

/// What an entry of a built reply holds. The four names are in the order the structure has them.
#[derive(Clone, Default)]
struct Built {
    type_flags: i32,
    provider: u32,
    device_id: u32,
    vendor: [u8; 16],
    ancestor: u32,
    names: [Option<&'static str>; 4],
}

/// An entry in which every field has a value no other field has, so one read from the wrong
/// place shows.
fn distinct(seed: u8) -> Built {
    let seed32 = u32::from(seed);
    Built {
        type_flags: 0x0100_0000 + i32::from(seed),
        provider: 0x0200_0000 + seed32,
        device_id: 0x0300_0000 + seed32,
        vendor: core::array::from_fn(|index| {
            seed.wrapping_add(u8::try_from(index).expect("small"))
        }),
        ancestor: 0x0400_0000 + seed32,
        names: [
            Some(r"\\.\PhysicalDrive7"),
            Some(r"\\?\Volume{11111111-2222-3333-4444-555555555555}\"),
            Some(r"\\?\Volume{66666666-7777-8888-9999-000000000000}\"),
            Some(r"images\disk.vhdx"),
        ],
    }
}

fn put(bytes: &mut [u8], at: usize, value: &[u8]) {
    bytes[at..at + value.len()].copy_from_slice(value);
}

/// How many bytes a reply of `version` holding `entries` takes, the names after the entries.
fn needed(version: i32, entries: &[Built]) -> usize {
    let entry = if version == 1 { ENTRY_1 } else { ENTRY_2 };
    let names: usize = if version == 2 {
        entries
            .iter()
            .flat_map(|built| built.names.iter().flatten())
            .map(|name| (name.encode_utf16().count() + 1) * 2)
            .sum()
    } else {
        0
    };
    (FIRST_ENTRY + entries.len() * entry + names).max(MIN_BYTES)
}

/// Writes a reply of `version` holding `entries` into `bytes`, whose first byte is where the
/// name pointers are made to point from. It must be `needed` bytes at least.
fn fill(bytes: &mut [u8], version: i32, entries: &[Built]) {
    let base = bytes.as_ptr().addr();
    put(
        bytes,
        offset_of!(STORAGE_DEPENDENCY_INFO, Version),
        &version.to_ne_bytes(),
    );
    let count = u32::try_from(entries.len()).expect("small");
    put(
        bytes,
        offset_of!(STORAGE_DEPENDENCY_INFO, NumberEntries),
        &count.to_ne_bytes(),
    );

    let entry_size = if version == 1 { ENTRY_1 } else { ENTRY_2 };
    let mut names_at = FIRST_ENTRY + entries.len() * entry_size;
    for (index, built) in entries.iter().enumerate() {
        let at = FIRST_ENTRY + index * entry_size;
        put(
            bytes,
            at + offset_of!(STORAGE_DEPENDENCY_INFO_TYPE_1, DependencyTypeFlags),
            &built.type_flags.to_ne_bytes(),
        );
        put(
            bytes,
            at + offset_of!(STORAGE_DEPENDENCY_INFO_TYPE_1, ProviderSpecificFlags),
            &built.provider.to_ne_bytes(),
        );
        let kind = at + offset_of!(STORAGE_DEPENDENCY_INFO_TYPE_1, VirtualStorageType);
        put(
            bytes,
            kind + offset_of!(VIRTUAL_STORAGE_TYPE, DeviceId),
            &built.device_id.to_ne_bytes(),
        );
        put(
            bytes,
            kind + offset_of!(VIRTUAL_STORAGE_TYPE, VendorId),
            &built.vendor,
        );
        if version != 2 {
            continue;
        }
        put(
            bytes,
            at + offset_of!(STORAGE_DEPENDENCY_INFO_TYPE_2, AncestorLevel),
            &built.ancestor.to_ne_bytes(),
        );
        let slots = [
            offset_of!(STORAGE_DEPENDENCY_INFO_TYPE_2, DependencyDeviceName),
            offset_of!(STORAGE_DEPENDENCY_INFO_TYPE_2, HostVolumeName),
            offset_of!(STORAGE_DEPENDENCY_INFO_TYPE_2, DependentVolumeName),
            offset_of!(STORAGE_DEPENDENCY_INFO_TYPE_2, DependentVolumeRelativePath),
        ];
        for (slot, name) in slots.into_iter().zip(built.names) {
            let Some(name) = name else { continue };
            put(bytes, at + slot, &(base + names_at).to_ne_bytes());
            for unit in name.encode_utf16().chain([0]) {
                put(bytes, names_at, &unit.to_ne_bytes());
                names_at += 2;
            }
        }
    }
}

/// A buffer holding a reply of `version` with `entries`, with `extra` bytes after it.
fn built(version: i32, entries: &[Built], extra: usize) -> AlignedBuf {
    let mut buffer =
        AlignedBuf::zeroed(needed(version, entries) + extra, ALIGNMENT).expect("alloc");
    fill(buffer.as_mut_slice(), version, entries);
    buffer
}

fn decode_built(
    version: i32,
    entries: &[Built],
) -> Result<StorageDependencyInfo, StorageDependencyError> {
    let buffer = built(version, entries, 0);
    decode(
        buffer.as_slice(),
        u32::try_from(buffer.len()).expect("small"),
    )
}

/// Asserts that `result` is a `Malformed` whose reason contains `reason`, so that a guard is
/// shown to be the one that refused and not a later one that happens to refuse the same bytes.
#[track_caller]
fn assert_malformed(result: Result<StorageDependencyInfo, StorageDependencyError>, reason: &str) {
    match result {
        Err(StorageDependencyError::Malformed { what }) => {
            assert!(
                what.contains(reason),
                "refused for `{what}`, not `{reason}`"
            );
        }
        other => panic!("expected a reply refused for `{reason}`, got {other:?}"),
    }
}

fn name(text: &str) -> Option<Wtf16String> {
    Some(Wtf16String::from(text))
}

// --- The real machine ----------------------------------------------------------------------

#[test]
fn a_volume_that_hosts_nothing_answers_with_no_entries_in_either_version() {
    let _allocating = handle_allocation().read().expect("not poisoned");
    let volume = system_volume();

    for version in [StorageDependencyVersion::V1, StorageDependencyVersion::V2] {
        let info = request_for(&volume)
            .with_version(version)
            .perform()
            .unwrap_or_else(|error| panic!("{version:?}: {error}"));

        assert_eq!(info.version(), version, "the version is echoed back");
        assert!(info.entries().is_empty(), "{info:?}");
    }
}

#[test]
fn a_root_directory_and_a_disk_answer_as_a_volume_does() {
    let _allocating = handle_allocation().read().expect("not poisoned");

    let root = request_for(&system_root())
        .perform()
        .expect("a root directory");
    let disk = request_for(&system_disk(false)).perform().expect("a disk");

    assert!(root.entries().is_empty(), "{root:?}");
    assert!(disk.entries().is_empty(), "{disk:?}");
}

#[test]
fn a_first_buffer_below_the_structure_is_raised_and_never_asked() {
    // Measured: anything under the structure's own size is ERROR_INVALID_PARAMETER and writes
    // nothing, so a request configured smaller must still succeed.
    let _allocating = handle_allocation().read().expect("not poisoned");
    let volume = system_volume();

    for bytes in [0, 1, 8, MIN_BYTES - 1, MIN_BYTES, MIN_BYTES + 1] {
        let info = request_for(&volume)
            .with_capacity(bytes)
            .perform()
            .unwrap_or_else(|error| panic!("a first buffer of {bytes} bytes: {error}"));
        assert!(info.entries().is_empty());
    }
}

#[test]
fn a_version_the_device_does_not_have_is_the_devices_to_refuse() {
    let _allocating = handle_allocation().read().expect("not poisoned");
    let volume = system_volume();

    for raw in [0, 3, 4, -1, i32::MAX] {
        let error = request_for(&volume)
            .with_version(StorageDependencyVersion::from_raw(raw))
            .perform()
            .expect_err("a version that is not 1 or 2");
        assert_eq!(win32_code(&error), ERROR_INVALID_LEVEL, "version {raw}");
    }
}

#[test]
fn the_host_direction_on_a_handle_that_is_not_virtual_is_the_raw_status() {
    // The status is an NTSTATUS value, not a Win32 error code, and is carried as the call
    // returned it. A system volume that is itself on a virtual disk answers with its host
    // instead, which is the other thing this call is for.
    let _allocating = handle_allocation().read().expect("not poisoned");
    let volume = system_volume();

    match request_for(&volume)
        .with_flags(StorageDependencyFlags::HOST_VOLUMES)
        .perform()
    {
        Err(error) => assert_eq!(win32_code(&error), NOT_A_VIRTUAL_DISK, "{error}"),
        Ok(info) => {
            eprintln!("the system volume is virtual: {info:?}");
            assert!(!info.entries().is_empty());
        }
    }
}

#[test]
fn the_disk_handle_flag_is_the_devices_to_answer_on_a_volume() {
    let _allocating = handle_allocation().read().expect("not poisoned");
    let volume = system_volume();

    let error = request_for(&volume)
        .with_flags(StorageDependencyFlags::DISK_HANDLE)
        .perform()
        .expect_err("a volume is not a disk");
    assert_eq!(win32_code(&error), ERROR_INVALID_PARAMETER, "{error}");
}

#[test]
fn the_request_owns_its_handle_is_cloneable_and_performable_through_the_trait() {
    fn through_the_seam<
        R: Request<Output = StorageDependencyInfo, Error = StorageDependencyError>,
    >(
        request: &R,
    ) -> StorageDependencyInfo {
        request.perform().expect("perform")
    }

    let _allocating = handle_allocation().read().expect("not poisoned");
    let volume = system_volume();
    let request = request_for(&volume)
        .with_flags(StorageDependencyFlags::NONE)
        .with_version(StorageDependencyVersion::V1)
        .with_capacity(123);

    assert_ne!(request.handle().raw(), volume.as_raw_handle());
    assert_eq!(request.flags(), StorageDependencyFlags::NONE);
    assert_eq!(request.version(), StorageDependencyVersion::V1);
    assert_eq!(request.capacity(), 123);

    let first = through_the_seam(&request);
    let copy = request.try_clone().expect("duplicate the handle");
    drop(request);
    drop(volume);

    assert_eq!(copy.version(), StorageDependencyVersion::V1);
    assert_eq!(copy.capacity(), 123);
    assert_eq!(copy.perform().expect("the copy"), first);
}

#[test]
fn a_new_request_asks_for_version_two_with_no_flags_and_the_documented_first_size() {
    let _allocating = handle_allocation().read().expect("not poisoned");
    let volume = system_volume();
    let request = request_for(&volume);

    assert_eq!(request.version(), StorageDependencyVersion::V2);
    assert_eq!(request.flags(), StorageDependencyFlags::default());
    assert_eq!(request.capacity(), FIRST_BYTES);
}

// --- Flags and versions ---------------------------------------------------------------------

#[test]
fn the_flags_are_the_win32_values_and_combine() {
    assert_eq!(StorageDependencyFlags::NONE.bits(), 0);
    assert_eq!(StorageDependencyFlags::HOST_VOLUMES.bits(), 1);
    assert_eq!(StorageDependencyFlags::DISK_HANDLE.bits(), 2);
    assert_eq!(
        (StorageDependencyFlags::HOST_VOLUMES | StorageDependencyFlags::DISK_HANDLE).bits(),
        3
    );
    assert_eq!(StorageDependencyFlags::from_bits(0x40).bits(), 0x40);

    let both = StorageDependencyFlags::HOST_VOLUMES | StorageDependencyFlags::DISK_HANDLE;
    assert!(both.contains(StorageDependencyFlags::HOST_VOLUMES));
    assert!(both.contains(StorageDependencyFlags::DISK_HANDLE));
    assert!(both.contains(StorageDependencyFlags::NONE));
    assert!(!StorageDependencyFlags::HOST_VOLUMES.contains(StorageDependencyFlags::DISK_HANDLE));
    assert!(!StorageDependencyFlags::NONE.contains(StorageDependencyFlags::HOST_VOLUMES));
}

#[test]
fn the_versions_are_the_win32_values() {
    assert_eq!(StorageDependencyVersion::V1.raw(), 1);
    assert_eq!(StorageDependencyVersion::V2.raw(), 2);
    assert_eq!(StorageDependencyVersion::from_raw(9).raw(), 9);
}

#[test]
fn the_dependent_disk_flags_are_the_win32_values_and_combine() {
    let named = [
        (DependentDiskFlags::NONE, 0),
        (DependentDiskFlags::MULT_BACKING_FILES, 1),
        (DependentDiskFlags::FULLY_ALLOCATED, 2),
        (DependentDiskFlags::READ_ONLY, 4),
        (DependentDiskFlags::REMOTE, 8),
        (DependentDiskFlags::SYSTEM_VOLUME, 16),
        (DependentDiskFlags::SYSTEM_VOLUME_PARENT, 32),
        (DependentDiskFlags::REMOVABLE, 64),
        (DependentDiskFlags::NO_DRIVE_LETTER, 128),
        (DependentDiskFlags::PARENT, 256),
        (DependentDiskFlags::NO_HOST_DISK, 512),
        (DependentDiskFlags::PERMANENT_LIFETIME, 1024),
        (DependentDiskFlags::SUPPORT_COMPRESSED_VOLUMES, 2048),
        (DependentDiskFlags::ALWAYS_ALLOW_SPARSE, 4096),
        (DependentDiskFlags::SUPPORT_ENCRYPTED_FILES, 8192),
    ];
    for (flag, bits) in named {
        assert_eq!(flag.bits(), bits);
        assert_eq!(DependentDiskFlags::from_bits(bits), flag);
    }

    let both = DependentDiskFlags::REMOVABLE | DependentDiskFlags::PARENT;
    assert_eq!(both.bits(), 64 + 256);
    assert!(both.contains(DependentDiskFlags::REMOVABLE));
    assert!(both.contains(DependentDiskFlags::PARENT));
    assert!(both.contains(DependentDiskFlags::NONE));
    assert!(!both.contains(DependentDiskFlags::REMOTE));
}

// --- What the entry does with a reply -------------------------------------------------------

#[test]
fn a_version_two_entry_has_every_field_read_from_where_the_structure_has_it() {
    let info = decode_built(2, &[distinct(1)]).expect("a whole reply");
    let entry = &info.entries()[0];
    let wanted = distinct(1);

    assert_eq!(info.version(), StorageDependencyVersion::V2);
    assert_eq!(info.entries().len(), 1);
    assert_eq!(entry.dependency_type_flags().bits(), wanted.type_flags);
    assert_eq!(entry.provider_specific_flags(), wanted.provider);
    assert_eq!(entry.virtual_storage_type().device_id(), wanted.device_id);
    assert_eq!(entry.virtual_storage_type().vendor_id(), wanted.vendor);
    assert_eq!(entry.ancestor_level(), Some(wanted.ancestor));
    assert_eq!(
        entry.dependency_device_name(),
        name(wanted.names[0].expect("set")).as_ref()
    );
    assert_eq!(
        entry.host_volume_name(),
        name(wanted.names[1].expect("set")).as_ref()
    );
    assert_eq!(
        entry.dependent_volume_name(),
        name(wanted.names[2].expect("set")).as_ref()
    );
    assert_eq!(
        entry.dependent_volume_relative_path(),
        name(wanted.names[3].expect("set")).as_ref()
    );
}

#[test]
fn a_version_one_entry_has_its_fields_and_no_names() {
    let info = decode_built(1, &[distinct(2), distinct(3)]).expect("a whole reply");

    assert_eq!(info.version(), StorageDependencyVersion::V1);
    assert_eq!(info.entries().len(), 2);
    for (entry, seed) in info.entries().iter().zip([2, 3]) {
        let wanted = distinct(seed);
        assert_eq!(entry.dependency_type_flags().bits(), wanted.type_flags);
        assert_eq!(entry.provider_specific_flags(), wanted.provider);
        assert_eq!(entry.virtual_storage_type().device_id(), wanted.device_id);
        assert_eq!(entry.virtual_storage_type().vendor_id(), wanted.vendor);
        assert_eq!(entry.ancestor_level(), None);
        assert_eq!(entry.dependency_device_name(), None);
        assert_eq!(entry.host_volume_name(), None);
        assert_eq!(entry.dependent_volume_name(), None);
        assert_eq!(entry.dependent_volume_relative_path(), None);
    }
}

#[test]
fn several_version_two_entries_are_each_read_at_their_own_offset_with_their_own_names() {
    let entries: Vec<Built> = (1..=5).map(distinct).collect();
    let info = decode_built(2, &entries).expect("a whole reply");

    assert_eq!(info.entries().len(), 5);
    for (entry, wanted) in info.entries().iter().zip(&entries) {
        assert_eq!(entry.dependency_type_flags().bits(), wanted.type_flags);
        assert_eq!(entry.virtual_storage_type().device_id(), wanted.device_id);
        assert_eq!(entry.ancestor_level(), Some(wanted.ancestor));
        assert_eq!(
            entry.host_volume_name(),
            name(wanted.names[1].expect("set")).as_ref()
        );
    }
}

#[test]
fn a_null_name_is_none_and_the_others_are_still_read() {
    for missing in 0..4 {
        let mut built_entry = distinct(4);
        built_entry.names[missing] = None;
        let info = decode_built(2, &[built_entry.clone()]).expect("a whole reply");
        let entry = &info.entries()[0];

        let got = [
            entry.dependency_device_name(),
            entry.host_volume_name(),
            entry.dependent_volume_name(),
            entry.dependent_volume_relative_path(),
        ];
        for (slot, (got, wanted)) in got.into_iter().zip(built_entry.names).enumerate() {
            assert_eq!(
                got,
                wanted.and_then(name).as_ref(),
                "name {slot}, with {missing} null"
            );
        }
    }
}

#[test]
fn an_empty_name_is_an_empty_string_and_not_none() {
    let mut built_entry = distinct(5);
    built_entry.names = [Some(""), None, None, None];
    let info = decode_built(2, &[built_entry]).expect("a whole reply");

    assert_eq!(
        info.entries()[0].dependency_device_name(),
        name("").as_ref()
    );
}

#[test]
fn a_name_with_characters_outside_the_basic_plane_is_carried_as_the_units_the_device_wrote() {
    let mut built_entry = distinct(6);
    built_entry.names = [Some("disk-\u{1F4BE}-\u{00E9}"), None, None, None];
    let info = decode_built(2, &[built_entry]).expect("a whole reply");

    assert_eq!(
        info.entries()[0].dependency_device_name(),
        name("disk-\u{1F4BE}-\u{00E9}").as_ref()
    );
}

#[test]
fn a_reply_that_holds_no_entries_is_empty_and_not_an_error() {
    for version in [1, 2] {
        let info = decode_built(version, &[]).expect("no entries");
        assert!(info.entries().is_empty());
        assert_eq!(info.version().raw(), version);
    }
}

#[test]
fn flag_bits_are_carried_as_the_device_wrote_them_including_the_top_one() {
    for bits in [0, 1, 64 + 256, 0x3FFF, 0x4000, i32::MIN, -1] {
        let mut built_entry = distinct(7);
        built_entry.type_flags = bits;
        let info = decode_built(1, &[built_entry]).expect("a whole reply");
        assert_eq!(info.entries()[0].dependency_type_flags().bits(), bits);
    }
}

#[test]
fn a_name_pointer_outside_the_reply_is_reported_and_not_read() {
    // Each of these is an address a hostile or damaged reply could hold, for the first name.
    let slot = FIRST_ENTRY + offset_of!(STORAGE_DEPENDENCY_INFO_TYPE_2, DependencyDeviceName);

    for (what, pick) in [
        (
            "below the reply",
            (|base: usize, _len: usize| base - 8) as fn(usize, usize) -> usize,
        ),
        ("at its end", |base, len| base + len),
        ("past its end", |base, len| base + len + 4096),
        ("at address 1", |_, _| 1),
        ("at the top of the address space", |_, _| usize::MAX),
    ] {
        let mut buffer = built(2, &[distinct(8)], 0);
        let base = buffer.as_slice().as_ptr().addr();
        let len = buffer.len();
        put(buffer.as_mut_slice(), slot, &pick(base, len).to_ne_bytes());

        eprintln!("a name {what}");
        assert_malformed(
            decode(buffer.as_slice(), u32::try_from(len).expect("small")),
            "does not lie",
        );
    }
}

#[test]
fn a_name_that_runs_to_the_end_of_the_reply_without_a_terminator_is_reported() {
    // The last name's terminator is overwritten with a letter, so it runs to the end.
    let mut buffer = built(2, &[distinct(9)], 0);
    let used = buffer.len();
    put(
        buffer.as_mut_slice(),
        used - 2,
        &u16::from(b'x').to_ne_bytes(),
    );

    assert_malformed(
        decode(buffer.as_slice(), u32::try_from(used).expect("small")),
        "no terminator",
    );
}

#[test]
fn a_name_whose_terminator_lies_beyond_what_the_call_used_is_reported() {
    // The bytes are there and the terminator is in them, but the call said it used fewer: what
    // it did not claim to have written is not read.
    let buffer = built(2, &[distinct(9)], 8);
    let used = needed(2, &[distinct(9)]);

    assert!(decode(buffer.as_slice(), u32::try_from(used).expect("small")).is_ok());
    for short in [1, 2, 3] {
        eprintln!("used {short} short");
        assert_malformed(
            decode(
                buffer.as_slice(),
                u32::try_from(used - short).expect("small"),
            ),
            "no terminator",
        );
    }
}

#[test]
fn a_name_that_ends_exactly_at_the_end_of_the_reply_is_read() {
    // The terminator is the last two bytes used: the boundary the check above is one past.
    let buffer = built(2, &[distinct(10)], 0);
    assert_eq!(buffer.len(), needed(2, &[distinct(10)]));

    let info = decode(
        buffer.as_slice(),
        u32::try_from(buffer.len()).expect("small"),
    )
    .expect("a terminator in the last two bytes");
    assert_eq!(
        info.entries()[0].dependent_volume_relative_path(),
        name(r"images\disk.vhdx").as_ref()
    );
}

#[test]
fn a_count_the_reply_cannot_hold_is_reported_and_not_read() {
    let entries = [distinct(11), distinct(12)];
    let len = built(2, &entries, 0).len();
    // The most entries whose own bytes fit in the reply, names and all, and the next one.
    let fits = (len - FIRST_ENTRY) / ENTRY_2;
    let fits = u32::try_from(fits).expect("small");

    for count in [fits + 1, fits + 2, 1000, u32::MAX] {
        let mut buffer = built(2, &entries, 0);
        put(
            buffer.as_mut_slice(),
            offset_of!(STORAGE_DEPENDENCY_INFO, NumberEntries),
            &count.to_ne_bytes(),
        );
        let used = u32::try_from(buffer.len()).expect("small");

        eprintln!("a count of {count}");
        assert_malformed(decode(buffer.as_slice(), used), "count");
    }
}

#[test]
fn a_count_that_fits_exactly_is_read_and_one_more_is_not() {
    // A version 1 reply with no names: the bytes are exactly the header and the entries.
    let entries = [distinct(13), distinct(14)];
    let mut buffer = built(1, &entries, 0);
    let end = FIRST_ENTRY + 2 * ENTRY_1;
    let used = u32::try_from(end).expect("small");
    assert!(buffer.len() >= end);

    assert!(decode(buffer.as_slice(), used).is_ok());
    put(
        buffer.as_mut_slice(),
        offset_of!(STORAGE_DEPENDENCY_INFO, NumberEntries),
        &3_u32.to_ne_bytes(),
    );
    assert_malformed(decode(buffer.as_slice(), used), "count");
}

#[test]
fn a_reply_in_a_version_this_entry_does_not_know_is_reported_and_not_read() {
    for version in [0_i32, 3, 4, -1, i32::MAX] {
        let mut buffer = built(2, &[distinct(15)], 0);
        put(
            buffer.as_mut_slice(),
            offset_of!(STORAGE_DEPENDENCY_INFO, Version),
            &version.to_ne_bytes(),
        );
        let used = u32::try_from(buffer.len()).expect("small");

        assert!(
            matches!(
                decode(buffer.as_slice(), used),
                Err(StorageDependencyError::Malformed { .. })
            ),
            "version {version}"
        );
    }
}

#[test]
fn a_version_too_large_to_be_one_is_reported() {
    let mut buffer = built(2, &[], 0);
    put(
        buffer.as_mut_slice(),
        offset_of!(STORAGE_DEPENDENCY_INFO, Version),
        &u32::MAX.to_ne_bytes(),
    );
    let used = u32::try_from(buffer.len()).expect("small");

    assert!(matches!(
        decode(buffer.as_slice(), used),
        Err(StorageDependencyError::Malformed { .. })
    ));
}

#[test]
fn a_call_that_used_more_than_it_was_given_is_reported() {
    let buffer = built(2, &[], 0);
    let len = u32::try_from(buffer.len()).expect("small");

    assert!(decode(buffer.as_slice(), len).is_ok());
    for used in [len + 1, len + 4096, u32::MAX] {
        assert!(
            matches!(
                decode(buffer.as_slice(), used),
                Err(StorageDependencyError::Malformed { .. })
            ),
            "used {used} of {len}"
        );
    }
}

#[test]
fn a_reply_cut_short_inside_its_header_is_reported() {
    let buffer = built(2, &[], 0);

    for used in 0..FIRST_ENTRY {
        assert!(
            matches!(
                decode(buffer.as_slice(), u32::try_from(used).expect("small")),
                Err(StorageDependencyError::Malformed { .. })
            ),
            "used {used}"
        );
    }
}

#[test]
fn an_entry_cut_short_is_reported_wherever_it_is_cut() {
    // Version 1 has no names, so every shorter `used` cuts an entry or the count's room for it.
    let buffer = built(1, &[distinct(16)], 0);
    let end = FIRST_ENTRY + ENTRY_1;

    for used in FIRST_ENTRY..end {
        assert!(
            matches!(
                decode(buffer.as_slice(), u32::try_from(used).expect("small")),
                Err(StorageDependencyError::Malformed { .. })
            ),
            "used {used} of {end}"
        );
    }
    assert!(decode(buffer.as_slice(), u32::try_from(end).expect("small")).is_ok());
}

// --- The size protocol, against a fake device -----------------------------------------------

/// A device that holds `entries`, checks what it is asked, and answers as the documentation says:
/// a buffer too small is `status` with the size needed written, and otherwise the reply.
struct Device {
    version: i32,
    entries: Vec<Built>,
    status: u32,
    sizes: Vec<usize>,
}

impl Device {
    fn new(version: i32, entries: Vec<Built>, status: u32) -> Self {
        Self {
            version,
            entries,
            status,
            sizes: Vec::new(),
        }
    }

    fn ask(&mut self, buffer: &mut [u8], used: *mut u32) -> u32 {
        assert_eq!(buffer.as_ptr().addr() % ALIGNMENT, 0, "an aligned buffer");
        assert!(buffer.len() >= MIN_BYTES, "never a buffer the call refuses");
        let asked = i32::from_ne_bytes(buffer[..4].try_into().expect("four bytes"));
        assert_eq!(asked, self.version, "the version is set before the call");
        self.sizes.push(buffer.len());

        let needed = needed(self.version, &self.entries);
        // SAFETY: `used` is the `u32` the entry lent for this call.
        unsafe { *used = u32::try_from(needed).expect("small") };
        if buffer.len() < needed {
            return self.status;
        }
        fill(buffer, self.version, &self.entries);
        0
    }
}

#[test]
fn a_reply_that_fits_the_first_buffer_is_asked_for_once() {
    let mut device = Device::new(2, vec![distinct(20)], ERROR_INSUFFICIENT_BUFFER);

    let info = dependencies_via(FIRST_BYTES, StorageDependencyVersion::V2, |buffer, used| {
        device.ask(buffer, used)
    })
    .expect("a reply that fits");

    assert_eq!(device.sizes, [FIRST_BYTES]);
    assert_eq!(info.entries().len(), 1);
}

#[test]
fn a_reply_that_does_not_fit_is_asked_for_again_at_the_size_the_call_gave() {
    for status in [ERROR_INSUFFICIENT_BUFFER, ERROR_MORE_DATA] {
        let entries: Vec<Built> = (1..=3).map(distinct).collect();
        let needed = needed(2, &entries);
        let mut device = Device::new(2, entries, status);

        let info = dependencies_via(MIN_BYTES, StorageDependencyVersion::V2, |buffer, used| {
            device.ask(buffer, used)
        })
        .unwrap_or_else(|error| panic!("status {status}: {error}"));

        assert_eq!(device.sizes, [MIN_BYTES, needed], "status {status}");
        assert_eq!(info.entries().len(), 3);
        assert_eq!(
            info.entries()[2].dependent_volume_relative_path(),
            name(r"images\disk.vhdx").as_ref()
        );
    }
}

#[test]
fn a_first_size_below_the_structure_is_raised_for_every_size_under_it() {
    for first in [0, 1, 7, 8, MIN_BYTES - 1] {
        let mut device = Device::new(1, vec![], ERROR_INSUFFICIENT_BUFFER);

        dependencies_via(first, StorageDependencyVersion::V1, |buffer, used| {
            device.ask(buffer, used)
        })
        .unwrap_or_else(|error| panic!("a first size of {first}: {error}"));

        assert_eq!(device.sizes, [MIN_BYTES], "a first size of {first}");
    }
}

#[test]
fn a_call_that_names_a_size_that_is_not_larger_is_reported_and_not_repeated() {
    // Asking again at the same size would be asking the same question: the call has not said
    // what it needs, so the reply cannot be trusted to be coming.
    for named in [0_u32, 1, u32::try_from(MIN_BYTES).expect("small")] {
        let mut calls = 0;
        let result = dependencies_via(MIN_BYTES, StorageDependencyVersion::V2, |_, used| {
            calls += 1;
            // SAFETY: `used` is the `u32` the entry lent for this call.
            unsafe { *used = named };
            ERROR_INSUFFICIENT_BUFFER
        });

        assert!(
            matches!(result, Err(StorageDependencyError::Malformed { .. })),
            "named {named}: {result:?}"
        );
        assert_eq!(calls, 1, "named {named}");
    }
}

#[test]
fn a_size_that_keeps_growing_is_given_up_on_after_the_attempts_allowed() {
    let mut sizes = Vec::new();
    let result = dependencies_via(MIN_BYTES, StorageDependencyVersion::V2, |buffer, used| {
        sizes.push(buffer.len());
        // SAFETY: `used` is the `u32` the entry lent for this call.
        unsafe { *used = u32::try_from(buffer.len() + 8).expect("small") };
        ERROR_INSUFFICIENT_BUFFER
    });

    assert!(
        matches!(result, Err(StorageDependencyError::Unstable { attempts }) if attempts == MAX_ATTEMPTS),
        "{result:?}"
    );
    assert_eq!(sizes.len(), MAX_ATTEMPTS);
    assert!(sizes.windows(2).all(|pair| pair[1] == pair[0] + 8));
}

#[test]
fn any_other_status_is_carried_unaltered_and_is_not_asked_again() {
    for status in [
        NOT_A_VIRTUAL_DISK,
        ERROR_INVALID_LEVEL,
        ERROR_INVALID_PARAMETER,
        5,
        u32::MAX,
    ] {
        let mut calls = 0;
        let result = dependencies_via(FIRST_BYTES, StorageDependencyVersion::V2, |_, _| {
            calls += 1;
            status
        });

        let error = result.expect_err("a refusal");
        assert_eq!(win32_code(&error), status);
        assert_eq!(calls, 1, "status {status}");
    }
}

#[test]
fn a_buffer_that_cannot_be_allocated_is_out_of_memory_and_the_call_is_not_made() {
    let mut calls = 0;
    let result = dependencies_via(usize::MAX, StorageDependencyVersion::V2, |_, _| {
        calls += 1;
        0
    });

    let error = result.expect_err("no buffer of that size");
    assert_eq!(win32_code(&error), ERROR_NOT_ENOUGH_MEMORY);
    assert_eq!(calls, 0);
}

#[test]
fn the_version_asked_for_is_in_the_buffer_the_call_is_given() {
    for raw in [1, 2, 0, 7, -3] {
        let mut seen = None;
        let _ = dependencies_via(
            FIRST_BYTES,
            StorageDependencyVersion::from_raw(raw),
            |buffer, _| {
                seen = Some(i32::from_ne_bytes(
                    buffer[..4].try_into().expect("four bytes"),
                ));
                5
            },
        );
        assert_eq!(seen, Some(raw));
    }
}

#[test]
fn a_new_buffer_after_a_regrow_has_the_version_set_again() {
    let mut device = Device::new(
        1,
        vec![distinct(21), distinct(22), distinct(23)],
        ERROR_MORE_DATA,
    );

    // The fake checks the version on both attempts; a buffer reused or re-zeroed without it
    // would fail there.
    let info = dependencies_via(MIN_BYTES, StorageDependencyVersion::V1, |buffer, used| {
        device.ask(buffer, used)
    })
    .expect("a regrown reply");

    assert_eq!(device.sizes.len(), 2);
    assert_eq!(info.entries().len(), 3);
}

// --- Errors ---------------------------------------------------------------------------------

#[test]
fn the_errors_display_what_happened_and_chain_the_win32_one() {
    let win32 = StorageDependencyError::from(Win32Error::from_code(NOT_A_VIRTUAL_DISK));
    assert!(
        win32
            .to_string()
            .contains("GetStorageDependencyInformation")
    );
    let source = std::error::Error::source(&win32).expect("the Win32 error is the source");
    assert_eq!(
        source
            .downcast_ref::<Win32Error>()
            .map(|error| error.code()),
        Some(NOT_A_VIRTUAL_DISK)
    );

    let bad = StorageDependencyError::Malformed {
        what: "a reply that lies",
    };
    assert!(bad.to_string().contains("a reply that lies"));
    assert!(std::error::Error::source(&bad).is_none());

    let unstable = StorageDependencyError::Unstable { attempts: 4 };
    assert!(unstable.to_string().contains('4'));
    assert!(std::error::Error::source(&unstable).is_none());
}
