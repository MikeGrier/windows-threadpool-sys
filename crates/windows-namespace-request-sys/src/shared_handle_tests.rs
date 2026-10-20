// Copyright (c) Mike Grier.

//! Every entry that takes a handle holds it shared (`NR-6.1`, `D-32`): the request is one more holder
//! of the handle it was given, a clone of the request is another, and no entry makes a handle of its
//! own to hold.

use std::os::windows::io::{AsHandle, AsRawHandle};

use win_shared_os_owned_handle::SharedHandle;
use windows_sys::Win32::Storage::FileSystem::{
    FILE_GENERIC_READ, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
};
use wtf_string::Wtf16String;

use crate::handle::tests::{Fixture, handle_allocation};
use crate::{
    CapturedHandle, FileIdentifier, FileInformationClass, FileType, OpenFile, OpenFileByIdentifier,
    QueryFileIdInfo, QueryFileInformation, QueryFileInformationByHandle, QueryFileType,
    QueryFinalPath, QueryVolumeInformation, ReOpenFile,
};

const SHARE_ALL: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;

fn file_handle(fixture: &Fixture) -> SharedHandle {
    SharedHandle::from(fixture.open_file())
}

#[test]
fn every_entry_holds_the_handle_it_is_given_and_makes_none_of_its_own() {
    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");
    let fixture = Fixture::new("shared-holds");
    let handle = file_handle(&fixture);

    assert!(
        QueryFileType::new(handle.clone())
            .handle()
            .same_handle(&handle)
    );
    assert!(
        QueryFileIdInfo::new(handle.clone())
            .handle()
            .same_handle(&handle)
    );
    assert!(
        QueryFileInformationByHandle::new(handle.clone())
            .handle()
            .same_handle(&handle)
    );
    assert!(
        QueryFinalPath::new(handle.clone())
            .handle()
            .same_handle(&handle)
    );
    assert!(
        QueryVolumeInformation::new(handle.clone())
            .handle()
            .same_handle(&handle)
    );
    assert!(
        QueryFileInformation::new(handle.clone(), FileInformationClass::BASIC)
            .handle()
            .same_handle(&handle)
    );
    assert!(
        ReOpenFile::new(handle.clone())
            .original()
            .same_handle(&handle)
    );
    assert!(
        OpenFileByIdentifier::new(handle.clone(), FileIdentifier::FileId(0))
            .volume_hint()
            .same_handle(&handle)
    );
    let path = fixture.file();
    let template = OpenFile::new(Wtf16String::from(
        path.to_str().expect("the fixture path is valid UTF-8"),
    ))
    .expect("build the request")
    .with_template(Some(handle.clone()));
    assert!(
        template
            .template()
            .expect("the template is held")
            .same_handle(&handle)
    );
}

#[test]
fn a_clone_of_a_request_shares_the_handle_the_request_holds() {
    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");
    let fixture = Fixture::new("shared-clone");
    let handle = file_handle(&fixture);

    let file_type = QueryFileType::new(handle.clone());
    assert!(file_type.clone().handle().same_handle(file_type.handle()));
    let file_id = QueryFileIdInfo::new(handle.clone());
    assert!(file_id.clone().handle().same_handle(file_id.handle()));
    let by_handle = QueryFileInformationByHandle::new(handle.clone());
    assert!(by_handle.clone().handle().same_handle(by_handle.handle()));
    let final_path = QueryFinalPath::new(handle.clone());
    assert!(final_path.clone().handle().same_handle(final_path.handle()));
    let volume = QueryVolumeInformation::new(handle.clone());
    assert!(volume.clone().handle().same_handle(volume.handle()));
    let query = QueryFileInformation::new(handle.clone(), FileInformationClass::BASIC);
    assert!(query.clone().handle().same_handle(query.handle()));
    let reopen = ReOpenFile::new(handle);
    assert!(reopen.clone().original().same_handle(reopen.original()));
}

#[test]
fn a_request_keeps_its_handle_open_after_every_other_holder_drops() {
    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");
    let fixture = Fixture::new("shared-outlives");
    let handle = file_handle(&fixture);

    let file_type = QueryFileType::new(handle.clone());
    let file_id = QueryFileIdInfo::new(handle.clone());
    let by_handle = QueryFileInformationByHandle::new(handle.clone());
    let final_path = QueryFinalPath::new(handle.clone());
    let volume = QueryVolumeInformation::new(handle.clone());
    let query =
        QueryFileInformation::new(handle.clone(), FileInformationClass::BASIC).with_capacity(64);
    let reopen = ReOpenFile::new(handle)
        .with_desired_access(FILE_GENERIC_READ)
        .with_share_mode(SHARE_ALL);

    // `handle` has moved into the last request; the requests are now its only holders.
    assert_eq!(
        file_type.perform().expect("the file's type"),
        FileType::DISK
    );
    file_id.perform().expect("the file's identity");
    by_handle.perform().expect("the file's information");
    final_path.perform().expect("the file's final path");
    volume.perform().expect("the file's volume");
    query.perform().expect("the file's basic information");
    reopen.perform().expect("the file reopened");
}

#[test]
fn a_request_performs_on_another_thread_with_its_clone_left_behind() {
    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");
    let fixture = Fixture::new("shared-thread");
    let request = QueryFileIdInfo::new(file_handle(&fixture));
    let here = request.clone();

    let there = std::thread::spawn(move || request.perform().expect("the identity on a worker"))
        .join()
        .expect("the worker did not panic");

    assert_eq!(there, here.perform().expect("the identity here"));
}

#[test]
fn a_captured_handle_converts_into_a_shared_handle_holding_its_duplicate() {
    // The transition conversion, deleted by `NR-6.2`: the shared handle is the captured duplicate,
    // not a further one.
    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");
    let fixture = Fixture::new("shared-from-captured");
    let file = fixture.open_file();
    let captured = CapturedHandle::capture(file.as_handle()).expect("capture the handle");
    let raw = captured.as_handle().as_raw_handle();

    let shared = SharedHandle::from(captured);

    assert_eq!(shared.as_raw_handle(), raw);
    assert_ne!(shared.as_raw_handle(), file.as_raw_handle());
}
