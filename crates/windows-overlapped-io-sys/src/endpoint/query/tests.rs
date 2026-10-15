// Copyright (c) 2026 Mike Grier
use std::io::Write;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use windows_sys::Win32::Foundation::{
    ERROR_INVALID_FUNCTION, ERROR_INVALID_PARAMETER, ERROR_NOACCESS,
};
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_READONLY, FILE_BASIC_INFO,
    FILE_DISPOSITION_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_FULL_DIR_INFO,
    FILE_REMOTE_PROTOCOL_INFO, FILE_STORAGE_INFO, FileDispositionInfo,
    FileFullDirectoryRestartInfo, FileRemoteProtocolInfo, GetFileInformationByHandleEx,
    SetFileInformationByHandle,
};

use super::remote_protocol_values::{
    FLAG_LOOPBACK, FLAG_OFFLINE, FLAG_PERSISTENT_HANDLE, PROTOCOL_SMB,
};
use super::storage_info_values::{
    FLAGS_ALIGNED_DEVICE as STORAGE_INFO_FLAGS_ALIGNED_DEVICE,
    FLAGS_PARTITION_ALIGNED_ON_DEVICE as STORAGE_INFO_FLAGS_PARTITION_ALIGNED_ON_DEVICE,
    OFFSET_UNKNOWN as STORAGE_INFO_OFFSET_UNKNOWN,
};
use super::{basic_info_from, remote_protocol_info_from, storage_info_from};
use crate::{AlignmentInfo, FileTime, Smb2Info, StandardInfo, UnassociatedEndpoint};

use windows_sys::Win32::System::Ioctl::FSCTL_SET_SPARSE;

fn temp_path(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "windows-overlapped-io-sys-standard-{tag}-{}.tmp",
        std::process::id()
    ))
}

fn open(path: &Path) -> UnassociatedEndpoint {
    UnassociatedEndpoint::open(path, true, false, 0).expect("open endpoint")
}

/// Mark `file` sparse, so extending it allocates nothing.
fn make_sparse(file: &std::fs::File) {
    let mut returned = 0_u32;
    // SAFETY: a live synchronous handle and no buffers, so nothing outlives the call.
    let ok = unsafe {
        windows_sys::Win32::System::IO::DeviceIoControl(
            file.as_raw_handle(),
            FSCTL_SET_SPARSE,
            std::ptr::null(),
            0,
            std::ptr::null_mut(),
            0,
            &raw mut returned,
            std::ptr::null_mut(),
        )
    };
    assert_ne!(
        ok,
        0,
        "FSCTL_SET_SPARSE: {}",
        std::io::Error::last_os_error()
    );
}

/// Request a delete through `file`'s handle, which takes effect when the last handle closes.
fn request_delete(file: &std::fs::File) {
    let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
    let size = u32::try_from(size_of::<FILE_DISPOSITION_INFO>()).expect("small struct");
    // SAFETY: a live handle opened with DELETE access, and an input buffer of exactly the size passed.
    let ok = unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileDispositionInfo,
            (&raw const disposition).cast(),
            size,
        )
    };
    assert_ne!(
        ok,
        0,
        "FileDispositionInfo: {}",
        std::io::Error::last_os_error()
    );
}

