// Copyright (c) 2026 Mike Grier
use core::mem;

use windows_sys::Win32::System::Threading::{
    TP_CALLBACK_ENVIRON_V3, TP_CALLBACK_PRIORITY_HIGH, TP_CALLBACK_PRIORITY_LOW,
    TP_CALLBACK_PRIORITY_NORMAL,
};

use crate::callback_env::CallbackEnviron;

fn inner(env: &CallbackEnviron) -> &TP_CALLBACK_ENVIRON_V3 {
    env.as_inner()
}

// --- new() field defaults (10 normal cases) ---

#[test]
fn new_version_is_3() {
    assert_eq!(inner(&CallbackEnviron::new()).Version, 3);
}

#[test]
fn new_priority_is_normal() {
    assert_eq!(
        inner(&CallbackEnviron::new()).CallbackPriority,
        TP_CALLBACK_PRIORITY_NORMAL,
    );
}

#[test]
fn new_size_is_sizeof_struct() {
    assert_eq!(
        inner(&CallbackEnviron::new()).Size,
        mem::size_of::<TP_CALLBACK_ENVIRON_V3>() as u32,
    );
}

#[test]
fn new_pool_is_zero() {
    assert_eq!(inner(&CallbackEnviron::new()).Pool, 0);
}

#[test]
fn new_cleanup_group_is_zero() {
    assert_eq!(inner(&CallbackEnviron::new()).CleanupGroup, 0);
}

#[test]
fn new_cleanup_group_cancel_callback_is_none() {
    assert!(
        inner(&CallbackEnviron::new())
            .CleanupGroupCancelCallback
            .is_none()
    );
}

#[test]
fn new_race_dll_is_null() {
    assert!(inner(&CallbackEnviron::new()).RaceDll.is_null());
}

#[test]
fn new_activation_context_is_zero() {
    assert_eq!(inner(&CallbackEnviron::new()).ActivationContext, 0);
}

#[test]
fn new_finalization_callback_is_none() {
    assert!(
        inner(&CallbackEnviron::new())
            .FinalizationCallback
            .is_none()
    );
}

#[test]
fn new_flags_are_zero() {
    // SAFETY: Flags and s._bitfield alias the same u32.
    let flags = unsafe { inner(&CallbackEnviron::new()).u.Flags };
    assert_eq!(flags, 0);
}

// --- default() ---

#[test]
fn default_matches_new() {
    let a = CallbackEnviron::new();
    let b = CallbackEnviron::default();
    let (a, b) = (inner(&a), inner(&b));
    assert_eq!(a.Version, b.Version);
    assert_eq!(a.Pool, b.Pool);
    assert_eq!(a.CleanupGroup, b.CleanupGroup);
    assert_eq!(a.CallbackPriority, b.CallbackPriority);
    assert_eq!(a.Size, b.Size);
    assert_eq!(a.ActivationContext, b.ActivationContext);
    // SAFETY: both unions use the same layout.
    assert_eq!(unsafe { a.u.Flags }, unsafe { b.u.Flags });
}

// --- set_pool ---
//
// `set_pool` now takes an owned `ThreadpoolPool`, so it cannot be handed an
// invented value. Its behaviour is covered in the pool module's own tests, next
// to the type that makes it sound.

#[test]
fn clear_pool_leaves_the_default_pool() {
    let mut env = CallbackEnviron::new();
    env.clear_pool();
    assert_eq!(inner(&env).Pool, 0);
}

#[test]
fn clear_pool_does_not_alter_priority_or_size() {
    let mut env = CallbackEnviron::new();
    env.clear_pool();
    assert_eq!(inner(&env).CallbackPriority, TP_CALLBACK_PRIORITY_NORMAL);
    assert_eq!(
        inner(&env).Size,
        mem::size_of::<TP_CALLBACK_ENVIRON_V3>() as u32,
    );
}

// --- set_cleanup_group ---
//
// The values below are never handed to the thread pool; these tests only check
// that the setter records what it was given. `set_cleanup_group` is `unsafe`
// precisely because a real call would have the pool dereference them.

#[test]
fn set_cleanup_group_none_callback() {
    let mut env = CallbackEnviron::new();
    // SAFETY: the environment is never used to create an object, so the group
    // value is only stored and read back, never dereferenced by the pool.
    unsafe { env.set_cleanup_group(99, None) };
    assert_eq!(inner(&env).CleanupGroup, 99);
    assert!(inner(&env).CleanupGroupCancelCallback.is_none());
}

