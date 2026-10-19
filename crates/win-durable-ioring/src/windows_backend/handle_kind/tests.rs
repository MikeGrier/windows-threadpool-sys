// Copyright (c) 2026 Mike Grier
//! Tests for `HandleKindQuery`.
//!
//! The classification is tested over every answer the three queries can give, with the controls
//! replaced by closures that say what they were asked, and then over real handles: a file, a
//! directory, `NUL`, a named pipe, the system volume and the disk it is on. A volume and a disk are
//! opened asking for no access, which their device controls need none of, so no test needs elevation.
//! What a real handle answers is what was observed on the machine this was written on.

use std::cell::Cell;
use std::io;
use std::os::windows::io::{AsHandle, OwnedHandle};

use win_pipes_sys::{NamedPipeOptions, PipeMode, PipeOpenMode, create_named_pipe};
use win_volume_sys::{DiskExtent, StorageDeviceNumber};
use windows_namespace_request_sys::{FileType, OpenFile};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use wtf_string::Wtf16String;

use super::{HandleKind, HandleKindQuery, answered, classify};
use crate::windows_backend::{Cached, FileOptions, OpenedFile};

const GENERIC_READ: u32 = 0x8000_0000;
const NO_ACCESS: u32 = 0;

fn number(partition: u32) -> StorageDeviceNumber {
    StorageDeviceNumber {
        device_type: 7,
        device_number: 3,
        partition_number: partition,
    }
}

fn extent() -> DiskExtent {
    DiskExtent {
        disk_number: 3,
        starting_offset: 1 << 20,
        length: 1 << 30,
    }
}

/// `classify` with the two controls counted, so a test can say which were asked.
fn classified(
    file_type: FileType,
    device: io::Result<Option<StorageDeviceNumber>>,
    extents: io::Result<Option<Vec<DiskExtent>>>,
) -> (io::Result<HandleKind>, u32, u32) {
    let (asked_device, asked_extents) = (Cell::new(0), Cell::new(0));
    let kind = classify(
        file_type,
        || {
            asked_device.set(asked_device.get() + 1);
            device
        },
        || {
            asked_extents.set(asked_extents.get() + 1);
            extents
        },
    );
    (kind, asked_device.get(), asked_extents.get())
}

fn out_of_memory() -> io::Error {
    io::Error::from(io::ErrorKind::OutOfMemory)
}

// --- The classification -----------------------------------------------------------------------------

#[test]
fn a_character_device_is_one_and_neither_control_is_asked() {
    let (kind, device, extents) = classified(FileType::CHAR, Ok(Some(number(1))), Ok(Some(vec![])));
    assert_eq!(kind.expect("a kind"), HandleKind::CharacterDevice);
    assert_eq!((device, extents), (0, 0));
}

#[test]
fn a_pipe_is_one_and_neither_control_is_asked() {
    let (kind, device, extents) = classified(FileType::PIPE, Ok(Some(number(1))), Ok(Some(vec![])));
    assert_eq!(kind.expect("a kind"), HandleKind::Pipe);
    assert_eq!((device, extents), (0, 0));
}

#[test]
fn another_type_is_carried_as_it_came_and_neither_control_is_asked() {
    for other in [
        FileType::REMOTE,
        FileType::UNKNOWN,
        FileType::from_raw(0x1234),
    ] {
        let (kind, device, extents) = classified(other, Ok(Some(number(1))), Ok(None));
        assert_eq!(kind.expect("a kind"), HandleKind::Other(other));
        assert_eq!((device, extents), (0, 0), "{other:?}");
    }
}

#[test]
fn a_disk_the_device_control_refused_is_a_disk_file_and_the_extents_are_not_asked() {
    let (kind, device, extents) = classified(FileType::DISK, Ok(None), Ok(Some(vec![extent()])));
    assert_eq!(kind.expect("a kind"), HandleKind::DiskFile);
    assert_eq!((device, extents), (1, 0));
}

