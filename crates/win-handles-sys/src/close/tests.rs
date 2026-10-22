// Copyright (c) 2026 Mike Grier
//! Unit tests for `close_handle`.
//!
//! What a close did is observed through named events, whose object is removed when the last handle to it is closed, so a test
//! never asks a closed handle's value anything: another test's thread may have been given it. A handle is protected through the raw
//! `SetHandleInformation`, not through this crate's own.

use std::fs::File;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

use windows_sys::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, FALSE, HANDLE_FLAG_PROTECT_FROM_CLOSE, SetHandleInformation,
};
use windows_sys::Win32::System::Threading::CreateEventW;

use super::close_handle;

/// A manual-reset event, anonymous or named, and whether a named one already existed.
fn event(name: Option<&str>) -> (OwnedHandle, bool) {
    let wide: Option<Vec<u16>> = name.map(|n| n.encode_utf16().chain([0]).collect());
    let name_ptr = wide.as_ref().map_or(std::ptr::null(), |w| w.as_ptr());
    // SAFETY: no attributes are given and the name, if any, is NUL-terminated and outlives the call.
    let raw = unsafe { CreateEventW(std::ptr::null(), 1, 0, name_ptr) };
    // The error is read at once, before anything else can change it.
    let existed =
        std::io::Error::last_os_error().raw_os_error() == Some(ERROR_ALREADY_EXISTS as i32);
    assert!(!raw.is_null(), "create an event");
    // SAFETY: the call returned a new handle nothing else owns.
    (unsafe { OwnedHandle::from_raw_handle(raw) }, existed)
}

/// A name for an event that is this test's alone.
fn name(test: &str) -> String {
    format!("wh-close-{test}-{}", std::process::id())
}

#[test]
fn closing_an_open_handle_succeeds() {
    let (handle, _) = event(None);
    close_handle(handle).expect("closed");
}

#[test]
fn closing_the_last_handle_to_an_object_removes_it() {
    // The page: "After the last handle to an object is closed, the object is removed from the system."
    let name = name("last");
    let (only, existed) = event(Some(&name));
    assert!(!existed, "a new object");

    close_handle(only).expect("closed");

    let (again, existed) = event(Some(&name));
    assert!(!existed, "the object was removed with its last handle");
    close_handle(again).expect("closed");
}

#[test]
fn closing_one_of_two_handles_leaves_the_object() {
    let name = name("two");
    let (first, _) = event(Some(&name));
    let (second, existed) = event(Some(&name));
    assert!(existed, "the same object");

    close_handle(first).expect("closed");

    let (third, existed) = event(Some(&name));
    assert!(existed, "the second handle keeps the object");
    close_handle(second).expect("closed");
    close_handle(third).expect("closed");
}

#[test]
fn a_file_is_closed_through_its_owned_handle() {
    let path = std::env::temp_dir().join(format!("wh-close-file-{}.tmp", std::process::id()));
    let file = File::create(&path).expect("create");

    let closed = close_handle(file.into());

    std::fs::remove_file(&path).expect("remove");
    closed.expect("closed");
}

#[test]
fn a_thousand_handles_closed_in_a_row_all_close() {
    for _ in 0..1000 {
        let (handle, _) = event(None);
        close_handle(handle).expect("closed");
    }
}

#[test]
fn a_handle_is_closed_from_the_thread_it_was_moved_to() {
    let name = name("thread");
    let (handle, _) = event(Some(&name));

    std::thread::spawn(move || close_handle(handle))
        .join()
        .expect("the thread")
        .expect("closed");

    let (again, existed) = event(Some(&name));
    assert!(!existed, "the close on the other thread removed the object");
    close_handle(again).expect("closed");
}

#[test]
fn a_handle_protected_from_close_is_an_error_and_stays_open() {
    // Measured with no debugger attached, 2026-10-21 (WH-D-5): the call returned FALSE with `ERROR_INVALID_HANDLE`, and the
    // handle was still open. The page says only that the flag means "calling the CloseHandle function will not close the object
    // handle"; under a debugger the call raises an exception, which was not measured.
    let (handle, _) = event(None);
    let raw = handle.as_raw_handle();
    // SAFETY: the handle is open.
    assert_ne!(
        unsafe {
            SetHandleInformation(
                raw,
                HANDLE_FLAG_PROTECT_FROM_CLOSE,
                HANDLE_FLAG_PROTECT_FROM_CLOSE,
            )
        },
        FALSE,
        "protect it"
    );

    let error = close_handle(handle).expect_err("a protected handle is not closed");
    assert!(error.raw_os_error().is_some(), "{error}");

    // Still open: the flag can be cleared on it, and then it closes. The handle was taken, so this reclaims it.
    // SAFETY: the handle was not closed, so it is open and nothing else owns it.
    let reclaimed = unsafe {
        assert_ne!(
            SetHandleInformation(raw, HANDLE_FLAG_PROTECT_FROM_CLOSE, 0),
            FALSE,
            "it is still open"
        );
        OwnedHandle::from_raw_handle(raw)
    };
    close_handle(reclaimed).expect("closes once the protection is cleared");
}