#[test]
fn set_cleanup_group_with_callback() {
    unsafe extern "system" fn cancel_cb(
        _obj: *mut core::ffi::c_void,
        _ctx: *mut core::ffi::c_void,
    ) {
    }

    let mut env = CallbackEnviron::new();
    // SAFETY: as above, the environment never reaches an object creation call.
    unsafe { env.set_cleanup_group(7, Some(cancel_cb)) };
    assert_eq!(inner(&env).CleanupGroup, 7);
    assert!(inner(&env).CleanupGroupCancelCallback.is_some());
}

#[test]
fn set_cleanup_group_zero_clears() {
    let mut env = CallbackEnviron::new();
    // SAFETY: as above, the environment never reaches an object creation call.
    unsafe {
        env.set_cleanup_group(55, None);
        env.set_cleanup_group(0, None);
    }
    assert_eq!(inner(&env).CleanupGroup, 0);
}
// --- set_priority ---

#[test]
fn set_priority_high() {
    let mut env = CallbackEnviron::new();
    env.set_priority(TP_CALLBACK_PRIORITY_HIGH);
    assert_eq!(inner(&env).CallbackPriority, TP_CALLBACK_PRIORITY_HIGH);
}

#[test]
fn set_priority_low() {
    let mut env = CallbackEnviron::new();
    env.set_priority(TP_CALLBACK_PRIORITY_LOW);
    assert_eq!(inner(&env).CallbackPriority, TP_CALLBACK_PRIORITY_LOW);
}

#[test]
fn set_priority_normal_is_idempotent() {
    let mut env = CallbackEnviron::new();
    env.set_priority(TP_CALLBACK_PRIORITY_NORMAL);
    assert_eq!(inner(&env).CallbackPriority, TP_CALLBACK_PRIORITY_NORMAL);
}

#[test]
fn set_priority_round_trip() {
    let mut env = CallbackEnviron::new();
    env.set_priority(TP_CALLBACK_PRIORITY_HIGH);
    env.set_priority(TP_CALLBACK_PRIORITY_NORMAL);
    assert_eq!(inner(&env).CallbackPriority, TP_CALLBACK_PRIORITY_NORMAL);
}

// --- set_runs_long ---

#[test]
fn set_runs_long_sets_bit_zero() {
    let mut env = CallbackEnviron::new();
    env.set_runs_long();
    // SAFETY: Flags aliases s._bitfield as u32.
    assert_eq!(unsafe { inner(&env).u.Flags } & 1, 1);
}

#[test]
fn set_runs_long_is_idempotent() {
    let mut env = CallbackEnviron::new();
    env.set_runs_long();
    env.set_runs_long();
    // SAFETY: Flags aliases s._bitfield as u32.
    assert_eq!(unsafe { inner(&env).u.Flags }, 1);
}

#[test]
fn set_runs_long_preserves_version_size_priority() {
    let mut env = CallbackEnviron::new();
    env.set_runs_long();
    let i = inner(&env);
    assert_eq!(i.Version, 3);
    assert_eq!(i.Size, mem::size_of::<TP_CALLBACK_ENVIRON_V3>() as u32);
    assert_eq!(i.CallbackPriority, TP_CALLBACK_PRIORITY_NORMAL);
}

// --- set_library ---

#[test]
fn set_library_stores_pointer() {
    let mut env = CallbackEnviron::new();
    let fake_dll = 0xDEAD_BEEF_usize as *mut core::ffi::c_void;
    // SAFETY: pointer is not dereferenced; only stored.
    unsafe { env.set_library(fake_dll) };
    assert_eq!(inner(&env).RaceDll, fake_dll);
}

#[test]
fn set_library_null_stores_null() {
    let mut env = CallbackEnviron::new();
    // SAFETY: null pointer, only stored.
    unsafe { env.set_library(core::ptr::null_mut()) };
    assert!(inner(&env).RaceDll.is_null());
}

// --- as_mut_ptr ---

#[test]
fn as_mut_ptr_is_nonnull() {
    let mut env = CallbackEnviron::new();
    assert!(!env.as_mut_ptr().is_null());
}

#[test]
fn as_mut_ptr_points_to_correct_version() {
    let mut env = CallbackEnviron::new();
    let ptr = env.as_mut_ptr();
    // SAFETY: ptr is valid for the lifetime of env.
    assert_eq!(unsafe { (*ptr).Version }, 3);
}