#[test]
fn a_disk_both_controls_answered_is_a_volume_with_what_they_said() {
    let extents = vec![
        extent(),
        DiskExtent {
            disk_number: 4,
            ..extent()
        },
    ];
    let (kind, device, asked) = classified(
        FileType::DISK,
        Ok(Some(number(2))),
        Ok(Some(extents.clone())),
    );
    assert_eq!(
        kind.expect("a kind"),
        HandleKind::Volume {
            number: number(2),
            extents
        }
    );
    assert_eq!((device, asked), (1, 1));
}

#[test]
fn a_disk_the_extents_control_refused_is_a_physical_disk_with_its_number() {
    let (kind, device, extents) = classified(FileType::DISK, Ok(Some(number(0))), Ok(None));
    assert_eq!(kind.expect("a kind"), HandleKind::PhysicalDisk(number(0)));
    assert_eq!((device, extents), (1, 1));
}

#[test]
fn a_volume_with_no_extents_is_still_a_volume() {
    let (kind, _, _) = classified(FileType::DISK, Ok(Some(number(1))), Ok(Some(vec![])));
    assert_eq!(
        kind.expect("a kind"),
        HandleKind::Volume {
            number: number(1),
            extents: vec![]
        }
    );
}

#[test]
fn the_numbers_a_device_wrote_are_carried_as_it_wrote_them() {
    let mpio = StorageDeviceNumber {
        device_type: 7,
        device_number: 0xffff_ffff,
        partition_number: 0xffff_ffff,
    };
    let (kind, _, _) = classified(FileType::DISK, Ok(Some(mpio)), Ok(None));
    assert_eq!(kind.expect("a kind"), HandleKind::PhysicalDisk(mpio));
}

#[test]
fn an_error_from_either_control_that_is_not_an_answer_is_passed_on() {
    let (kind, device, extents) = classified(FileType::DISK, Err(out_of_memory()), Ok(None));
    assert_eq!(
        kind.expect_err("passed on").kind(),
        io::ErrorKind::OutOfMemory
    );
    assert_eq!((device, extents), (1, 0));

    let (kind, device, extents) =
        classified(FileType::DISK, Ok(Some(number(1))), Err(out_of_memory()));
    assert_eq!(
        kind.expect_err("passed on").kind(),
        io::ErrorKind::OutOfMemory
    );
    assert_eq!((device, extents), (1, 1));
}

#[test]
fn a_refusal_is_an_answer_whatever_its_code_and_out_of_memory_is_not() {
    assert_eq!(answered(Ok(5)).expect("an answer"), Some(5));
    for code in [1, 5, 6, 31, 87, 122, 1117] {
        let refused = answered::<u32>(Err(io::Error::from_raw_os_error(code)));
        assert_eq!(
            refused.expect("a refusal is an answer"),
            None,
            "code {code}"
        );
    }
    assert!(answered::<u32>(Err(out_of_memory())).is_err());
}

// --- Real handles ------------------------------------------------------------------------------------

fn open(path: &str, access: u32, flags: u32) -> OwnedHandle {
    OpenFile::new(Wtf16String::from(path))
        .expect("the path has no NUL")
        .with_desired_access(access)
        .with_share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .with_creation_disposition(OPEN_EXISTING)
        .with_flags_and_attributes(flags)
        .perform()
        .unwrap_or_else(|error| panic!("open {path}: {error:?}"))
}

fn kind_of(handle: &impl AsHandle) -> HandleKind {
    HandleKindQuery::of(handle)
        .expect("a query")
        .perform()
        .expect("a kind")
}

fn system_volume() -> String {
    format!(
        r"\\.\{}",
        std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into())
    )
}

