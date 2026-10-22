// Copyright (c) 2026 Mike Grier
//! Unit tests for `get_handle_information`, `set_handle_information` and `HandleFlags`.
//!
//! The query's tests set and clear the flags through raw `SetHandleInformation`, and the setter's read them back through
//! raw `GetHandleInformation`, never through anything this crate offers, so a defect in either cannot hide behind the other.

use std::fs::File;
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle};

use windows_sys::Win32::Foundation::{
    FALSE, GetHandleInformation, HANDLE_FLAG_INHERIT, HANDLE_FLAG_PROTECT_FROM_CLOSE,
    INVALID_HANDLE_VALUE, SetHandleInformation, TRUE,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::System::Threading::CreateEventW;

use super::{HandleFlags, get_handle_information, set_handle_information};
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

/// The flags `handle` carries, through the raw call.
fn raw_flags(handle: &impl AsRawHandle) -> u32 {
    let mut flags = 0u32;
    // SAFETY: the handle is open for the duration of the borrow and `flags` is a valid out pointer.
    let ok = unsafe { GetHandleInformation(handle.as_raw_handle(), &mut flags) };
    assert_ne!(ok, FALSE, "read the flags");
    flags
}

/// Clears the protection when dropped, so a failing assertion does not leave a handle whose close is refused.
struct ClearsProtection<'a>(&'a OwnedHandle);

impl Drop for ClearsProtection<'_> {
    fn drop(&mut self) {
        set_flags(self.0, HANDLE_FLAG_PROTECT_FROM_CLOSE, 0);
    }
}

const BOTH: u32 = HANDLE_FLAG_INHERIT | HANDLE_FLAG_PROTECT_FROM_CLOSE;

#[test]
fn a_flag_in_the_mask_and_in_flags_is_set() {
    let handle = event(false);

    set_handle_information(&handle, HandleFlags::INHERIT, HandleFlags::INHERIT).expect("set");

    assert_eq!(raw_flags(&handle), HANDLE_FLAG_INHERIT);
}

#[test]
fn a_flag_in_the_mask_and_not_in_flags_is_cleared() {
    let handle = event(true);

    set_handle_information(&handle, HandleFlags::INHERIT, HandleFlags::NONE).expect("cleared");

    assert_eq!(raw_flags(&handle), 0);
}

#[test]
fn a_flag_in_flags_and_not_in_the_mask_is_left_as_it_was() {
    let handle = event(false);
    let _clears = ClearsProtection(&handle);
    let both = HandleFlags::INHERIT | HandleFlags::PROTECT_FROM_CLOSE;

    set_handle_information(&handle, HandleFlags::NONE, both).expect("an empty mask");
    assert_eq!(raw_flags(&handle), 0, "nothing changes with an empty mask");

    set_handle_information(&handle, HandleFlags::INHERIT, both).expect("one flag in the mask");
    assert_eq!(
        raw_flags(&handle),
        HANDLE_FLAG_INHERIT,
        "only the flag in the mask is set"
    );
}

#[test]
fn a_flag_the_handle_carries_that_is_not_in_the_mask_is_left_alone() {
    let handle = event(true);
    let _clears = ClearsProtection(&handle);
    set_flags(
        &handle,
        HANDLE_FLAG_PROTECT_FROM_CLOSE,
        HANDLE_FLAG_PROTECT_FROM_CLOSE,
    );
    assert_eq!(raw_flags(&handle), BOTH);

    set_handle_information(&handle, HandleFlags::INHERIT, HandleFlags::NONE).expect("clear one");

    assert_eq!(
        raw_flags(&handle),
        HANDLE_FLAG_PROTECT_FROM_CLOSE,
        "the protection stays"
    );
}

#[test]
fn every_start_mask_and_flags_give_the_result_the_pages_mask_rule_says() {
    // The page: "To set or clear the associated bit flag in dwFlags, you must set a change mask bit flag in dwMask".
    // The result is what stays of the start, outside the mask, with what `flags` says inside it.
    for start in 0..=3u32 {
        for mask in 0..=3u32 {
            for flags in 0..=3u32 {
                let handle = event(false);
                let _clears = ClearsProtection(&handle);
                set_flags(&handle, BOTH, start);

                set_handle_information(
                    &handle,
                    HandleFlags::from_bits(mask),
                    HandleFlags::from_bits(flags),
                )
                .expect("set");

                let expected = (start & !mask) | (flags & mask);
                assert_eq!(
                    raw_flags(&handle) & BOTH,
                    expected,
                    "start {start}, mask {mask}, flags {flags}"
                );
            }
        }
    }
}

