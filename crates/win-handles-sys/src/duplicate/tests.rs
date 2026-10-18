// Copyright (c) 2026 Mike Grier
// Split from crates/win-sync-sys/src/duplicate/tests.rs at a9a97bf6.
//! Unit tests for `duplicate_handle_with_access` and `Access`.
//!
//! What a duplicate can and cannot do is observed through raw Win32 calls, never through anything
//! this crate offers to set, reset or wait, so a defect in the duplication cannot hide behind the
//! thing it duplicated. The objects are events made with `CreateEventW`, since the events of
//! `win-sync-sys` are a layer above this crate.

use std::os::windows::io::{AsHandle, AsRawHandle, FromRawHandle, OwnedHandle};

use windows_sys::Win32::Foundation::{
    CompareObjectHandles, ERROR_ACCESS_DENIED, FALSE, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, ResetEvent, SYNCHRONIZATION_SYNCHRONIZE,
    SetEvent, WaitForSingleObject,
};

use super::{Access, duplicate_handle_with_access};
use crate::{HandleFlags, get_handle_information};

/// A manual-reset event with no name and default security.
fn manual_event(signalled: bool) -> OwnedHandle {
    // SAFETY: no attributes and no name are given.
    let raw = unsafe { CreateEventW(std::ptr::null(), 1, i32::from(signalled), std::ptr::null()) };
    assert!(!raw.is_null(), "create an event");
    // SAFETY: the call returned a new handle nothing else owns.
    unsafe { OwnedHandle::from_raw_handle(raw) }
}

fn duplicate(handle: impl AsHandle, access: Access) -> OwnedHandle {
    duplicate_handle_with_access(handle, access).expect("duplicate")
}

/// What a zero-timeout wait on `handle` returned.
fn poll(handle: &impl AsRawHandle) -> u32 {
    // SAFETY: the handle is open for the duration of the borrow.
    unsafe { WaitForSingleObject(handle.as_raw_handle(), 0) }
}

/// The error `SetEvent` reports on `handle`, `None` if it succeeded.
fn set_error(handle: &impl AsRawHandle) -> Option<i32> {
    // SAFETY: the handle is open for the duration of the borrow.
    if unsafe { SetEvent(handle.as_raw_handle()) } == FALSE {
        return std::io::Error::last_os_error().raw_os_error();
    }
    None
}

/// The error `ResetEvent` reports on `handle`, `None` if it succeeded.
fn reset_error(handle: &impl AsRawHandle) -> Option<i32> {
    // SAFETY: the handle is open for the duration of the borrow.
    if unsafe { ResetEvent(handle.as_raw_handle()) } == FALSE {
        return std::io::Error::last_os_error().raw_os_error();
    }
    None
}

fn same_object(a: &impl AsRawHandle, b: &impl AsRawHandle) -> bool {
    // SAFETY: both handles are open for the duration of the borrows.
    unsafe { CompareObjectHandles(a.as_raw_handle(), b.as_raw_handle()) != FALSE }
}

const DENIED: Option<i32> = Some(ERROR_ACCESS_DENIED as i32);

#[test]
fn a_duplicate_is_the_same_object_and_a_new_event_is_not() {
    let event = manual_event(false);
    let other = manual_event(false);
    let copy = duplicate(&event, Access::SYNCHRONIZE);
    assert!(same_object(&event, &copy));
    assert!(!same_object(&other, &copy));
    assert_ne!(copy.as_raw_handle(), event.as_raw_handle());
}

#[test]
fn a_synchronize_only_duplicate_can_wait_and_sees_the_signal() {
    let event = manual_event(false);
    let copy = duplicate(&event, Access::SYNCHRONIZE);
    assert_eq!(poll(&copy), WAIT_TIMEOUT);
    assert_eq!(set_error(&event), None, "set the original");
    assert_eq!(poll(&copy), WAIT_OBJECT_0);
}