#[test]
fn a_file_this_crate_opened_is_a_disk_file() {
    let path = std::env::temp_dir().join(format!("wdi-kind-file-{}.tmp", std::process::id()));
    std::fs::write(&path, b"x").expect("create the file");
    let name = Wtf16String::from_os_str(path.as_os_str());
    let file =
        OpenedFile::<Cached, _>::open(name, FileOptions::new().read().write()).expect("open");

    assert_eq!(kind_of(file.handle()), HandleKind::DiskFile);

    drop(file);
    std::fs::remove_file(&path).expect("remove the file");
}

#[test]
fn a_directory_is_a_disk_file() {
    let directory = std::env::temp_dir();
    let handle = open(
        directory.to_str().expect("a UTF-8 temp path"),
        GENERIC_READ,
        FILE_FLAG_BACKUP_SEMANTICS,
    );
    assert_eq!(kind_of(&handle), HandleKind::DiskFile);
}

#[test]
fn nul_is_a_character_device() {
    let handle = open(r"\\.\NUL", GENERIC_READ, 0);
    assert_eq!(kind_of(&handle), HandleKind::CharacterDevice);
}

#[test]
fn both_ends_of_a_named_pipe_are_a_pipe() {
    let name = format!(r"\\.\pipe\wdi-kind-{}", std::process::id());
    let options = NamedPipeOptions {
        open_mode: PipeOpenMode::ACCESS_DUPLEX,
        pipe_mode: PipeMode::WAIT,
        max_instances: 1,
        out_buffer_size: 4096,
        in_buffer_size: 4096,
        default_timeout_ms: 0,
    };
    let server = create_named_pipe(&name, &options, None).expect("a pipe");
    let client = open(&name, GENERIC_READ, 0);
    assert_eq!(kind_of(&server), HandleKind::Pipe);
    assert_eq!(kind_of(&client), HandleKind::Pipe);
}

#[test]
fn the_system_volume_is_a_volume_with_its_extents() {
    let volume = open(&system_volume(), NO_ACCESS, 0);
    let HandleKind::Volume { number, extents } = kind_of(&volume) else {
        panic!("the system volume was not classified as a volume");
    };
    assert_ne!(number.partition_number, 0, "a volume is a partition");
    assert!(!extents.is_empty());
    assert!(
        extents
            .iter()
            .any(|extent| extent.disk_number == number.device_number)
    );
}

#[test]
fn the_disk_under_the_system_volume_is_a_physical_disk() {
    let volume = open(&system_volume(), NO_ACCESS, 0);
    let HandleKind::Volume { number, .. } = kind_of(&volume) else {
        panic!("the system volume was not classified as a volume");
    };
    drop(volume);

    let disk = open(
        &format!(r"\\.\PhysicalDrive{}", number.device_number),
        NO_ACCESS,
        0,
    );
    let HandleKind::PhysicalDisk(own) = kind_of(&disk) else {
        panic!("the disk was not classified as a physical disk");
    };
    assert_eq!(own.device_number, number.device_number);
}

#[test]
fn a_query_outlives_the_handle_it_was_built_from() {
    let path = std::env::temp_dir().join(format!("wdi-kind-outlive-{}.tmp", std::process::id()));
    std::fs::write(&path, b"x").expect("create the file");
    let name = Wtf16String::from_os_str(path.as_os_str());
    let file =
        OpenedFile::<Cached, _>::open(name, FileOptions::new().read().write()).expect("open");
    let query = HandleKindQuery::of(file.handle()).expect("a query");
    drop(file);
    std::fs::remove_file(&path).expect("remove the file");

    assert_eq!(query.perform().expect("a kind"), HandleKind::DiskFile);
}

#[test]
fn a_query_can_be_performed_again_and_on_another_thread() {
    let handle = open(r"\\.\NUL", GENERIC_READ, 0);
    let query = HandleKindQuery::of(&handle).expect("a query");
    assert_eq!(
        query.perform().expect("a kind"),
        HandleKind::CharacterDevice
    );
    let on_a_thread = std::thread::spawn(move || query.perform().expect("a kind"));
    assert_eq!(
        on_a_thread.join().expect("the thread"),
        HandleKind::CharacterDevice
    );
}
