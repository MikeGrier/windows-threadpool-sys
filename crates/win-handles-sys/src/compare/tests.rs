// Copyright (c) 2026 Mike Grier
//! Unit tests for `compare_object_handles`.
//!
//! The objects are made with raw calls and the duplicates by `std`'s `try_clone`, never by this crate's own duplicate, so the
//! comparison and the duplication are not checked against each other.

use std::fs::File;
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle};

use windows_sys::Win32::Foundation::{DuplicateHandle, FALSE, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Threading::{
    CreateEventW, CreateMutexW, GetCurrentProcess, GetCurrentProcessId, OpenProcess,
    PROCESS_QUERY_LIMITED_INFORMATION,
};

use super::compare_object_handles;

/// A manual-reset event, anonymous or named.
fn event(name: Option<&str>) -> OwnedHandle {
    let wide: Option<Vec<u16>> = name.map(|n| n.encode_utf16().chain([0]).collect());
    let name_ptr = wide.as_ref().map_or(std::ptr::null(), |w| w.as_ptr());
    // SAFETY: no attributes are given and the name, if any, is NUL-terminated and outlives the call.
    let raw = unsafe { CreateEventW(std::ptr::null(), 1, 0, name_ptr) };
    assert!(!raw.is_null(), "create an event");
    // SAFETY: the call returned a new handle nothing else owns.
    unsafe { OwnedHandle::from_raw_handle(raw) }
}

fn mutex() -> OwnedHandle {
    // SAFETY: no attributes and no name are given.
    let raw = unsafe { CreateMutexW(std::ptr::null(), 0, std::ptr::null()) };
    assert!(!raw.is_null(), "create a mutex");
    // SAFETY: the call returned a new handle nothing else owns.
    unsafe { OwnedHandle::from_raw_handle(raw) }
}

fn duplicate(handle: &OwnedHandle) -> OwnedHandle {
    handle.try_clone().expect("duplicate")
}

/// A temporary file's path, unique to this test.
fn path(test: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("wh-compare-{test}-{}.tmp", std::process::id()))
}

#[test]
fn a_handle_is_the_same_object_as_itself() {
    let handle = event(None);
    assert!(compare_object_handles(&handle, &handle));
}

#[test]
fn a_duplicate_is_the_same_object_whichever_is_first() {
    let handle = event(None);
    let copy = duplicate(&handle);
    assert_ne!(
        copy.as_raw_handle(),
        handle.as_raw_handle(),
        "a handle of its own"
    );
    assert!(compare_object_handles(&handle, &copy));
    assert!(compare_object_handles(&copy, &handle));
}

#[test]
fn two_events_made_separately_are_not_the_same_object() {
    let (a, b) = (event(None), event(None));
    assert!(!compare_object_handles(&a, &b));
    assert!(!compare_object_handles(&b, &a));
}

#[test]
fn a_named_event_made_twice_is_one_object() {
    // The page's own example.
    let name = format!("wh-compare-named-{}", std::process::id());
    let (first, second) = (event(Some(&name)), event(Some(&name)));
    assert_ne!(first.as_raw_handle(), second.as_raw_handle());
    assert!(compare_object_handles(&first, &second));
    assert!(compare_object_handles(&second, &first));
}

#[test]
fn objects_of_different_types_are_never_the_same() {
    // The page: "legal to make, though the function will always return FALSE".
    let (event, mutex) = (event(None), mutex());
    // SAFETY: `INVALID_HANDLE_VALUE` is the one value besides an open handle that `BorrowedHandle::borrow_raw` allows, and
    // names the current process.
    let process = unsafe { BorrowedHandle::borrow_raw(INVALID_HANDLE_VALUE) };
    assert!(!compare_object_handles(&event, &mutex));
    assert!(!compare_object_handles(&mutex, &event));
    assert!(!compare_object_handles(&event, process));
    assert!(!compare_object_handles(process, &mutex));
}