#[test]
fn a_synchronize_only_duplicate_can_neither_set_nor_reset() {
    let event = manual_event(false);
    let copy = duplicate(&event, Access::SYNCHRONIZE);

    assert_eq!(set_error(&copy), DENIED);
    assert_eq!(
        poll(&event),
        WAIT_TIMEOUT,
        "the refused set changed nothing"
    );

    assert_eq!(set_error(&event), None, "set the original");
    assert_eq!(reset_error(&copy), DENIED);
    assert_eq!(
        poll(&event),
        WAIT_OBJECT_0,
        "the refused reset changed nothing"
    );
}

#[test]
fn a_duplicate_with_modify_state_changes_the_original() {
    let event = manual_event(false);
    let copy = duplicate(&event, Access::EVENT_MODIFY_STATE | Access::SYNCHRONIZE);
    assert_eq!(set_error(&copy), None);
    assert_eq!(poll(&event), WAIT_OBJECT_0, "set through the duplicate");
    assert_eq!(reset_error(&copy), None);
    assert_eq!(poll(&event), WAIT_TIMEOUT, "reset through the duplicate");
}

#[test]
fn a_duplicate_with_modify_state_alone_can_change_the_event_and_not_wait_on_it() {
    let event = manual_event(true);
    let copy = duplicate(&event, Access::EVENT_MODIFY_STATE);
    assert_eq!(set_error(&copy), None);
    assert_eq!(reset_error(&copy), None);
    assert_eq!(poll(&event), WAIT_TIMEOUT, "the duplicate reset it");
    assert_eq!(poll(&copy), WAIT_FAILED, "no SYNCHRONIZE, no wait");
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        DENIED,
        "the wait is refused for access"
    );
}

/// **Windows does not confine a duplicate to the source handle's access**, which is why the
/// documentation of a wait-only handle says what it does and does not guarantee. This test is the
/// evidence for that sentence: it observes the platform, and a Windows that began refusing would
/// fail it, which is the cue to tighten the documentation.
#[test]
fn a_duplicate_may_hold_rights_its_source_does_not() {
    let event = manual_event(false);
    let waiter = duplicate(&event, Access::SYNCHRONIZE);
    assert_eq!(set_error(&waiter), DENIED, "the source cannot set");

    let widened = duplicate(&waiter, Access::EVENT_MODIFY_STATE | Access::SYNCHRONIZE);
    assert!(same_object(&widened, &event));
    assert_eq!(set_error(&widened), None, "the duplicate can");
    assert_eq!(poll(&event), WAIT_OBJECT_0);
}

#[test]
fn narrowing_and_asking_for_what_the_source_holds_both_work() {
    let event = manual_event(false);
    let both = duplicate(&event, Access::EVENT_MODIFY_STATE | Access::SYNCHRONIZE);
    let narrowed = duplicate(&both, Access::SYNCHRONIZE);
    let same = duplicate(&narrowed, Access::SYNCHRONIZE);
    let nothing = duplicate(&same, Access::from_bits(0));
    for copy in [&both, &narrowed, &same, &nothing] {
        assert!(same_object(copy, &event));
    }
    assert_eq!(set_error(&narrowed), DENIED);
    assert_eq!(set_error(&same), DENIED);
}

/// The error path: a mask Windows refuses. Reserved bits are refused with access denied on the
/// Windows this was observed on, which gives the error a deterministic way to be reached; what a
/// mask means is the object type's, so this says nothing about any mask but this one.
#[test]
fn a_mask_windows_refuses_is_reported_and_makes_no_handle() {
    let event = manual_event(false);
    let refused = duplicate_handle_with_access(&event, Access::from_bits(u32::MAX))
        .expect_err("every bit set, reserved ones included");
    assert_eq!(refused.raw_os_error(), DENIED);
    assert_eq!(
        set_error(&event),
        None,
        "the source is untouched by the refusal"
    );
    assert_eq!(poll(&event), WAIT_OBJECT_0);
}

#[test]
fn a_duplicate_with_no_rights_can_do_nothing() {
    let event = manual_event(true);
    let nothing = duplicate(&event, Access::from_bits(0));
    assert_eq!(poll(&nothing), WAIT_FAILED);
    assert_eq!(set_error(&nothing), DENIED);
    assert_eq!(poll(&event), WAIT_OBJECT_0, "the original is untouched");
}