#[test]
fn a_set_flag_is_read_back_by_the_query() {
    let handle = event(false);
    let _clears = ClearsProtection(&handle);

    set_handle_information(&handle, HandleFlags::INHERIT, HandleFlags::INHERIT).expect("set");
    assert_eq!(
        get_handle_information(&handle).expect("query"),
        HandleFlags::INHERIT
    );

    set_handle_information(&handle, BOTH_FLAGS, BOTH_FLAGS).expect("both");
    assert_eq!(get_handle_information(&handle).expect("query"), BOTH_FLAGS);

    set_handle_information(&handle, BOTH_FLAGS, HandleFlags::NONE).expect("neither");
    assert_eq!(
        get_handle_information(&handle).expect("query"),
        HandleFlags::NONE
    );
}

const BOTH_FLAGS: HandleFlags = HandleFlags::from_bits(BOTH);

#[test]
fn a_handle_std_opens_can_be_made_inheritable_and_made_not_so_again() {
    let path = std::env::temp_dir().join(format!("wh-set-test-{}.tmp", std::process::id()));
    let file = File::create(&path).expect("create");
    assert_eq!(raw_flags(&file), 0);

    set_handle_information(&file, HandleFlags::INHERIT, HandleFlags::INHERIT).expect("set");
    let inheritable = raw_flags(&file);
    set_handle_information(&file, HandleFlags::INHERIT, HandleFlags::NONE).expect("cleared");
    let cleared = raw_flags(&file);

    drop(file);
    std::fs::remove_file(&path).expect("remove");
    assert_eq!((inheritable, cleared), (HANDLE_FLAG_INHERIT, 0));
}

#[test]
fn the_handle_may_be_borrowed_owned_or_a_reference() {
    let handle = event(false);

    set_handle_information(&handle, HandleFlags::INHERIT, HandleFlags::INHERIT).expect("reference");
    assert_eq!(raw_flags(&handle), HANDLE_FLAG_INHERIT);

    set_handle_information(handle.as_handle(), HandleFlags::INHERIT, HandleFlags::NONE)
        .expect("borrow");
    assert_eq!(raw_flags(&handle), 0);

    // An owned handle is dropped by the call, so what is shown is that it is accepted.
    set_handle_information(handle, HandleFlags::INHERIT, HandleFlags::INHERIT).expect("value");
}

#[test]
fn flags_belong_to_the_handle_and_not_to_its_object() {
    // Measured, 2026-10-21 (WH-D-3), and not a promise of the page, which speaks of "the object handle".
    let source = event(false);
    let copy = duplicate_handle_with_access(&source, Access::SYNCHRONIZE).expect("duplicate");

    set_handle_information(&copy, HandleFlags::INHERIT, HandleFlags::INHERIT).expect("the copy");
    assert_eq!(raw_flags(&copy), HANDLE_FLAG_INHERIT);
    assert_eq!(raw_flags(&source), 0, "the source is not changed");

    set_handle_information(&source, HandleFlags::INHERIT, HandleFlags::INHERIT)
        .expect("the source");
    set_handle_information(&copy, HandleFlags::INHERIT, HandleFlags::NONE).expect("clear the copy");
    assert_eq!(
        raw_flags(&source),
        HANDLE_FLAG_INHERIT,
        "nor is it by the copy"
    );
}

#[test]
fn a_bit_windows_does_not_name_is_not_refused_and_changes_no_named_flag() {
    // Measured, 2026-10-21 (WH-D-3), and the call's to judge: the page names two flags and calls no other bit invalid.
    let handle = event(true);
    let _clears = ClearsProtection(&handle);
    let unnamed = HandleFlags::from_bits(0x8000_0000);

    set_handle_information(&handle, unnamed, unnamed)
        .expect("an unnamed bit in the mask and flags");
    assert_eq!(
        raw_flags(&handle),
        HANDLE_FLAG_INHERIT,
        "nothing named changed"
    );

    set_handle_information(&handle, HandleFlags::INHERIT, unnamed)
        .expect("an unnamed bit in flags");
    assert_eq!(
        raw_flags(&handle),
        0,
        "the named flag took its value from the flags"
    );

    let every_bit = HandleFlags::from_bits(u32::MAX);
    set_handle_information(&handle, every_bit, every_bit).expect("every bit");
    assert_eq!(raw_flags(&handle) & BOTH, BOTH);
}

#[test]
fn a_handle_the_call_refuses_is_its_error() {
    // The current process's pseudo-handle, which `std` allows a `BorrowedHandle` to hold. Measured, 2026-10-21 (WH-D-3):
    // refused with `ERROR_ACCESS_DENIED`. The page does not say it is.
    // SAFETY: `INVALID_HANDLE_VALUE` is the one value besides an open handle that `BorrowedHandle::borrow_raw` allows.
    let pseudo = unsafe { BorrowedHandle::borrow_raw(INVALID_HANDLE_VALUE) };

    let error = set_handle_information(pseudo, HandleFlags::INHERIT, HandleFlags::INHERIT)
        .expect_err("a pseudo-handle is refused");

    assert!(error.raw_os_error().is_some(), "{error}");
}
