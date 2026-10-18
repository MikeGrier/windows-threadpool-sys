// Copyright (c) 2026 Mike Grier
//! What a created file is given (DI-14.3): its attributes and its security descriptor, and the one
//! thing a handle is never given, inheritance.

use std::fs;
use std::os::windows::fs::MetadataExt;
use std::os::windows::io::AsHandle;
use std::path::Path;

use win_authorization_sys::{ObjectType, SecurityInformation, get_security_info};
use win_handles_sys::{HandleFlags, get_handle_information};
use windows_namespace_request_sys::OpenFile;
use windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED;
use windows_sys::Win32::Storage::FileSystem::{
    CREATE_NEW, FILE_ATTRIBUTE_ARCHIVE, FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_NORMAL,
    FILE_ATTRIBUTE_READONLY, FILE_ATTRIBUTE_SYSTEM, FILE_FLAG_NO_BUFFERING, FILE_READ_DATA,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_WRITE_DATA,
};
use wtf_string::Wtf16String;

use super::{
    Cached, DataAccess, Disposition, FileAttributes, FileOptions, FileRights, OpenedFile,
    ReadWrite, SecurityDescriptor, Unbuffered,
};
use crate::dioring::tests::{TempFile, wtf};

fn create_new() -> FileOptions<ReadWrite> {
    FileOptions::new()
        .read()
        .write()
        .disposition(Disposition::CreateNew)
}

fn flags<A: DataAccess>(options: FileOptions<A>) -> u32 {
    options.parameters(0).flags
}

fn attributes_of(path: &Path) -> u32 {
    fs::metadata(path).expect("metadata").file_attributes()
}

fn sddl(text: &str) -> SecurityDescriptor {
    SecurityDescriptor::from_sddl(&Wtf16String::from(text)).expect("a valid descriptor")
}

/// Removes a file whatever attributes it was created with.
// The lint is about Unix, where this would make a file world writable; this crate is Windows only,
// where it clears the read-only attribute.
#[allow(clippy::permissions_set_readonly_false)]
fn remove(path: &Path) {
    let mut permissions = fs::metadata(path).expect("metadata").permissions();
    permissions.set_readonly(false);
    fs::set_permissions(path, permissions).expect("clear read-only");
    fs::remove_file(path).expect("remove");
}

#[test]
fn the_attributes_are_added_to_the_flags_and_none_is_no_bits() {
    let base = FileOptions::new().read().write();
    assert_eq!(flags(base), 0, "none asked for, none passed");
    assert_eq!(
        flags(base.attributes(FileAttributes::NONE)),
        0,
        "the empty set is not FILE_ATTRIBUTE_NORMAL"
    );
    assert_eq!(
        flags(base.attributes(FileAttributes::HIDDEN | FileAttributes::SYSTEM)),
        FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM
    );
    assert_eq!(
        flags(
            base.attributes(FileAttributes::HIDDEN)
                .attributes(FileAttributes::SYSTEM)
        ),
        FILE_ATTRIBUTE_SYSTEM,
        "the last set stated replaces the earlier"
    );
    let all = base
        .attributes(FileAttributes::HIDDEN)
        .delete_on_close(true)
        .random_access(true)
        .open_reparse_point(true)
        .parameters(FILE_FLAG_NO_BUFFERING)
        .flags;
    assert_eq!(
        all,
        FILE_FLAG_NO_BUFFERING
            | FILE_ATTRIBUTE_HIDDEN
            | windows_sys::Win32::Storage::FileSystem::FILE_FLAG_DELETE_ON_CLOSE
            | windows_sys::Win32::Storage::FileSystem::FILE_FLAG_RANDOM_ACCESS
            | windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT,
        "added to the others and never in place of them"
    );
}

#[test]
fn changing_the_data_access_keeps_the_attributes() {
    let moved = FileOptions::new()
        .attributes(FileAttributes::HIDDEN | FileAttributes::TEMPORARY)
        .read()
        .write();
    assert_eq!(
        flags(moved),
        FILE_ATTRIBUTE_HIDDEN | windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_TEMPORARY
    );
}

