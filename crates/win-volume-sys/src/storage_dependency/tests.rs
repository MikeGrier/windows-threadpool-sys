// Copyright (c) 2026 Mike Grier
//! Tests for the dependency query.
//!
//! What a machine with no attached virtual disk and an unelevated token can show is the call's
//! refusals and its empty answer: no entry was ever observed, because attaching one needs an
//! elevated token. So the entries, the names, and the growth on `ERROR_INSUFFICIENT_BUFFER` are
//! tested against replies built here, laid out as the structure is declared (`reply`), and against a
//! fake device that checks what it was asked (`device`). Whether a real entry's names lie inside the
//! buffer the caller gave is the part nothing here can show.

use std::fs::{File, OpenOptions};
use std::io;
use std::mem::offset_of;
use std::os::windows::fs::OpenOptionsExt;

use io_buffers::AlignedBuf;
use win_error_codes::Win32Error;
use windows_sys::Win32::Foundation::{
    ERROR_INVALID_LEVEL, ERROR_INVALID_PARAMETER, ERROR_VIRTDISK_NOT_VIRTUAL_DISK,
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
    ALIGNMENT, DependentDiskFlags, FIRST_BYTES, MAX_ATTEMPTS, MIN_BYTES, StorageDependencyFlags,
    StorageDependencyInfo, StorageDependencyVersion, decode, dependencies_via,
    storage_dependency_information, storage_dependency_information_with_capacity,
};
use crate::test_support::{open_volume, system_disk, system_volume_path};

mod device;
mod reply;

/// What a handle that is not on a virtual disk gets for `HOST_VOLUMES`.
const NOT_A_VIRTUAL_DISK: u32 = ERROR_VIRTDISK_NOT_VIRTUAL_DISK;

const FIRST_ENTRY: usize = offset_of!(STORAGE_DEPENDENCY_INFO, Anonymous);
const ENTRY_1: usize = size_of::<STORAGE_DEPENDENCY_INFO_TYPE_1>();
const ENTRY_2: usize = size_of::<STORAGE_DEPENDENCY_INFO_TYPE_2>();

/// The system volume's root directory, which the call also takes.
fn system_root() -> File {
    OpenOptions::new()
        .access_mode(0)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(format!(
            r"{}\",
            std::env::var("SystemDrive").unwrap_or_else(|_| "C:".to_owned())
        ))
        .expect("open the system volume's root with no access")
}

/// The code an error carries, which every error from the call has.
fn win32_code(error: &io::Error) -> u32 {
    Win32Error::from_io(error)
        .unwrap_or_else(|| panic!("an error with no OS code: {error}"))
        .code()
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

fn decode_built(version: i32, entries: &[Built]) -> io::Result<StorageDependencyInfo> {
    let buffer = built(version, entries, 0);
    decode(
        buffer.as_slice(),
        u32::try_from(buffer.len()).expect("small"),
    )
}

/// Asserts that `result` is a reply refused as `InvalidData` whose reason contains `reason`, so
/// that a guard is shown to be the one that refused and not a later one that happens to refuse the
/// same bytes.
#[track_caller]
fn assert_malformed(result: io::Result<StorageDependencyInfo>, reason: &str) {
    match result {
        Err(error) if error.kind() == io::ErrorKind::InvalidData => {
            let what = error.to_string();
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
    let volume = open_volume(&system_volume_path(), false);

    for version in [StorageDependencyVersion::V1, StorageDependencyVersion::V2] {
        let info = storage_dependency_information(&volume, StorageDependencyFlags::NONE, version)
            .unwrap_or_else(|error| panic!("{version:?}: {error}"));

        assert_eq!(info.version(), version, "the version is echoed back");
        assert!(info.entries().is_empty(), "{info:?}");
    }
}

#[test]
fn a_root_directory_and_a_disk_answer_as_a_volume_does() {
    let ask = |handle: &File| {
        storage_dependency_information(
            handle,
            StorageDependencyFlags::NONE,
            StorageDependencyVersion::V2,
        )
    };

    let root = ask(&system_root()).expect("a root directory");
    let disk = ask(&system_disk(false)).expect("a disk");

    assert!(root.entries().is_empty(), "{root:?}");
    assert!(disk.entries().is_empty(), "{disk:?}");
}

#[test]
fn an_overlapped_handle_gets_the_same_answer_as_a_plain_one() {
    let ask = |overlapped| {
        storage_dependency_information(
            &open_volume(&system_volume_path(), overlapped),
            StorageDependencyFlags::NONE,
            StorageDependencyVersion::V2,
        )
        .expect("query")
    };

    assert_eq!(ask(false), ask(true));
}

#[test]
fn a_first_buffer_below_the_structure_is_raised_and_never_asked() {
    // Measured: anything under the structure's own size is ERROR_INVALID_PARAMETER and writes
    // nothing, so a first size configured smaller must still succeed.
    let volume = open_volume(&system_volume_path(), false);

    for bytes in [0, 1, 8, MIN_BYTES - 1, MIN_BYTES, MIN_BYTES + 1] {
        let info = storage_dependency_information_with_capacity(
            &volume,
            StorageDependencyFlags::NONE,
            StorageDependencyVersion::V2,
            bytes,
        )
        .unwrap_or_else(|error| panic!("a first buffer of {bytes} bytes: {error}"));
        assert!(info.entries().is_empty());
    }
}

#[test]
fn a_version_the_device_does_not_have_is_the_devices_to_refuse() {
    let volume = open_volume(&system_volume_path(), false);

    for raw in [0, 3, 4, -1, i32::MAX] {
        let error = storage_dependency_information(
            &volume,
            StorageDependencyFlags::NONE,
            StorageDependencyVersion::from_raw(raw),
        )
        .expect_err("a version that is not 1 or 2");
        assert_eq!(win32_code(&error), ERROR_INVALID_LEVEL, "version {raw}");
    }
}

#[test]
fn the_host_direction_on_a_handle_that_is_not_virtual_is_the_raw_status() {
    // A system volume that is itself on a virtual disk answers with its host instead, which is the
    // other thing this call is for.
    let volume = open_volume(&system_volume_path(), false);

    match storage_dependency_information(
        &volume,
        StorageDependencyFlags::HOST_VOLUMES,
        StorageDependencyVersion::V2,
    ) {
        Err(error) => assert_eq!(win32_code(&error), NOT_A_VIRTUAL_DISK, "{error}"),
        Ok(info) => {
            eprintln!("the system volume is virtual: {info:?}");
            assert!(!info.entries().is_empty());
        }
    }
}

#[test]
fn the_disk_handle_flag_is_the_devices_to_answer_on_a_volume() {
    let volume = open_volume(&system_volume_path(), false);

    let error = storage_dependency_information(
        &volume,
        StorageDependencyFlags::DISK_HANDLE,
        StorageDependencyVersion::V2,
    )
    .expect_err("a volume is not a disk");
    assert_eq!(win32_code(&error), ERROR_INVALID_PARAMETER, "{error}");
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
