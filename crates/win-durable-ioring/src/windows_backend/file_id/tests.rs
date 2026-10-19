// Copyright (c) 2026 Mike Grier
//! Tests for `FileId`: that an identity is the file's and not the open's, that two files' differ,
//! and that it is what the request crate reads through the same handle. What a real file system
//! reports is what was observed on the machine this was written on.

use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::os::windows::fs::OpenOptionsExt;

use win_shared_os_owned_handle::SharedHandle;
use win_sync_sys::{Event, ResetMode};
use windows_namespace_request_sys::file_id::QueryFileIdInfo;
use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS;

use super::FileId;
use crate::dioring::tests::{TempFile, wtf};
use crate::windows_backend::{Cached, FileOptions, NoData, OpenedFile, ReadOnly, Unbuffered};

fn id_of(file: &TempFile) -> FileId {
    file.open().file_id().expect("the identity of an open file")
}

#[test]
fn two_opens_of_one_file_report_one_identity() {
    let temp = TempFile::new("fid-two-opens");
    let (first, second) = (temp.open(), temp.open());
    assert_eq!(
        first.file_id().expect("first"),
        second.file_id().expect("second")
    );
}

#[test]
fn two_files_on_one_volume_report_one_serial_and_different_identifiers() {
    let (one, two) = (TempFile::new("fid-one"), TempFile::new("fid-two"));
    let (one, two) = (id_of(&one), id_of(&two));
    assert_eq!(one.volume_serial_number(), two.volume_serial_number());
    assert_ne!(one.id(), two.id());
    assert_ne!(one, two);
}

#[test]
fn the_identity_is_what_the_request_crate_reads_through_the_same_handle() {
    let temp = TempFile::new("fid-request");
    let file = temp.open();
    let info = QueryFileIdInfo::new(file.handle().clone())
        .perform()
        .expect("the request crate's read");
    let id = file.file_id().expect("this crate's read");
    assert_eq!(id.volume_serial_number(), info.volume_serial_number());
    assert_eq!(id.id(), info.file_id());
}

#[test]
fn an_open_with_no_data_access_reads_the_identity_an_open_with_data_does() {
    let temp = TempFile::new("fid-no-data");
    let none = OpenedFile::<Cached, NoData>::open(wtf(&temp.0), FileOptions::new()).expect("open");
    let read = OpenedFile::<Cached, ReadOnly>::open(wtf(&temp.0), FileOptions::new().read())
        .expect("open");
    assert_eq!(none.file_id().expect("no data"), id_of(&temp));
    assert_eq!(read.file_id().expect("read only"), id_of(&temp));
}

#[test]
fn an_unbuffered_open_reports_the_identity_a_cached_one_does() {
    let temp = TempFile::new("fid-unbuffered");
    let unbuffered =
        OpenedFile::<Unbuffered, _>::open(wtf(&temp.0), FileOptions::new().read().write())
            .expect("open unbuffered");
    assert_eq!(unbuffered.file_id().expect("unbuffered"), id_of(&temp));
}

#[test]
fn a_clone_shares_the_handle_and_so_the_identity() {
    let temp = TempFile::new("fid-clone");
    let file = temp.open();
    let clone = file.clone();
    assert_eq!(
        clone.file_id().expect("clone"),
        file.file_id().expect("file")
    );
}

#[test]
fn the_handle_shared_out_of_a_dropped_file_still_reads_its_identity() {
    let temp = TempFile::new("fid-dropped");
    let file = temp.open();
    let expected = file.file_id().expect("before");
    let handle = file.handle().clone();
    drop(file);
    assert_eq!(FileId::of(&handle).expect("after"), expected);
}

#[test]
fn the_identity_survives_a_rename_and_a_change_of_contents() {
    let temp = TempFile::new("fid-rename");
    let file = temp.open();
    let before = file.file_id().expect("before");

    let moved = temp.0.with_extension("moved");
    fs::rename(&temp.0, &moved).expect("rename while open, which the default share mode allows");
    fs::write(&moved, b"other contents, and longer").expect("rewrite");
    let reopened = OpenedFile::<Cached, _>::open(wtf(&moved), FileOptions::new().read())
        .expect("open at the new name");
    let (through_old, through_new) = (
        file.file_id().expect("through the first handle"),
        reopened.file_id().expect("through the new one"),
    );
    drop((file, reopened));
    fs::rename(&moved, &temp.0).expect("rename back, so the file is removed");

    assert_eq!(through_old, before);
    assert_eq!(through_new, before);
}

#[test]
fn a_hard_link_reports_the_identity_of_its_file() {
    let temp = TempFile::new("fid-link");
    let link = temp.0.with_extension("link");
    fs::hard_link(&temp.0, &link).expect("a hard link");
    let through_link = OpenedFile::<Cached, _>::open(wtf(&link), FileOptions::new().read())
        .expect("open the link")
        .file_id();
    fs::remove_file(&link).expect("remove the link");
    assert_eq!(through_link.expect("through the link"), id_of(&temp));
}

#[test]
fn a_directory_has_an_identity_of_its_own() {
    let temp = TempFile::new("fid-directory");
    let directory = temp.0.with_extension("dir");
    fs::create_dir(&directory).expect("a directory");
    let handle = OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(&directory)
        .map(SharedHandle::from);
    let id = handle.and_then(|handle| FileId::of(&handle));
    fs::remove_dir(&directory).expect("remove the directory");

    let id = id.expect("a directory's identity");
    let file = id_of(&temp);
    assert_eq!(id.volume_serial_number(), file.volume_serial_number());
    assert_ne!(id, file);
}

#[test]
fn a_handle_that_is_not_a_file_is_refused_with_the_kernels_code() {
    let event = Event::new(ResetMode::Manual, false).expect("an event");
    let error = FileId::of(&SharedHandle::from(event)).expect_err("an event has no file identity");
    assert!(
        error.raw_os_error().is_some(),
        "the refusal is the kernel's code, carried unaltered: {error:?}"
    );
}

#[test]
fn new_carries_its_fields_and_equality_and_hashing_use_both() {
    let id = [
        0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 9, 10, 11, 12, 13, 14, 15, 16,
    ];
    let value = FileId::new(0x0123_4567_89ab_cdef, id);
    assert_eq!(value.volume_serial_number(), 0x0123_4567_89ab_cdef);
    assert_eq!(value.id(), id);

    let mut other_id = id;
    other_id[15] ^= 1;
    let other_volume = FileId::new(0x0123_4567_89ab_cdee, id);
    let other_file = FileId::new(0x0123_4567_89ab_cdef, other_id);
    assert_ne!(value, other_volume);
    assert_ne!(value, other_file);
    assert_eq!(value, FileId::new(0x0123_4567_89ab_cdef, id));

    let set: HashSet<FileId> = [value, other_volume, other_file, value]
        .into_iter()
        .collect();
    assert_eq!(set.len(), 3);
}