#[test]
fn the_current_process_pseudo_handle_is_the_current_process() {
    // The page's use: "whether a process handle is a handle to the current process".
    // SAFETY: the process id is this one's.
    let raw = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION,
            FALSE,
            GetCurrentProcessId(),
        )
    };
    assert!(!raw.is_null(), "open this process");
    // SAFETY: the call returned a new handle nothing else owns.
    let real = unsafe { OwnedHandle::from_raw_handle(raw) };
    // SAFETY: as above: `INVALID_HANDLE_VALUE` is the pseudo-handle `GetCurrentProcess` returns.
    let pseudo = unsafe { BorrowedHandle::borrow_raw(INVALID_HANDLE_VALUE) };
    // SAFETY: the call takes no arguments.
    assert_eq!(unsafe { GetCurrentProcess() }, INVALID_HANDLE_VALUE);

    assert!(compare_object_handles(pseudo, &real));
    assert!(compare_object_handles(&real, pseudo));
    assert!(compare_object_handles(pseudo, pseudo));
}

#[test]
fn a_handle_that_grants_no_access_is_compared_all_the_same() {
    // The page: "without imposing a requirement that specific access rights be granted to either handle".
    let source = event(None);
    let mut bare: HANDLE = std::ptr::null_mut();
    // SAFETY: both handles are this process's, the source is open, and `bare` is a valid out pointer; no access is asked for
    // and none is copied, so the new handle grants nothing.
    let ok = unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            source.as_raw_handle(),
            GetCurrentProcess(),
            &mut bare,
            0,
            FALSE,
            0,
        )
    };
    assert_ne!(ok, FALSE, "duplicate with no access");
    // SAFETY: the call returned a new handle nothing else owns.
    let bare = unsafe { OwnedHandle::from_raw_handle(bare) };

    assert!(compare_object_handles(&source, &bare));
    assert!(compare_object_handles(&bare, &source));
}

#[test]
fn a_file_and_its_duplicate_are_one_object_and_two_opens_of_it_are_two() {
    // Measured, 2026-10-21 (WH-D-4): each open of a file makes a file object of its own.
    let path = path("opens");
    let file = File::create(&path).expect("create");
    let duplicate = file.try_clone().expect("duplicate");
    let another_open = File::open(&path).expect("open again");

    let same = compare_object_handles(&file, &duplicate);
    let different = compare_object_handles(&file, &another_open);

    drop((file, duplicate, another_open));
    std::fs::remove_file(&path).expect("remove");
    assert!(same);
    assert!(!different);
}

#[test]
fn each_handle_may_be_borrowed_a_reference_or_owned() {
    let handle = event(None);

    assert!(compare_object_handles(&handle, &handle));
    assert!(compare_object_handles(handle.as_handle(), &handle));
    assert!(compare_object_handles(&handle, handle.as_handle()));
    // An owned handle is closed by the call, so the other is a duplicate of it.
    assert!(compare_object_handles(duplicate(&handle), &handle));
    assert!(compare_object_handles(&handle, duplicate(&handle)));
    assert!(compare_object_handles(
        duplicate(&handle),
        duplicate(&handle)
    ));
}

#[test]
fn comparing_is_an_equivalence_over_duplicates_of_three_objects() {
    // Seven handles in three groups: duplicates of one object, of a second, and of a mutex. Every pair is the same object
    // exactly when it is in one group, which is symmetry and transitivity as well as the answer.
    let a0 = event(None);
    let a1 = duplicate(&a0);
    let a2 = duplicate(&a1);
    let b0 = event(None);
    let b1 = duplicate(&b0);
    let c0 = mutex();
    let c1 = duplicate(&c0);
    let handles = [
        (&a0, 'a'),
        (&a1, 'a'),
        (&a2, 'a'),
        (&b0, 'b'),
        (&b1, 'b'),
        (&c0, 'c'),
        (&c1, 'c'),
    ];

    for (first, first_group) in handles {
        for (second, second_group) in handles {
            assert_eq!(
                compare_object_handles(first, second),
                first_group == second_group,
                "{first_group} against {second_group}"
            );
        }
    }
}

#[test]
fn a_comparison_leaves_both_handles_open_and_usable() {
    let (a, b) = (event(None), event(None));

    for _ in 0..3 {
        assert!(!compare_object_handles(&a, &b));
        assert!(compare_object_handles(&a, &a));
    }

    // A handle the comparison closed would no longer be duplicable.
    duplicate(&a);
    duplicate(&b);
}
