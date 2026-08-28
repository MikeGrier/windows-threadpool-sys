// Copyright (c) Mike Grier.

//! Tests for the `CreateFileW` entry.
//!
//! The three audited flag shapes are exercised by name, because the point of
//! the entry is that all three are expressible without the crate choosing
//! between them.

use std::fs::File;
use std::os::windows::io::{AsHandle, AsRawHandle};

use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED, FILE_GENERIC_READ, FILE_LIST_DIRECTORY,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use wtf_string::Wtf16String;

use super::OpenFile;
use crate::handle::tests::{FILE_CONTENTS, Fixture, handle_allocation};
use crate::security::tests::Absolute;
use crate::{CapturedHandle, SecurityAttributes, SecurityDescriptor, prepare};

/// The share mode all three audited consumers use: a watcher or an enumerator
/// is an observer and must not stop anyone else touching the directory.
const AUDITED_SHARE: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;

fn request_for(path: &std::path::Path) -> OpenFile {
    let text = path.to_str().expect("the fixture path is valid UTF-8");
    OpenFile::new(prepare(&Wtf16String::from(text)).expect("prepare the fixture path"))
}

/// The two audited consumers that open a directory without an overlapped
/// handle: `windows-file-enumeration-sys` and Globazog.
fn unassociated_directory_request(path: &std::path::Path) -> OpenFile {
    request_for(path)
        .with_desired_access(FILE_LIST_DIRECTORY)
        .with_share_mode(AUDITED_SHARE)
        .with_creation_disposition(OPEN_EXISTING)
        .with_flags_and_attributes(FILE_FLAG_BACKUP_SEMANTICS)
}

/// The watcher's shape: the same open plus `FILE_FLAG_OVERLAPPED`, because its
/// handle is destined for a completion port.
fn overlapped_directory_request(path: &std::path::Path) -> OpenFile {
    unassociated_directory_request(path)
        .with_flags_and_attributes(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED)
}

#[test]
fn the_unassociated_directory_shape_opens() {
    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");
    let fixture = Fixture::new("open-unassociated");

    let opened = unassociated_directory_request(fixture.directory())
        .perform()
        .expect("open the fixture directory");

    assert!(
        File::from(opened)
            .metadata()
            .expect("read the directory metadata")
            .is_dir()
    );
}

#[test]
fn the_overlapped_directory_shape_opens() {
    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");
    let fixture = Fixture::new("open-overlapped");

    let opened = overlapped_directory_request(fixture.directory())
        .perform()
        .expect("open the fixture directory for overlapped use");

    assert!(!opened.as_raw_handle().is_null());
}

#[test]
fn the_ordinary_file_shape_opens() {
    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");
    let fixture = Fixture::new("open-file");

    let opened = request_for(&fixture.file())
        .with_desired_access(FILE_GENERIC_READ)
        .with_share_mode(FILE_SHARE_READ)
        .with_creation_disposition(OPEN_EXISTING)
        .perform()
        .expect("open the fixture file");

    assert_eq!(
        File::from(opened).metadata().expect("read metadata").len(),
        FILE_CONTENTS.len() as u64
    );
}

#[test]
fn the_overlapped_flag_is_carried_rather_than_decided() {
    // The split the audit found: two consumers omit the flag, one supplies it,
    // and the difference is a request field. A crate that added or removed it
    // would be making the delivery-model choice it refuses to make.
    let fixture = Fixture::new("open-flag-split");

    let plain = unassociated_directory_request(fixture.directory());
    let overlapped = overlapped_directory_request(fixture.directory());

    assert_eq!(plain.flags_and_attributes(), FILE_FLAG_BACKUP_SEMANTICS);
    assert_eq!(
        overlapped.flags_and_attributes(),
        FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED
    );
    assert_eq!(
        plain.flags_and_attributes() | FILE_FLAG_OVERLAPPED,
        overlapped.flags_and_attributes(),
        "the two shapes differ by exactly the one flag and nothing else"
    );
}

