// Copyright (c) 2026 Mike Grier
//! Unit tests for `get_handle_information` and `HandleFlags`.
//!
//! The flags a handle carries are set and cleared through raw `SetHandleInformation`, never through
//! anything this crate offers, so a defect in the query cannot hide behind the thing that set them.

use std::fs::File;
use std::os::windows::io::{AsHandle, AsRawHandle, FromRawHandle, OwnedHandle};

use windows_sys::Win32::Foundation::{
    FALSE, HANDLE_FLAG_INHERIT, HANDLE_FLAG_PROTECT_FROM_CLOSE, SetHandleInformation, TRUE,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::System::Threading::CreateEventW;

use super::{HandleFlags, get_handle_information};
use crate::{Access, duplicate_handle_with_access};

/// A manual-reset event, inheritable or not.
fn event(inheritable: bool) -> OwnedHandle {
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: std::ptr::null_mut(),
        bInheritHandle: if inheritable { TRUE } else { FALSE },
    };
    // SAFETY: the attributes live across the call, and no name is given.
    let raw = unsafe { CreateEventW(&attributes, 1, 0, std::ptr::null()) };
    assert!(!raw.is_null(), "create an event");
    // SAFETY: the call returned a new handle nothing else owns.
    unsafe { OwnedHandle::from_raw_handle(raw) }
}

/// Sets the bits of `mask` to `value`'s bits on `handle`, through the raw call.
fn set_flags(handle: &impl AsRawHandle, mask: u32, value: u32) {
    // SAFETY: the handle is open for the duration of the borrow.
    let ok = unsafe { SetHandleInformation(handle.as_raw_handle(), mask, value) };
    assert_ne!(ok, FALSE, "set the flags");
}

/// Protects a handle from close and clears the protection when dropped, so a failing assertion does
/// not leave a handle whose close is refused.
struct Protected<'a>(&'a OwnedHandle);

impl<'a> Protected<'a> {
    fn new(handle: &'a OwnedHandle) -> Self {
        set_flags(
            handle,
            HANDLE_FLAG_PROTECT_FROM_CLOSE,
            HANDLE_FLAG_PROTECT_FROM_CLOSE,
        );
        Self(handle)
    }
}

impl Drop for Protected<'_> {
    fn drop(&mut self) {
        set_flags(self.0, HANDLE_FLAG_PROTECT_FROM_CLOSE, 0);
    }
}

#[test]
fn a_handle_made_without_inheritance_carries_no_flag() {
    let handle = event(false);
    assert_eq!(
        get_handle_information(&handle).expect("query"),
        HandleFlags::NONE
    );
}

#[test]
fn a_handle_made_inheritable_carries_the_inherit_flag_and_only_it() {
    let handle = event(true);
    assert_eq!(
        get_handle_information(&handle).expect("query"),
        HandleFlags::INHERIT
    );
}

#[test]
fn the_inherit_flag_follows_the_handle_as_it_is_set_and_cleared() {
    let handle = event(false);
    set_flags(&handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT);
    assert_eq!(
        get_handle_information(&handle).expect("set"),
        HandleFlags::INHERIT
    );
    set_flags(&handle, HANDLE_FLAG_INHERIT, 0);
    assert_eq!(
        get_handle_information(&handle).expect("cleared"),
        HandleFlags::NONE
    );
}

#[test]
fn a_protected_handle_carries_the_protect_flag_and_only_it() {
    let handle = event(false);
    {
        let _protected = Protected::new(&handle);
        assert_eq!(
            get_handle_information(&handle).expect("protected"),
            HandleFlags::PROTECT_FROM_CLOSE
        );
    }
    assert_eq!(
        get_handle_information(&handle).expect("unprotected"),
        HandleFlags::NONE
    );
}

#[test]
fn a_handle_can_carry_both_flags_and_each_is_reported() {
    let handle = event(true);
    let _protected = Protected::new(&handle);

    let flags = get_handle_information(&handle).expect("query");

    assert_eq!(
        flags,
        HandleFlags::INHERIT | HandleFlags::PROTECT_FROM_CLOSE
    );
    assert!(flags.contains(HandleFlags::INHERIT));
    assert!(flags.contains(HandleFlags::PROTECT_FROM_CLOSE));
}

#[test]
fn a_duplicate_is_not_inheritable_even_of_a_source_that_is() {
    let source = event(true);
    assert!(
        get_handle_information(&source)
            .expect("the source")
            .contains(HandleFlags::INHERIT)
    );

    let copy = duplicate_handle_with_access(&source, Access::SYNCHRONIZE).expect("duplicate");

    assert_eq!(
        get_handle_information(&copy).expect("the copy"),
        HandleFlags::NONE
    );
}

#[test]
fn a_handle_std_opens_is_not_inheritable() {
    let path = std::env::temp_dir().join(format!("wh-info-test-{}.tmp", std::process::id()));
    let file = File::create(&path).expect("create");

    let flags = get_handle_information(&file).expect("query a file");

    drop(file);
    std::fs::remove_file(&path).expect("remove");
    assert_eq!(flags, HandleFlags::NONE);
}

#[test]
fn the_source_may_be_borrowed_owned_or_a_reference() {
    let handle = event(true);

    let by_reference = get_handle_information(&handle).expect("reference");
    let by_borrow = get_handle_information(handle.as_handle()).expect("borrow");
    let by_value = get_handle_information(handle).expect("value");

    assert_eq!(by_reference, HandleFlags::INHERIT);
    assert_eq!(by_borrow, by_reference);
    assert_eq!(by_value, by_reference);
}

#[test]
fn the_named_flags_are_the_win32_ones() {
    assert_eq!(HandleFlags::NONE.bits(), 0);
    assert_eq!(HandleFlags::INHERIT.bits(), HANDLE_FLAG_INHERIT);
    assert_eq!(
        HandleFlags::PROTECT_FROM_CLOSE.bits(),
        HANDLE_FLAG_PROTECT_FROM_CLOSE
    );
    assert_eq!(
        HandleFlags::INHERIT.bits() & HandleFlags::PROTECT_FROM_CLOSE.bits(),
        0,
        "the two flags are distinct bits"
    );
}

#[test]
fn flags_combine_contain_and_round_trip() {
    let both = HandleFlags::INHERIT | HandleFlags::PROTECT_FROM_CLOSE;
    assert_eq!(
        both.bits(),
        HANDLE_FLAG_INHERIT | HANDLE_FLAG_PROTECT_FROM_CLOSE
    );
    assert_eq!(HandleFlags::from_bits(both.bits()), both);
    assert_eq!(both | both, both, "combining is a union");
    assert!(
        both.contains(HandleFlags::NONE),
        "every set contains nothing"
    );
    assert!(both.contains(HandleFlags::INHERIT));
    assert!(!HandleFlags::INHERIT.contains(both));
    assert!(!HandleFlags::NONE.contains(HandleFlags::INHERIT));
    assert_eq!(HandleFlags::default(), HandleFlags::NONE);
}

#[test]
fn a_bit_windows_defines_that_is_not_named_here_is_kept() {
    let unnamed = HandleFlags::from_bits(0x8000_0000);
    assert_eq!(unnamed.bits(), 0x8000_0000);
    assert!(!unnamed.contains(HandleFlags::INHERIT));
    assert_eq!((unnamed | HandleFlags::INHERIT).bits(), 0x8000_0001);
}

#[test]
fn flags_are_copy_send_and_sync() {
    fn assert_traits<T: Copy + Send + Sync + std::fmt::Debug + Eq + std::hash::Hash + Default>() {}
    assert_traits::<HandleFlags>();
}