#[test]
fn an_ordinary_file_reports_its_length_one_link_and_no_flags() {
    let path = temp_path("ordinary");
    std::fs::write(&path, vec![b'a'; 65_536]).expect("write temp file");
    let endpoint = open(&path);
    let info = endpoint.observer().standard_info().expect("standard_info");

    assert_eq!(info.len, 65_536);
    assert!(info.allocation_size >= info.len, "{info:?}");
    assert_eq!(info.link_count, 1);
    assert!(!info.delete_pending);
    assert!(!info.is_directory);

    drop(endpoint);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn file_len_is_standard_info_len_across_sizes() {
    let path = temp_path("shortcut");
    let writer = std::fs::File::create(&path).expect("create temp file");
    let endpoint = open(&path);
    let observer = endpoint.observer();

    for size in [0_u64, 1, 511, 512, 4096, 4097, 1 << 20, 3] {
        writer.set_len(size).expect("set length");
        let info = observer.standard_info().expect("standard_info");
        assert_eq!(info.len, size);
        assert_eq!(observer.file_len().expect("file_len"), info.len);
    }

    drop((observer, endpoint, writer));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_sparse_file_reports_an_allocation_smaller_than_its_length() {
    let path = temp_path("sparse");
    let writer = std::fs::File::create(&path).expect("create temp file");
    make_sparse(&writer);
    writer.set_len(1 << 30).expect("extend to 1 GiB");
    let endpoint = open(&path);
    let info = endpoint.observer().standard_info().expect("standard_info");

    assert_eq!(info.len, 1 << 30);
    assert!(info.allocation_size < info.len, "{info:?}");

    drop((endpoint, writer));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn the_link_count_follows_hard_links_made_and_removed() {
    let path = temp_path("links");
    let second = temp_path("links-second");
    let third = temp_path("links-third");
    std::fs::write(&path, b"linked").expect("write temp file");
    let endpoint = open(&path);
    let observer = endpoint.observer();
    let links = || observer.standard_info().expect("standard_info").link_count;

    assert_eq!(links(), 1);
    std::fs::hard_link(&path, &second).expect("second link");
    assert_eq!(links(), 2);
    std::fs::hard_link(&path, &third).expect("third link");
    assert_eq!(links(), 3);
    std::fs::remove_file(&second).expect("remove second link");
    assert_eq!(links(), 2);
    std::fs::remove_file(&third).expect("remove third link");
    assert_eq!(links(), 1);

    drop((observer, endpoint));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_delete_requested_while_the_observer_holds_the_file_is_pending_until_it_lets_go() {
    let path = temp_path("delete");
    std::fs::write(&path, b"doomed").expect("write temp file");
    let endpoint = open(&path);
    let observer = endpoint.observer();
    drop(endpoint);
    assert!(
        !observer
            .standard_info()
            .expect("standard_info")
            .delete_pending
    );

    let deleter = std::fs::OpenOptions::new()
        .access_mode(DELETE)
        .open(&path)
        .expect("open for delete");
    request_delete(&deleter);
    drop(deleter);

    let info = observer
        .standard_info()
        .expect("standard_info after the request");
    assert!(info.delete_pending, "{info:?}");
    assert_eq!(
        info.len, 6,
        "a pending delete leaves the contents readable through the handle"
    );

    drop(observer);
    assert!(
        !path.exists(),
        "the last holder's close completes the delete"
    );
}

#[test]
fn a_directory_reports_itself_as_one() {
    let dir = temp_path("directory");
    std::fs::create_dir(&dir).expect("create directory");
    let endpoint = UnassociatedEndpoint::open(&dir, true, false, FILE_FLAG_BACKUP_SEMANTICS)
        .expect("open directory endpoint");
    let info = endpoint.observer().standard_info().expect("standard_info");

    assert!(info.is_directory, "{info:?}");
    assert!(!info.delete_pending);

    drop(endpoint);
    let _ = std::fs::remove_dir(&dir);
}

#[test]
fn every_observer_of_a_handle_reads_the_same_standard_info() {
    let path = temp_path("agree");
    let mut writer = std::fs::File::create(&path).expect("create temp file");
    writer.write_all(b"agreed").expect("write");
    let endpoint = open(&path);
    let first = endpoint.observer();
    let clone = first.clone();
    let held = endpoint.into_endpoint_handle();
    let late = held.observer();

    let expected: StandardInfo = first.standard_info().expect("standard_info");
    for observer in [&clone, &late] {
        assert_eq!(observer.standard_info().expect("standard_info"), expected);
    }

    drop((first, clone, late, held, writer));
    let _ = std::fs::remove_file(&path);
}

// --- file_id ---

#[test]
fn every_observer_of_a_handle_reads_the_same_file_id() {
    let path = temp_path("id-agree");
    std::fs::write(&path, b"one file").expect("write temp file");
    let endpoint = open(&path);
    let first = endpoint.observer();
    let clone = first.clone();
    let held = endpoint.into_endpoint_handle();
    let late = held.observer();

    let expected = first.file_id().expect("file_id");
    for observer in [&clone, &late] {
        assert_eq!(observer.file_id().expect("file_id"), expected);
    }

    drop((first, clone, late, held));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_second_open_of_the_same_path_reads_the_same_file_id() {
    let path = temp_path("id-reopen");
    std::fs::write(&path, b"reopened").expect("write temp file");
    let first = open(&path);
    let second = open(&path);

    assert_eq!(
        first.observer().file_id().expect("file_id"),
        second.observer().file_id().expect("file_id")
    );

    drop((first, second));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn files_in_one_directory_differ_in_id_and_share_a_volume() {
    let paths: Vec<PathBuf> = (0..4)
        .map(|i| temp_path(&format!("id-distinct-{i}")))
        .collect();
    let mut ids = Vec::new();
    for path in &paths {
        std::fs::write(path, b"distinct").expect("write temp file");
        let endpoint = open(path);
        ids.push(endpoint.observer().file_id().expect("file_id"));
    }

    for (i, a) in ids.iter().enumerate() {
        assert_eq!(a.volume_serial_number, ids[0].volume_serial_number);
        for b in &ids[i + 1..] {
            assert_ne!(a.file_id, b.file_id, "{a:?} {b:?}");
        }
    }

    for path in &paths {
        let _ = std::fs::remove_file(path);
    }
}

#[test]
fn a_rename_while_the_observer_holds_the_file_keeps_its_id() {
    let path = temp_path("id-rename");
    let renamed = temp_path("id-renamed");
    std::fs::write(&path, b"moving").expect("write temp file");
    let endpoint = open(&path);
    let observer = endpoint.observer();
    let before = observer.file_id().expect("file_id");

    std::fs::rename(&path, &renamed).expect("rename while held");
    assert_eq!(observer.file_id().expect("file_id after rename"), before);
    let reopened = open(&renamed);
    assert_eq!(reopened.observer().file_id().expect("file_id"), before);

    drop((observer, endpoint, reopened));
    let _ = std::fs::remove_file(&renamed);
}

#[test]
fn a_hard_link_reads_the_same_file_id() {
    let path = temp_path("id-link");
    let link = temp_path("id-link-second");
    std::fs::write(&path, b"linked").expect("write temp file");
    std::fs::hard_link(&path, &link).expect("hard link");

    let through_path = open(&path);
    let through_link = open(&link);
    assert_eq!(
        through_path.observer().file_id().expect("file_id"),
        through_link.observer().file_id().expect("file_id")
    );

    drop((through_path, through_link));
    let _ = std::fs::remove_file(&link);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_file_recreated_under_the_same_name_has_a_new_id() {
    let path = temp_path("id-recreate");
    std::fs::write(&path, b"first").expect("write temp file");
    let first = open(&path);
    let first_id = first.observer().file_id().expect("file_id");
    drop(first);

    std::fs::remove_file(&path).expect("remove");
    std::fs::write(&path, b"second").expect("recreate");
    let second = open(&path);
    let second_id = second.observer().file_id().expect("file_id");

    assert_eq!(
        second_id.volume_serial_number,
        first_id.volume_serial_number
    );
    assert_ne!(second_id.file_id, first_id.file_id);

    drop(second);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_directory_has_a_file_id_distinct_from_a_file_inside_it() {
    let dir = temp_path("id-directory");
    std::fs::create_dir(&dir).expect("create directory");
    let inner = dir.join("inner.tmp");
    std::fs::write(&inner, b"inside").expect("write inner file");

    let dir_endpoint = UnassociatedEndpoint::open(&dir, true, false, FILE_FLAG_BACKUP_SEMANTICS)
        .expect("open directory endpoint");
    let file_endpoint = open(&inner);
    let dir_id = dir_endpoint.observer().file_id().expect("file_id");
    let file_id = file_endpoint.observer().file_id().expect("file_id");

    assert_eq!(dir_id.volume_serial_number, file_id.volume_serial_number);
    assert_ne!(dir_id.file_id, file_id.file_id);

    drop((dir_endpoint, file_endpoint));
    let _ = std::fs::remove_file(&inner);
    let _ = std::fs::remove_dir(&dir);
}

/// The NUL device answers the standard query but not the file-ID one, which is
/// how the shared query's error path is reached.
#[test]
fn a_device_without_file_ids_reports_the_error() {
    let nul = UnassociatedEndpoint::open(r"\\.\NUL", true, false, 0).expect("open NUL");
    let observer = nul.observer();

    let error = observer
        .file_id()
        .expect_err("the NUL device has no file ID");
    assert_eq!(
        error.raw_os_error(),
        Some(ERROR_INVALID_PARAMETER as i32),
        "{error:?}"
    );
    observer
        .standard_info()
        .expect("the same handle still answers the standard query");
}

// --- storage_info and alignment_info ---

fn raw_storage(sector_offset: u32, partition_offset: u32, flags: u32) -> FILE_STORAGE_INFO {
    FILE_STORAGE_INFO {
        LogicalBytesPerSector: 512,
        PhysicalBytesPerSectorForAtomicity: 4096,
        PhysicalBytesPerSectorForPerformance: 4096,
        FileSystemEffectivePhysicalBytesPerSectorForAtomicity: 4096,
        Flags: flags,
        ByteOffsetForSectorAlignment: sector_offset,
        ByteOffsetForPartitionAlignment: partition_offset,
    }
}

#[test]
fn storage_info_carries_every_field_and_maps_the_unknown_offset_to_none() {
    let info = storage_info_from(&raw_storage(0, STORAGE_INFO_OFFSET_UNKNOWN, 0));
    assert_eq!(info.logical_bytes_per_sector, 512);
    assert_eq!(info.physical_bytes_per_sector_for_atomicity, 4096);
    assert_eq!(info.physical_bytes_per_sector_for_performance, 4096);
    assert_eq!(
        info.file_system_effective_physical_bytes_per_sector_for_atomicity,
        4096
    );
    assert_eq!(info.byte_offset_for_sector_alignment, Some(0));
    assert_eq!(info.byte_offset_for_partition_alignment, None);

    let swapped = storage_info_from(&raw_storage(STORAGE_INFO_OFFSET_UNKNOWN, 3584, 0));
    assert_eq!(swapped.byte_offset_for_sector_alignment, None);
    assert_eq!(swapped.byte_offset_for_partition_alignment, Some(3584));
}

#[test]
fn storage_flags_are_read_bit_by_bit_and_kept_as_reported() {
    for (flags, aligned, partition) in [
        (0, false, false),
        (STORAGE_INFO_FLAGS_ALIGNED_DEVICE, true, false),
        (STORAGE_INFO_FLAGS_PARTITION_ALIGNED_ON_DEVICE, false, true),
        (
            STORAGE_INFO_FLAGS_ALIGNED_DEVICE | STORAGE_INFO_FLAGS_PARTITION_ALIGNED_ON_DEVICE,
            true,
            true,
        ),
        (0x8000_0000, false, false),
    ] {
        let info = storage_info_from(&raw_storage(0, 0, flags));
        assert_eq!(info.flags, flags);
        assert_eq!(info.aligned_device(), aligned, "flags {flags:#x}");
        assert_eq!(
            info.partition_aligned_on_device(),
            partition,
            "flags {flags:#x}"
        );
    }
}

#[test]
fn an_alignment_mask_names_its_byte_alignment() {
    for (mask, bytes) in [
        (0_u32, 1_u64),
        (1, 2),
        (3, 4),
        (7, 8),
        (0x1FF, 512),
        (0xFFF, 4096),
        (0xFFFF, 65_536),
        (u32::MAX, 1 << 32),
    ] {
        let info = AlignmentInfo {
            requirement_mask: mask,
        };
        assert_eq!(info.byte_alignment(), Some(bytes), "mask {mask:#x}");
    }
}

#[test]
fn a_mask_that_is_not_one_less_than_a_power_of_two_names_no_alignment() {
    for mask in [2_u32, 5, 0x100, 0x1FE, 0x8000_0000] {
        assert_eq!(
            AlignmentInfo {
                requirement_mask: mask
            }
            .byte_alignment(),
            None,
            "mask {mask:#x}"
        );
    }
}

#[test]
fn an_address_is_aligned_when_it_clears_every_bit_of_the_mask() {
    let sector = AlignmentInfo {
        requirement_mask: 0x1FF,
    };
    for (address, aligned) in [
        (0, true),
        (512, true),
        (4096, true),
        (1, false),
        (511, false),
        (513, false),
    ] {
        assert_eq!(sector.is_aligned(address), aligned, "address {address:#x}");
    }
    let any = AlignmentInfo {
        requirement_mask: 0,
    };
    assert!(any.is_aligned(1) && any.is_aligned(0x1234_5677));
    // A mask that is not of the documented form still means "these bits clear".
    let odd = AlignmentInfo {
        requirement_mask: 0x100,
    };
    assert!(odd.is_aligned(0xFF) && !odd.is_aligned(0x100));
}

#[test]
fn a_file_reports_a_sector_geometry_consistent_with_itself() {
    let path = temp_path("storage");
    std::fs::write(&path, b"geometry").expect("write temp file");
    let endpoint = open(&path);
    let info = endpoint.observer().storage_info().expect("storage_info");

    let logical = info.logical_bytes_per_sector;
    assert!(logical.is_power_of_two(), "{info:?}");
    for physical in [
        info.physical_bytes_per_sector_for_atomicity,
        info.physical_bytes_per_sector_for_performance,
        info.file_system_effective_physical_bytes_per_sector_for_atomicity,
    ] {
        assert!(physical >= logical && physical % logical == 0, "{info:?}");
    }

    drop(endpoint);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_file_reports_an_alignment_of_the_documented_form() {
    let path = temp_path("alignment");
    std::fs::write(&path, b"aligned").expect("write temp file");
    let endpoint = open(&path);
    let info = endpoint
        .observer()
        .alignment_info()
        .expect("alignment_info");

    let bytes = info.byte_alignment().expect("one less than a power of two");
    assert!(
        info.is_aligned(0) && info.is_aligned(bytes as usize),
        "{info:?}"
    );

    drop(endpoint);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn files_on_one_volume_and_every_observer_agree_on_storage_and_alignment() {
    let first_path = temp_path("geometry-first");
    let second_path = temp_path("geometry-second");
    std::fs::write(&first_path, b"one").expect("write temp file");
    std::fs::write(&second_path, b"two").expect("write temp file");
    let first = open(&first_path);
    let second = open(&second_path);
    let observer = first.observer();
    let clone = observer.clone();

    let storage = observer.storage_info().expect("storage_info");
    let alignment = observer.alignment_info().expect("alignment_info");
    for other in [clone, second.observer()] {
        assert_eq!(other.storage_info().expect("storage_info"), storage);
        assert_eq!(other.alignment_info().expect("alignment_info"), alignment);
    }

    drop((observer, first, second));
    let _ = std::fs::remove_file(&first_path);
    let _ = std::fs::remove_file(&second_path);
}

/// Measured: the NUL device refuses the storage query but answers the alignment
/// query, with no requirement at all.
#[test]
fn the_nul_device_has_no_storage_geometry_and_needs_no_alignment() {
    let nul = UnassociatedEndpoint::open(r"\\.\NUL", true, false, 0).expect("open NUL");
    let observer = nul.observer();

    let error = observer
        .storage_info()
        .expect_err("the NUL device has no sectors");
    assert_eq!(
        error.raw_os_error(),
        Some(ERROR_INVALID_FUNCTION as i32),
        "{error:?}"
    );
    let alignment = observer.alignment_info().expect("alignment_info");
    assert_eq!(alignment.requirement_mask, 0);
    assert_eq!(alignment.byte_alignment(), Some(1));
}

#[test]
fn a_directory_reports_its_volumes_geometry() {
    let dir = temp_path("geometry-dir");
    std::fs::create_dir(&dir).expect("create directory");
    let file_path = dir.join("inner.tmp");
    std::fs::write(&file_path, b"inside").expect("write inner file");
    let dir_endpoint = UnassociatedEndpoint::open(&dir, true, false, FILE_FLAG_BACKUP_SEMANTICS)
        .expect("open directory endpoint");
    let file_endpoint = open(&file_path);

    let from_dir = dir_endpoint.observer();
    let from_file = file_endpoint.observer();
    assert_eq!(
        from_dir.storage_info().expect("storage_info"),
        from_file.storage_info().expect("storage_info")
    );
    assert_eq!(
        from_dir.alignment_info().expect("alignment_info"),
        from_file.alignment_info().expect("alignment_info")
    );

    drop((from_dir, from_file, dir_endpoint, file_endpoint));
    let _ = std::fs::remove_file(&file_path);
    let _ = std::fs::remove_dir(&dir);
}

// --- remote_protocol_info ---

fn raw_remote(protocol: u32, flags: u32) -> FILE_REMOTE_PROTOCOL_INFO {
    let mut raw = FILE_REMOTE_PROTOCOL_INFO {
        StructureVersion: 4,
        StructureSize: 116,
        Protocol: protocol,
        ProtocolMajorVersion: 3,
        ProtocolMinorVersion: 1,
        ProtocolRevision: 1,
        Flags: flags,
        ..FILE_REMOTE_PROTOCOL_INFO::default()
    };
    raw.ProtocolSpecific.Smb2.Server.Capabilities = 0x6F;
    raw.ProtocolSpecific.Smb2.Share.Capabilities = 0x80;
    raw.ProtocolSpecific.Smb2.Share.ShareFlags = 0x33;
    raw
}

#[test]
fn remote_protocol_info_carries_every_field_and_the_smb2_block_for_smb() {
    let info = remote_protocol_info_from(&raw_remote(PROTOCOL_SMB, 0));
    assert_eq!(info.structure_version, 4);
    assert_eq!(info.protocol, PROTOCOL_SMB);
    assert_eq!(
        (info.major_version, info.minor_version, info.revision),
        (3, 1, 1)
    );
    assert_eq!(
        info.smb2,
        Some(Smb2Info {
            server_capabilities: 0x6F,
            share_capabilities: 0x80,
            share_flags: 0x33,
        })
    );
}

#[test]
fn a_protocol_other_than_smb_carries_no_smb2_block() {
    for protocol in [0_u32, 0x0001_0000, 0x0002_0001, 0x0025_0000, u32::MAX] {
        let info = remote_protocol_info_from(&raw_remote(protocol, 0));
        assert_eq!(info.protocol, protocol);
        assert_eq!(info.smb2, None, "protocol {protocol:#x}");
    }
}

#[test]
fn remote_flags_are_read_bit_by_bit_and_kept_as_reported() {
    for (flags, loopback, offline, persistent) in [
        (0, false, false, false),
        (FLAG_LOOPBACK, true, false, false),
        (FLAG_OFFLINE, false, true, false),
        (FLAG_PERSISTENT_HANDLE, false, false, true),
        (
            FLAG_LOOPBACK | FLAG_OFFLINE | FLAG_PERSISTENT_HANDLE,
            true,
            true,
            true,
        ),
        (0x38, false, false, false),
        (0x39, true, false, false),
    ] {
        let info = remote_protocol_info_from(&raw_remote(PROTOCOL_SMB, flags));
        assert_eq!(info.flags, flags);
        assert_eq!(info.loopback(), loopback, "flags {flags:#x}");
        assert_eq!(info.offline(), offline, "flags {flags:#x}");
        assert_eq!(info.persistent_handle(), persistent, "flags {flags:#x}");
    }
}

/// Measured: a local file is refused, rather than answered with an empty protocol.
#[test]
fn a_local_file_is_refused_with_invalid_parameter() {
    let path = temp_path("remote-local");
    std::fs::write(&path, b"local").expect("write temp file");
    let endpoint = open(&path);

    let error = endpoint
        .observer()
        .remote_protocol_info()
        .expect_err("a local file has no remote protocol");
    assert_eq!(
        error.raw_os_error(),
        Some(ERROR_INVALID_PARAMETER as i32),
        "{error:?}"
    );

    drop(endpoint);
    let _ = std::fs::remove_file(&path);
}

/// The temp directory reached through its drive's administrative share, so the
/// handle is on SMB over loopback. Needs that share: administrator rights and
/// the Server service. Run it with `--include-ignored` where both are present.
fn loopback_share_path(path: &Path) -> String {
    let full = std::fs::canonicalize(path).expect("canonicalize");
    let full = full.to_str().expect("a UTF-8 temp path");
    let local = full.trim_start_matches(r"\\?\");
    let (drive, rest) = local.split_once(":\\").expect("a drive-letter path");
    format!(r"\\localhost\{drive}$\{rest}")
}

#[test]
#[ignore = "needs the administrative share (admin rights and the Server service)"]
fn a_file_on_the_loopback_administrative_share_is_smb_over_loopback() {
    let path = temp_path("remote-share");
    std::fs::write(&path, b"remote").expect("write temp file");
    let unc = loopback_share_path(&path);

    let endpoint =
        UnassociatedEndpoint::open(&unc, true, false, 0).expect("open through the share");
    let info = endpoint
        .observer()
        .remote_protocol_info()
        .expect("remote_protocol_info");

    assert_eq!(info.protocol, PROTOCOL_SMB, "{info:?}");
    assert!(info.loopback(), "{info:?}");
    assert!(info.smb2.is_some(), "{info:?}");

    drop(endpoint);
    let _ = std::fs::remove_file(&path);
}

/// Why every query's buffer is aligned to 8: the same remote-protocol query through
/// a buffer aligned to 4 but not 8 is refused with `ERROR_NOACCESS`, while the
/// observer's own query succeeds. Needs the administrative share, as above.
#[test]
#[ignore = "needs the administrative share (admin rights and the Server service)"]
fn the_remote_protocol_query_refuses_a_buffer_aligned_to_four_but_not_eight() {
    let path = temp_path("remote-align");
    std::fs::write(&path, b"remote").expect("write temp file");
    let unc = loopback_share_path(&path);
    let endpoint =
        UnassociatedEndpoint::open(&unc, true, false, 0).expect("open through the share");
    let observer = endpoint.observer();

    let mut storage = [0_u64; 17];
    let size = size_of::<FILE_REMOTE_PROTOCOL_INFO>();
    assert!(4 + size <= size_of_val(&storage));
    // SAFETY: 4 bytes into an 8-aligned array of 136 bytes, leaving room for the
    // structure; the call is synchronous and the handle is the observer's own.
    let ok = unsafe {
        let misaligned = storage.as_mut_ptr().cast::<u8>().add(4);
        GetFileInformationByHandleEx(
            observer.handle().as_raw_handle(),
            FileRemoteProtocolInfo,
            misaligned.cast(),
            u32::try_from(size).expect("a u32 size"),
        )
    };
    let error = std::io::Error::last_os_error();
    assert_eq!(ok, 0, "a buffer aligned to 4 was accepted");
    assert_eq!(
        error.raw_os_error(),
        Some(ERROR_NOACCESS as i32),
        "{error:?}"
    );

    observer
        .remote_protocol_info()
        .expect("the observer's own aligned query");

    drop((observer, endpoint));
    let _ = std::fs::remove_file(&path);
}

// --- basic_info ---

/// The file time of 1970-01-01 00:00 UTC, as Win32 documents it; written out
/// rather than derived, so it checks the conversion's own constants.
const UNIX_EPOCH_TICKS: i64 = 116_444_736_000_000_000;

#[test]
fn basic_info_carries_every_field() {
    let raw = FILE_BASIC_INFO {
        CreationTime: 1,
        LastAccessTime: 2,
        LastWriteTime: 3,
        ChangeTime: 4,
        FileAttributes: 0x8020,
    };
    let info = basic_info_from(&raw);
    assert_eq!(info.creation_time, FileTime { ticks: 1 });
    assert_eq!(info.last_access_time, FileTime { ticks: 2 });
    assert_eq!(info.last_write_time, FileTime { ticks: 3 });
    assert_eq!(info.change_time, FileTime { ticks: 4 });
    assert_eq!(info.attributes, 0x8020);
}

#[test]
fn a_file_time_converts_to_the_system_time_it_names() {
    for (ticks, since_unix_epoch) in [
        (UNIX_EPOCH_TICKS, Duration::ZERO),
        (UNIX_EPOCH_TICKS + 1, Duration::from_nanos(100)),
        (UNIX_EPOCH_TICKS + 15_000_000, Duration::from_millis(1500)),
        (
            UNIX_EPOCH_TICKS + 9_999_999,
            Duration::from_nanos(999_999_900),
        ),
        // 2000-01-01 00:00 UTC.
        (125_911_584_000_000_000, Duration::from_secs(946_684_800)),
    ] {
        assert_eq!(
            FileTime { ticks }.to_system_time(),
            Some(SystemTime::UNIX_EPOCH + since_unix_epoch),
            "ticks {ticks}"
        );
    }
}

#[test]
fn a_file_time_before_1970_converts_too() {
    for (ticks, before_unix_epoch) in [
        (0, Duration::from_secs(11_644_473_600)),
        (UNIX_EPOCH_TICKS - 1, Duration::from_nanos(100)),
    ] {
        assert_eq!(
            FileTime { ticks }.to_system_time(),
            Some(SystemTime::UNIX_EPOCH - before_unix_epoch),
            "ticks {ticks}"
        );
    }
}

#[test]
fn a_negative_file_time_has_no_system_time() {
    for ticks in [-1, -UNIX_EPOCH_TICKS, i64::MIN] {
        assert_eq!(FileTime { ticks }.to_system_time(), None, "ticks {ticks}");
    }
}

#[test]
fn the_largest_file_time_converts() {
    let largest = FileTime { ticks: i64::MAX }
        .to_system_time()
        .expect("the largest count");
    let ticks_past_unix_epoch = (i64::MAX - UNIX_EPOCH_TICKS).cast_unsigned();
    assert_eq!(
        largest,
        SystemTime::UNIX_EPOCH
            + Duration::from_secs(ticks_past_unix_epoch / 10_000_000)
            + Duration::from_nanos(ticks_past_unix_epoch % 10_000_000 * 100)
    );
}

#[test]
fn file_times_order_by_their_ticks() {
    let times = [i64::MIN, -1, 0, 1, UNIX_EPOCH_TICKS, i64::MAX].map(|ticks| FileTime { ticks });
    assert!(times.windows(2).all(|pair| pair[0] < pair[1]));
}

#[test]
fn a_new_file_reports_times_near_now_and_is_not_a_directory() {
    let before = SystemTime::now() - Duration::from_secs(60);
    let path = temp_path("basic-new");
    std::fs::write(&path, b"basic").expect("write temp file");
    let endpoint = open(&path);
    let info = endpoint.observer().basic_info().expect("basic_info");
    let after = SystemTime::now() + Duration::from_secs(60);

    for time in [
        info.creation_time,
        info.last_access_time,
        info.last_write_time,
        info.change_time,
    ] {
        let system = time.to_system_time().expect("a time after 1601");
        assert!(before < system && system < after, "{info:?}");
    }
    assert!(info.creation_time <= info.last_write_time, "{info:?}");
    assert_eq!(info.attributes & FILE_ATTRIBUTE_DIRECTORY, 0, "{info:?}");

    drop(endpoint);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_directory_reports_the_directory_attribute() {
    let dir = temp_path("basic-dir");
    std::fs::create_dir(&dir).expect("create dir");
    let endpoint = UnassociatedEndpoint::open(&dir, true, false, FILE_FLAG_BACKUP_SEMANTICS)
        .expect("open directory");
    let info = endpoint.observer().basic_info().expect("basic_info");
    assert_ne!(info.attributes & FILE_ATTRIBUTE_DIRECTORY, 0, "{info:?}");
    drop(endpoint);
    let _ = std::fs::remove_dir(&dir);
}

#[test]
fn the_read_only_attribute_follows_changes_made_by_path() {
    let path = temp_path("basic-readonly");
    std::fs::write(&path, b"basic").expect("write temp file");
    let endpoint = open(&path);
    let observer = endpoint.observer();
    let set_read_only = |read_only: bool| {
        let mut permissions = std::fs::metadata(&path).expect("metadata").permissions();
        permissions.set_readonly(read_only);
        std::fs::set_permissions(&path, permissions).expect("set_permissions");
    };

    for read_only in [true, false, true, false] {
        set_read_only(read_only);
        let info = observer.basic_info().expect("basic_info");
        assert_eq!(
            info.attributes & FILE_ATTRIBUTE_READONLY != 0,
            read_only,
            "{info:?}"
        );
    }

    drop((observer, endpoint));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn every_observer_of_a_handle_reads_the_same_basic_info() {
    let path = temp_path("basic-shared");
    std::fs::write(&path, b"basic").expect("write temp file");
    let endpoint = open(&path);
    let first = endpoint.observer();
    let second = first.clone();
    assert_eq!(
        first.basic_info().expect("first"),
        second.basic_info().expect("second")
    );
    drop((first, second, endpoint));
    let _ = std::fs::remove_file(&path);
}

/// Measured: the NUL device refuses the basic query.
#[test]
fn the_nul_device_refuses_the_basic_query() {
    let nul = UnassociatedEndpoint::open(r"\\.\NUL", true, false, 0).expect("open NUL");
    let error = nul
        .observer()
        .basic_info()
        .expect_err("the NUL device has no times");
    assert_eq!(
        error.raw_os_error(),
        Some(ERROR_INVALID_PARAMETER as i32),
        "{error:?}"
    );
}

/// The times and length in `name`'s directory entry, read by enumerating `dir`.
fn directory_entry(dir: &Path, name: &str) -> (FileTime, FileTime, i64) {
    let dir_handle = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(dir)
        .expect("open directory");
    let mut buffer = vec![0_u64; 8192];
    let size = u32::try_from(size_of_val(buffer.as_slice())).expect("a u32 size");
    // SAFETY: a live directory handle and an 8-aligned buffer of exactly the size
    // passed; the call is synchronous.
    let ok = unsafe {
        GetFileInformationByHandleEx(
            dir_handle.as_raw_handle(),
            FileFullDirectoryRestartInfo,
            buffer.as_mut_ptr().cast(),
            size,
        )
    };
    assert_ne!(ok, 0, "enumerate: {}", std::io::Error::last_os_error());
    let base = buffer.as_ptr().cast::<u8>();
    let mut offset = 0_usize;
    loop {
        // SAFETY: the call filled the buffer with a chain of entries, each
        // 8-aligned and starting at the previous one's `NextEntryOffset`.
        let entry = unsafe { &*base.add(offset).cast::<FILE_FULL_DIR_INFO>() };
        let length = entry.FileNameLength as usize / size_of::<u16>();
        // SAFETY: the entry's name runs `FileNameLength` bytes from `FileName`.
        let entry_name = unsafe { std::slice::from_raw_parts(entry.FileName.as_ptr(), length) };
        if String::from_utf16_lossy(entry_name) == name {
            return (
                FileTime {
                    ticks: entry.LastWriteTime,
                },
                FileTime {
                    ticks: entry.ChangeTime,
                },
                entry.EndOfFile,
            );
        }
        assert_ne!(entry.NextEntryOffset, 0, "{name} is not in {dir:?}");
        offset += entry.NextEntryOffset as usize;
    }
}

/// The measurement `basic_info`'s documentation reports: a write through another
/// handle shows here at once, while the directory entry keeps the earlier times
/// until that handle closes.
#[test]
fn a_write_elsewhere_shows_here_at_once_and_in_the_directory_entry_at_close() {
    let dir = temp_path("basic-lag");
    std::fs::create_dir(&dir).expect("create dir");
    let name = "file.txt";
    let path = dir.join(name);
    std::fs::write(&path, b"a").expect("write temp file");
    let endpoint = open(&path);
    let observer = endpoint.observer();
    let before = observer.basic_info().expect("before");
    // Not compared with `before`: the entry's change time was measured lagging the
    // handle's by under a millisecond right after the create.
    let entry_before = directory_entry(&dir, name);
    assert_eq!(entry_before.2, 1);

    let mut writer = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("open writer");
    // Past the system clock's tick, so the write gets a later time.
    std::thread::sleep(Duration::from_millis(40));
    writer.write_all(b"b").expect("write");

    let during = observer.basic_info().expect("during");
    assert!(
        during.last_write_time > before.last_write_time,
        "{during:?}"
    );
    assert!(during.change_time > before.change_time, "{during:?}");
    assert_eq!(
        directory_entry(&dir, name),
        entry_before,
        "the directory entry already reflects the write"
    );

    drop(writer);
    let after = observer.basic_info().expect("after");
    assert_eq!(after.last_write_time, during.last_write_time);
    let entry_after = directory_entry(&dir, name);
    assert_eq!(entry_after.0, during.last_write_time, "{entry_after:?}");
    assert!(entry_after.1 > entry_before.1, "{entry_after:?}");
    assert_eq!(entry_after.2, 2);

    drop((observer, endpoint));
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir(&dir);
}