#[test]
fn a_created_file_carries_the_attributes_asked_for() {
    let temp = TempFile::new("cr-attributes");
    let fresh = temp.0.with_extension("fresh");
    let attributes = FileAttributes::HIDDEN | FileAttributes::SYSTEM;
    let opened = OpenedFile::<Cached>::open(wtf(&fresh), create_new().attributes(attributes))
        .expect("create");
    let found = attributes_of(&fresh);
    assert_eq!(
        found & (FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM),
        FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM
    );
    drop(opened);
    remove(&fresh);
}

#[test]
fn a_file_created_read_only_can_be_written_through_the_handle_that_created_it() {
    // Measured on Windows 11 build 26300: the create succeeds with write access although the file is
    // read-only, and the handle that made it can write; a later open for writing is refused.
    let temp = TempFile::new("cr-readonly");
    let fresh = temp.0.with_extension("fresh");
    let opened = OpenedFile::<Cached>::open(
        wtf(&fresh),
        create_new().attributes(FileAttributes::READONLY),
    )
    .expect("create read-only with write access");
    assert_ne!(attributes_of(&fresh) & FILE_ATTRIBUTE_READONLY, 0);
    {
        use std::io::Write;
        use std::os::windows::io::AsHandle;
        let mut through = fs::File::from(
            opened
                .as_handle()
                .try_clone_to_owned()
                .expect("duplicate the handle"),
        );
        through.write_all(b"written").expect("write through it");
    }
    let refused = OpenedFile::<Cached, _>::open(wtf(&fresh), FileOptions::new().write())
        .expect_err("a read-only file");
    assert_eq!(
        refused.raw_os_error(),
        Some(ERROR_ACCESS_DENIED.cast_signed())
    );
    drop(opened);
    remove(&fresh);
}

#[test]
fn no_attributes_and_the_normal_attribute_create_the_same_file() {
    // What `FILE_ATTRIBUTE_NORMAL` asks for is the absence of attributes, so the empty set passes
    // zero; the two are compared here through the request crate, which can pass either.
    let temp = TempFile::new("cr-normal");
    let none = temp.0.with_extension("none");
    let normal = temp.0.with_extension("normal");
    drop(OpenedFile::<Cached>::open(wtf(&none), create_new()).expect("none"));

    let share = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
    let request = OpenFile::new(wtf(&normal))
        .expect("a path")
        .with_desired_access(FILE_READ_DATA | FILE_WRITE_DATA)
        .with_share_mode(share)
        .with_creation_disposition(CREATE_NEW)
        .with_flags_and_attributes(FILE_ATTRIBUTE_NORMAL);
    drop(request.perform().expect("normal"));

    assert_eq!(attributes_of(&none), attributes_of(&normal));
    assert_ne!(attributes_of(&none) & FILE_ATTRIBUTE_ARCHIVE, 0);
    remove(&none);
    remove(&normal);
}

#[test]
fn an_open_of_a_file_that_exists_leaves_its_attributes_as_they_were() {
    // Measured on Windows 11 build 26300, for every disposition that opens a file that is there.
    let temp = TempFile::new("cr-existing");
    let before = attributes_of(&temp.0);
    for disposition in [Disposition::OpenExisting, Disposition::OpenAlways] {
        let options = FileOptions::new()
            .read()
            .write()
            .disposition(disposition)
            .attributes(FileAttributes::HIDDEN | FileAttributes::SYSTEM);
        drop(OpenedFile::<Cached>::open(wtf(&temp.0), options).expect("open"));
        assert_eq!(attributes_of(&temp.0), before, "{disposition:?}");
    }
}