#[test]
fn a_new_request_defaults_nothing_on_the_callers_behalf() {
    // Every field starts at "the caller said nothing". A plausible-looking
    // default is exactly what a caller cannot see they were given.
    let fixture = Fixture::new("open-defaults");

    let request = request_for(&fixture.file());

    assert_eq!(request.desired_access(), 0);
    assert_eq!(request.share_mode(), 0);
    assert_eq!(request.creation_disposition(), 0);
    assert_eq!(request.flags_and_attributes(), 0);
    assert!(request.security().is_none());
    assert!(request.template().is_none());
}

#[test]
fn backup_semantics_is_not_implied_for_a_directory() {
    // Windows refuses to open a directory without the flag, whatever the access
    // mask says. The entry does not add it, so this fails -- which is the
    // caller's mistake surfacing rather than the crate silently correcting it.
    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");
    let fixture = Fixture::new("open-no-backup");

    let outcome = request_for(fixture.directory())
        .with_desired_access(FILE_LIST_DIRECTORY)
        .with_share_mode(AUDITED_SHARE)
        .with_creation_disposition(OPEN_EXISTING)
        .perform();

    assert!(
        outcome.is_err(),
        "omitting FILE_FLAG_BACKUP_SEMANTICS must reach Windows unaltered"
    );
}

#[test]
fn a_missing_path_reports_the_raw_code_unaltered() {
    let fixture = Fixture::new("open-missing");
    let absent = fixture.directory().join("no-such-file.t");

    let error = request_for(&absent)
        .with_desired_access(FILE_GENERIC_READ)
        .with_share_mode(FILE_SHARE_READ)
        .with_creation_disposition(OPEN_EXISTING)
        .perform()
        .expect_err("a missing file cannot be opened");

    assert_eq!(
        error.code(),
        ERROR_FILE_NOT_FOUND,
        "the code is passed through, not reclassified"
    );
}

#[test]
fn a_missing_directory_is_distinguishable_from_a_missing_file() {
    // Windows distinguishes these, so the entry must too -- by doing nothing.
    let fixture = Fixture::new("open-missing-dir");
    let absent = fixture.directory().join("no-such-dir").join("file.t");

    let error = request_for(&absent)
        .with_desired_access(FILE_GENERIC_READ)
        .with_share_mode(FILE_SHARE_READ)
        .with_creation_disposition(OPEN_EXISTING)
        .perform()
        .expect_err("a missing directory cannot be traversed");

    assert_eq!(error.code(), ERROR_PATH_NOT_FOUND);
}

#[test]
fn security_attributes_are_expressible_though_no_consumer_uses_them() {
    // Kept because an entry that cannot express two of its own call's
    // parameters is a narrowed CreateFileW, not because a consumer asked.
    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");
    let fixture = Fixture::new("open-security");
    let absolute = Absolute::with_populated_dacl();

    // SAFETY: the absolute descriptor and everything it names outlive the call.
    let descriptor =
        unsafe { SecurityDescriptor::capture(absolute.as_ptr()) }.expect("capture a descriptor");
    let request = request_for(&fixture.file())
        .with_desired_access(FILE_GENERIC_READ)
        .with_share_mode(FILE_SHARE_READ)
        .with_creation_disposition(OPEN_EXISTING)
        .with_security(Some(SecurityAttributes::new(Some(descriptor), false)));

    assert!(request.security().is_some());
    // OPEN_EXISTING ignores the descriptor, but the inheritance choice still
    // applies, so the call must succeed with the attributes present.
    request.perform().expect("open with security attributes");
}

#[test]
fn a_template_handle_is_expressible_though_no_consumer_uses_one() {
    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");
    let fixture = Fixture::new("open-template");
    let template_source = fixture.open_file();
    let template =
        CapturedHandle::capture(template_source.as_handle()).expect("capture the template handle");

    let request = request_for(&fixture.file())
        .with_desired_access(FILE_GENERIC_READ)
        .with_share_mode(FILE_SHARE_READ)
        .with_creation_disposition(OPEN_EXISTING)
        .with_template(Some(template));

    assert!(request.template().is_some());
    // OPEN_EXISTING ignores hTemplateFile, so this asserts the parameter is
    // carried and accepted, not that it changes the result.
    request.perform().expect("open with a template handle");
}