#[test]
fn a_duplicate_outlives_its_source() {
    let event = manual_event(false);
    let keeper = duplicate(&event, Access::EVENT_MODIFY_STATE | Access::SYNCHRONIZE);
    let copy = duplicate(&event, Access::SYNCHRONIZE);
    drop(event);
    assert_eq!(set_error(&keeper), None, "set through the kept duplicate");
    assert_eq!(poll(&copy), WAIT_OBJECT_0);
    drop(keeper);
    // Every other handle is closed; the duplicate alone keeps the object, and it still answers.
    assert_eq!(poll(&copy), WAIT_OBJECT_0);
}

#[test]
fn closing_a_duplicate_leaves_its_source_working() {
    let event = manual_event(false);
    drop(duplicate(&event, Access::SYNCHRONIZE));
    assert_eq!(set_error(&event), None, "set after the duplicate closed");
    assert_eq!(poll(&event), WAIT_OBJECT_0);
}

#[test]
fn a_duplicate_is_not_inheritable() {
    let event = manual_event(false);
    let copy = duplicate(&event, Access::SYNCHRONIZE);
    let flags = get_handle_information(&copy).expect("query the duplicate");
    assert!(!flags.contains(HandleFlags::INHERIT));
}

#[test]
fn any_kind_of_object_can_be_duplicated_not_only_an_event() {
    // SAFETY: an unnamed, unowned mutex with default security; null pointers are accepted.
    let raw = unsafe { CreateMutexW(std::ptr::null(), FALSE, std::ptr::null()) };
    assert!(!raw.is_null());
    // SAFETY: the call returned a new handle nothing else owns.
    let mutex = unsafe { OwnedHandle::from_raw_handle(raw) };

    let copy = duplicate(&mutex, Access::SYNCHRONIZE);
    assert!(same_object(&mutex, &copy));
    assert_eq!(poll(&copy), WAIT_OBJECT_0, "an unowned mutex is signalled");
}

#[test]
fn the_source_may_be_borrowed_or_owned_or_a_duplicate() {
    let event = manual_event(false);
    let duplicated: OwnedHandle =
        duplicate(&event, Access::EVENT_MODIFY_STATE | Access::SYNCHRONIZE);

    let from_owned = duplicate(&event, Access::SYNCHRONIZE);
    let from_borrowed = duplicate(event.as_handle(), Access::SYNCHRONIZE);
    let from_duplicate = duplicate(&duplicated, Access::SYNCHRONIZE);
    for copy in [&from_owned, &from_borrowed, &from_duplicate] {
        assert!(same_object(copy, &event));
    }
}

#[test]
fn the_named_rights_are_the_win32_ones() {
    assert_eq!(Access::SYNCHRONIZE.bits(), SYNCHRONIZATION_SYNCHRONIZE);
    assert_eq!(Access::EVENT_MODIFY_STATE.bits(), EVENT_MODIFY_STATE);
    assert_eq!(
        Access::SYNCHRONIZE.bits() & Access::EVENT_MODIFY_STATE.bits(),
        0,
        "the two rights are distinct bits"
    );
}

#[test]
fn access_masks_combine_and_round_trip() {
    let both = Access::EVENT_MODIFY_STATE | Access::SYNCHRONIZE;
    assert_eq!(
        both.bits(),
        EVENT_MODIFY_STATE | SYNCHRONIZATION_SYNCHRONIZE
    );
    assert_eq!(Access::from_bits(both.bits()), both);
    assert_eq!(Access::from_bits(0).bits(), 0);
    assert_eq!(both | both, both, "combining is a union");
    assert_ne!(both, Access::SYNCHRONIZE);
}

#[test]
fn access_is_copy_send_and_sync() {
    fn assert_traits<T: Copy + Send + Sync + std::fmt::Debug + Eq + std::hash::Hash>() {}
    assert_traits::<Access>();
}