#[test]
fn the_attributes_reach_an_unbuffered_create_as_they_reach_a_cached_one() {
    let temp = TempFile::new("cr-unbuffered");
    let fresh = temp.0.with_extension("fresh");
    let opened = OpenedFile::<Unbuffered>::open(
        wtf(&fresh),
        create_new().attributes(FileAttributes::HIDDEN),
    )
    .expect("create unbuffered");
    assert_ne!(attributes_of(&fresh) & FILE_ATTRIBUTE_HIDDEN, 0);
    drop(opened);
    remove(&fresh);
}

#[test]
fn a_request_with_no_descriptor_passes_no_security_attributes() {
    // Passing none is what the `CreateFileW` page documents as a handle that cannot be inherited.
    let temp = TempFile::new("cr-none");
    let request = OpenedFile::<Cached>::request(wtf(&temp.0), create_new()).expect("build");
    assert!(request.request.security().is_none());
}

#[test]
fn a_descriptor_is_carried_and_the_handle_is_never_asked_to_be_inheritable() {
    let temp = TempFile::new("cr-carried");
    let request = OpenedFile::<Cached>::request(wtf(&temp.0), create_new())
        .expect("build")
        .with_security_descriptor(sddl("D:P(A;;GA;;;WD)"));
    let attributes = request.request.security().expect("attributes");
    assert!(!attributes.inherit_handle());
    let descriptor = attributes.descriptor().expect("the descriptor");
    assert_eq!(
        descriptor
            .to_sddl(windows_namespace_request_sys::SecurityInformation::DACL)
            .expect("text")
            .to_string_lossy(),
        "D:P(A;;GA;;;WD)"
    );
}

#[test]
fn the_last_descriptor_given_is_the_one_used() {
    let temp = TempFile::new("cr-replaced");
    let request = OpenedFile::<Cached>::request(wtf(&temp.0), create_new())
        .expect("build")
        .with_security_descriptor(sddl("D:P(A;;GA;;;WD)"))
        .with_security_descriptor(sddl("D:P(A;;GR;;;WD)"));
    let text = request
        .request
        .security()
        .and_then(|attributes| attributes.descriptor())
        .expect("the descriptor")
        .to_sddl(windows_namespace_request_sys::SecurityInformation::DACL)
        .expect("text")
        .to_string_lossy();
    assert_eq!(text, "D:P(A;;GR;;;WD)");
}

fn create_with(path: &Path, descriptor: &str) -> OpenedFile {
    OpenedFile::<Cached>::request(wtf(path), create_new())
        .expect("build")
        .with_security_descriptor(sddl(descriptor))
        .perform()
        .expect("create with the descriptor")
}

#[test]
fn a_created_file_is_given_the_descriptor_and_a_later_open_is_judged_by_it() {
    // The descriptor is judged by behaviour as well as read back (below): a DACL that denies read to
    // everyone refuses a later open for reading, and one that allows it admits it.
    let temp = TempFile::new("cr-judged");
    let denied = temp.0.with_extension("denied");
    let allowed = temp.0.with_extension("allowed");

    let held = create_with(&denied, "D:P(D;;GR;;;WD)(A;;GA;;;WD)");
    let error = OpenedFile::<Cached, _>::open(wtf(&denied), FileOptions::new().read())
        .expect_err("read is denied by the descriptor");
    assert_eq!(
        error.raw_os_error(),
        Some(ERROR_ACCESS_DENIED.cast_signed())
    );

    let kept = create_with(&allowed, "D:P(A;;GA;;;WD)");
    OpenedFile::<Cached, _>::open(wtf(&allowed), FileOptions::new().read())
        .expect("read is allowed by the descriptor");

    drop((held, kept));
    fs::remove_file(&denied).expect("remove");
    fs::remove_file(&allowed).expect("remove");
}

/// The DACL of the file `file` holds a handle to, as the text Windows renders it as, read through
/// `win-authorization-sys`'s `GetSecurityInfo`. The file has to have been opened with the right to
/// read its security.
fn dacl_of<B: super::Buffering>(file: &OpenedFile<B, ReadWrite>) -> String {
    get_security_info(
        file.as_handle(),
        ObjectType::FILE,
        SecurityInformation::DACL,
    )
    .expect("read the file's security")
    .to_sddl(SecurityInformation::DACL)
    .expect("write the descriptor as text")
    .to_string_lossy()
}