#[test]
fn a_request_survives_every_input_it_was_built_from() {
    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");

    let (request, expected_len) = {
        let fixture = Fixture::new("open-outlives");
        let template_source = fixture.open_file();
        let absolute = Absolute::with_populated_dacl();

        // SAFETY: the absolute descriptor is alive for this call.
        let descriptor = unsafe { SecurityDescriptor::capture(absolute.as_ptr()) }
            .expect("capture a descriptor");

        let request = request_for(&fixture.file())
            .with_desired_access(FILE_GENERIC_READ)
            .with_share_mode(FILE_SHARE_READ)
            .with_creation_disposition(OPEN_EXISTING)
            .with_security(Some(SecurityAttributes::new(Some(descriptor), false)))
            .with_template(Some(
                CapturedHandle::capture(template_source.as_handle())
                    .expect("capture the template handle"),
            ));

        // The fixture is deliberately leaked so the file outlives this scope;
        // the point is that the *request's* inputs are gone, not the file.
        std::mem::forget(fixture);
        (request, FILE_CONTENTS.len() as u64)
    };

    let opened = request
        .perform()
        .expect("open after every input is dropped");
    assert_eq!(
        File::from(opened).metadata().expect("read metadata").len(),
        expected_len
    );
}

#[test]
fn a_request_performs_the_same_way_on_another_thread() {
    const fn assert_send<T: Send>() {}
    const fn assert_sync<T: Sync>() {}

    assert_send::<OpenFile>();
    assert_sync::<OpenFile>();

    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");
    let fixture = Fixture::new("open-thread");
    let request = unassociated_directory_request(fixture.directory());

    let opened = std::thread::spawn(move || request.perform())
        .join()
        .expect("the worker did not panic")
        .expect("open on a worker thread");

    assert!(
        File::from(opened)
            .metadata()
            .expect("read the directory metadata")
            .is_dir()
    );
}

#[test]
fn a_request_can_be_performed_more_than_once() {
    // A request is a parameter set, not a one-shot ticket: performing it twice
    // yields two independent handles.
    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");
    let fixture = Fixture::new("open-repeat");
    let request = unassociated_directory_request(fixture.directory());

    let first = request.perform().expect("first open");
    let second = request.perform().expect("second open");

    assert_ne!(
        first.as_raw_handle(),
        second.as_raw_handle(),
        "two opens are two handles"
    );
}

#[test]
fn a_copy_is_fallible_because_a_request_may_own_a_handle() {
    // Not `Clone`: duplicating a handle can fail, and the request inherits that
    // from CapturedHandle rather than hiding it behind a signature that would
    // have to panic.
    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");
    let fixture = Fixture::new("open-copy");
    let request = unassociated_directory_request(fixture.directory());

    let copy = request.try_clone().expect("a request with no template");
    drop(request);

    assert!(
        File::from(copy.perform().expect("the copy opens"))
            .metadata()
            .expect("read the directory metadata")
            .is_dir()
    );
}

#[test]
fn a_copy_duplicates_the_template_rather_than_sharing_the_owner() {
    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");
    let fixture = Fixture::new("open-copy-template");
    let template_source = fixture.open_file();
    let request = request_for(&fixture.file())
        .with_desired_access(FILE_GENERIC_READ)
        .with_share_mode(FILE_SHARE_READ)
        .with_creation_disposition(OPEN_EXISTING)
        .with_template(Some(
            CapturedHandle::capture(template_source.as_handle())
                .expect("capture the template handle"),
        ));

    let copy = request.try_clone().expect("duplicate the template");

    assert_ne!(
        copy.template()
            .expect("the copy kept a template")
            .as_handle()
            .as_raw_handle(),
        request
            .template()
            .expect("the original kept a template")
            .as_handle()
            .as_raw_handle(),
        "each request owns its own duplicate"
    );
    drop(request);
    copy.perform()
        .expect("the copy's template outlived the original");
}
