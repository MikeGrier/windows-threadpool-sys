// Copyright (c) Mike Grier.

//! What a file created through [`OpenFile`] with a security descriptor is given: the descriptor's own
//! parsing and rendering are `win-authorization-sys`'s and are tested there; what is checked here is
//! that the request hands the descriptor to `CreateFileW` as it was given.

use std::ffi::c_void;
use std::os::windows::io::{AsRawHandle, OwnedHandle};
use std::ptr;

use win_authorization_sys::{SecurityAttributes, SecurityDescriptor, SecurityInformation};
use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, LocalFree};
use windows_sys::Win32::Security::Authorization::{GetSecurityInfo, SE_FILE_OBJECT};
use windows_sys::Win32::Security::{DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR};
use windows_sys::Win32::Storage::FileSystem::{
    CREATE_NEW, FILE_GENERIC_READ, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
};
use wtf_string::Wtf16String;

use crate::OpenFile;
use crate::handle::tests::{Fixture, handle_allocation};

fn text(sddl: &str) -> Wtf16String {
    Wtf16String::from(sddl)
}

fn parse(sddl: &str) -> SecurityDescriptor {
    SecurityDescriptor::from_sddl(&text(sddl)).unwrap_or_else(|error| panic!("{sddl:?}: {error}"))
}

const EVERYONE_ALL: &str = "D:P(A;;GA;;;WD)";

/// The DACL of the file `handle` is open on, as the text Windows renders it as, read through a
/// documented query.
fn dacl_of(handle: *mut c_void) -> String {
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    // SAFETY: the handle is open with `READ_CONTROL`, the unwanted out-parameters are null, and
    // `descriptor` is a writable pointer for the allocation Windows makes.
    let status = unsafe {
        GetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &raw mut descriptor,
        )
    };
    assert_eq!(status, 0, "read the file's security");

    // SAFETY: Windows returned a valid self-relative descriptor, which stays allocated until freed
    // below, after the copy.
    let captured = unsafe { SecurityDescriptor::capture(descriptor) };
    // SAFETY: the descriptor was allocated with `LocalAlloc` for this call to free, once.
    unsafe { LocalFree(descriptor) };
    captured
        .expect("capture the file's security")
        .to_sddl(SecurityInformation::DACL)
        .expect("write the descriptor as text")
        .to_string_lossy()
}

fn create_with(fixture: &Fixture, name: &str, sddl: &str) -> OwnedHandle {
    let path = fixture.directory().join(name);
    let path = path.to_str().expect("the fixture path is valid UTF-8");
    OpenFile::new(text(path))
        .expect("build the request")
        .with_desired_access(FILE_GENERIC_READ | GENERIC_READ | GENERIC_WRITE)
        .with_share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .with_creation_disposition(CREATE_NEW)
        .with_security(Some(SecurityAttributes::new(Some(parse(sddl)), false)))
        .perform()
        .expect("create the file")
}

#[test]
fn a_file_created_with_a_descriptor_from_text_is_given_that_dacl() {
    let _allocating = handle_allocation().read().expect("not poisoned");
    let fixture = Fixture::new("sddl-create");

    // A protected DACL, so nothing is inherited from the directory and added to it, and file
    // rights by name, because the file system stores a generic right as the file rights it maps to
    // (`GA` reads back as `FA`).
    let everyone_all = "D:P(A;;FA;;;WD)";
    let everyone = create_with(&fixture, "everyone.bin", everyone_all);
    let read_only = create_with(&fixture, "read-only.bin", "D:P(A;;FR;;;WD)(A;;FA;;;BA)");

    assert_eq!(dacl_of(everyone.as_raw_handle()), everyone_all);
    assert_eq!(
        dacl_of(create_with(&fixture, "generic.bin", EVERYONE_ALL).as_raw_handle()),
        everyone_all
    );
    assert_eq!(
        dacl_of(read_only.as_raw_handle()),
        "D:P(A;;FR;;;WD)(A;;FA;;;BA)"
    );
}

#[test]
fn a_failed_security_copy_is_reported_by_a_request_under_its_own_failure() {
    use crate::{HandleCaptureError, HandleCaptureFailure};
    use windows_sys::Win32::Foundation::ERROR_NOT_ENOUGH_MEMORY;

    let source = std::io::Error::from_raw_os_error(
        i32::try_from(ERROR_NOT_ENOUGH_MEMORY).expect("the code fits in i32"),
    );

    let error = HandleCaptureError::copying_security(source);

    assert_eq!(error.failure(), HandleCaptureFailure::CopySecurity);
    assert_eq!(
        error.raw_os_error(),
        Some(i32::try_from(ERROR_NOT_ENOUGH_MEMORY).expect("the code fits in i32"))
    );
}