#[test]
fn a_created_file_is_given_exactly_the_descriptor_it_was_asked_for() {
    // Protected, so nothing is inherited from the directory and added; and file rights by name,
    // because the file system stores a generic right as the file rights it maps to.
    let temp = TempFile::new("cr-read-back");
    for (index, descriptor) in [
        "D:P(A;;FA;;;WD)",
        "D:P(A;;FR;;;WD)",
        "D:P(A;;FR;;;WD)(A;;FA;;;BA)",
        "D:P(D;;FW;;;WD)(A;;FR;;;WD)",
        "D:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;FR;;;WD)",
    ]
    .into_iter()
    .enumerate()
    {
        let path = temp.0.with_extension(format!("read-back-{index}"));
        let file = OpenedFile::<Cached>::request(
            wtf(&path),
            create_new().rights(FileRights::READ_CONTROL),
        )
        .expect("build")
        .with_security_descriptor(sddl(descriptor))
        .perform()
        .expect("create with the descriptor");

        assert_eq!(dacl_of(&file), descriptor);

        drop(file);
        fs::remove_file(&path).expect("remove");
    }
}

#[test]
fn an_unbuffered_create_is_given_the_descriptor_as_a_cached_one_is() {
    let temp = TempFile::new("cr-read-back-unbuffered");
    let path = temp.0.with_extension("fresh");
    let file = OpenedFile::<Unbuffered>::request(
        wtf(&path),
        create_new().rights(FileRights::READ_CONTROL),
    )
    .expect("build")
    .with_security_descriptor(sddl("D:P(A;;FR;;;WD)(A;;FA;;;BA)"))
    .perform()
    .expect("create unbuffered with the descriptor");

    assert_eq!(dacl_of(&file), "D:P(A;;FR;;;WD)(A;;FA;;;BA)");

    drop(file);
    fs::remove_file(&path).expect("remove");
}

#[test]
fn the_handle_an_open_makes_is_not_inheritable_with_a_descriptor_or_without() {
    // `bInheritHandle` is false whether the request carries a descriptor or none (the request is
    // where the choice is made), and the handle that results says so.
    let temp = TempFile::new("cr-not-inheritable");
    let with_descriptor = temp.0.with_extension("described");
    let described = create_with(&with_descriptor, "D:P(A;;FA;;;WD)");
    assert_eq!(
        get_handle_information(described.as_handle()).expect("query"),
        HandleFlags::NONE
    );

    let plain = OpenedFile::<Cached>::open(
        wtf(&temp.0),
        create_new().disposition(Disposition::OpenExisting),
    )
    .expect("open the file that is there");
    assert_eq!(
        get_handle_information(plain.as_handle()).expect("query"),
        HandleFlags::NONE
    );

    drop((described, plain));
    fs::remove_file(&with_descriptor).expect("remove");
}

#[test]
fn the_descriptor_is_ignored_for_a_file_that_exists() {
    // The `CreateFileW` page says so, and it is measured here: a descriptor that would deny reading
    // does not change the file an open finds.
    let temp = TempFile::new("cr-ignored");
    OpenedFile::<Cached>::request(
        wtf(&temp.0),
        FileOptions::new()
            .read()
            .write()
            .disposition(Disposition::OpenAlways),
    )
    .expect("build")
    .with_security_descriptor(sddl("D:P(D;;GR;;;WD)(A;;GA;;;WD)"))
    .perform()
    .expect("open the file that is there");
    OpenedFile::<Cached, _>::open(wtf(&temp.0), FileOptions::new().read())
        .expect("reading is still allowed");
}

#[test]
fn a_descriptor_that_windows_refuses_is_refused_before_any_open() {
    let error = SecurityDescriptor::from_sddl(&Wtf16String::from("not sddl"))
        .expect_err("not a descriptor");
    assert!(error.raw_os_error().is_some());
}
