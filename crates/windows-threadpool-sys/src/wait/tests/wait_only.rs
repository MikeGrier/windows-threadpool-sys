// Copyright (c) 2026 Mike Grier
//! Tests for waits built from wait-only event handles.

use std::os::windows::io::AsRawHandle;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use super::{Activations, ThreadpoolWait, WaitResult, WaitableHandle, same_object};
use win_sync_sys::{Event, ResetMode};
use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, FALSE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::{ResetEvent, SetEvent, WaitForSingleObject};
use windows_sys::core::BOOL;

#[test]
fn conversion_shares_the_wait_only_handle_for_the_same_event() {
    let event = Event::new(ResetMode::Manual, false).expect("create an event");
    let wait_only = event
        .duplicate_wait_only()
        .expect("duplicate a wait-only handle");
    let kept = wait_only.clone();
    let waitable = WaitableHandle::from(wait_only);
    let raw = waitable.handle().as_raw_handle();

    assert_eq!(raw, kept.as_raw_handle(), "the wait shares its handle");
    assert!(
        same_object(waitable.handle(), &event),
        "it is the same event"
    );
    assert_ne!(
        raw,
        event.as_raw_handle(),
        "the wait-only handle is distinct from the Event handle"
    );
}

#[test]
fn a_wait_built_from_wait_only_event_fires_when_the_owner_sets_the_event() {
    let event = Event::new(ResetMode::Manual, false).expect("create an event");
    let waitable = event
        .duplicate_wait_only()
        .expect("duplicate a wait-only handle")
        .into();
    let seen = Activations::new();
    let recorder = Arc::clone(&seen);
    let wait = ThreadpoolWait::new(
        waitable,
        move |activation| recorder.record(activation.result()),
        None,
    )
    .expect("create wait");

    wait.arm(None);
    event.set().expect("set through the owning Event");
    let results = seen.wait_for(1);
    wait.stop_and_drain();
    assert_eq!(results, vec![WaitResult::Signalled]);
}

#[test]
fn a_pre_signalled_manual_reset_wait_only_event_fires_when_armed() {
    let event = Event::new(ResetMode::Manual, true).expect("create a signalled event");
    let waitable = event
        .duplicate_wait_only()
        .expect("duplicate a wait-only handle")
        .into();
    let seen = Activations::new();
    let recorder = Arc::clone(&seen);
    let wait = ThreadpoolWait::new(
        waitable,
        move |activation| recorder.record(activation.result()),
        None,
    )
    .expect("create wait");

    wait.arm(None);
    let results = seen.wait_for(1);
    wait.stop_and_drain();

    assert_eq!(results, vec![WaitResult::Signalled]);
    assert_eq!(
        // SAFETY: the owner keeps the event handle open.
        unsafe { WaitForSingleObject(event.as_raw_handle(), 0) },
        WAIT_OBJECT_0,
        "arming observes the existing manual-reset signal"
    );
}

#[test]
fn an_auto_reset_wait_only_event_consumes_the_owner_signal() {
    let event = Event::new(ResetMode::Auto, false).expect("create an event");
    let waitable = event
        .duplicate_wait_only()
        .expect("duplicate a wait-only handle")
        .into();
    let seen = Activations::new();
    let recorder = Arc::clone(&seen);
    let wait = ThreadpoolWait::new(
        waitable,
        move |activation| recorder.record(activation.result()),
        None,
    )
    .expect("create wait");

    wait.arm(None);
    event.set().expect("set through the owning Event");
    let results = seen.wait_for(1);
    wait.stop_and_drain();

    assert_eq!(results, vec![WaitResult::Signalled]);
    assert_eq!(
        // SAFETY: the owner keeps the event handle open.
        unsafe { WaitForSingleObject(event.as_raw_handle(), 0) },
        WAIT_TIMEOUT,
        "the wait consumes the auto-reset signal"
    );
}

#[test]
fn a_callback_can_reset_through_the_owner_before_rearming() {
    let event = Event::new(ResetMode::Manual, false).expect("create an event");
    let owner = event.clone();
    let waitable = event
        .duplicate_wait_only()
        .expect("duplicate a wait-only handle")
        .into();
    let seen = Activations::new();
    let recorder = Arc::clone(&seen);
    let wait = ThreadpoolWait::new(
        waitable,
        move |activation| {
            owner.reset().expect("reset through the owning Event");
            activation.rearm(None);
            recorder.record(activation.result());
        },
        None,
    )
    .expect("create wait");

    wait.arm(None);
    event.set().expect("set the first signal");
    let first = seen.wait_for(1);
    event
        .set()
        .expect("set the second signal after the callback reset");
    let results = seen.wait_for(2);
    wait.stop_and_drain();

    assert_eq!(first, vec![WaitResult::Signalled]);
    assert_eq!(results, vec![WaitResult::Signalled; 2]);
}

#[test]
fn rearming_a_still_signalled_wait_only_event_activates_again_immediately() {
    let event = Event::new(ResetMode::Manual, true).expect("create a signalled event");
    let waitable = event
        .duplicate_wait_only()
        .expect("duplicate a wait-only handle")
        .into();
    let seen = Activations::new();
    let recorder = Arc::clone(&seen);
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let callback_calls = Arc::clone(&calls);
    let wait = ThreadpoolWait::new(
        waitable,
        move |activation| {
            if callback_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                activation.rearm(None);
            }
            recorder.record(activation.result());
        },
        None,
    )
    .expect("create wait");

    wait.arm(None);
    let results = seen.wait_for(2);
    wait.stop_and_drain();

    assert_eq!(results, vec![WaitResult::Signalled; 2]);
}

fn assert_access_denied(result: BOOL, error: Option<i32>, operation: &str) {
    assert_eq!(result, FALSE, "{operation} must fail");
    assert_eq!(
        error,
        Some(ERROR_ACCESS_DENIED as i32),
        "{operation} must fail for lack of access"
    );
}

#[test]
fn the_wait_and_activation_handles_cannot_set_or_reset_the_event() {
    let event = Event::new(ResetMode::Manual, false).expect("create an event");
    let waitable = event
        .duplicate_wait_only()
        .expect("duplicate a wait-only handle")
        .into();
    let (sender, receiver) = mpsc::channel();
    let wait = ThreadpoolWait::new(
        waitable,
        move |activation| {
            let handle = activation.handle();
            let raw = handle.as_raw_handle();
            // SAFETY: the activation lends a live event handle.
            let set_result = unsafe { SetEvent(raw) };
            let set_error = std::io::Error::last_os_error().raw_os_error();
            // SAFETY: the activation lends a live event handle.
            let reset_result = unsafe { ResetEvent(raw) };
            let reset_error = std::io::Error::last_os_error().raw_os_error();
            let _ = sender.send((set_result, set_error, reset_result, reset_error));
        },
        None,
    )
    .expect("create wait");

    // SAFETY: the wait lends a live event handle.
    let set_result = unsafe { SetEvent(wait.handle().as_raw_handle()) };
    let set_error = std::io::Error::last_os_error().raw_os_error();
    assert_access_denied(set_result, set_error, "SetEvent on WaitableHandle::handle");
    // SAFETY: the wait lends a live event handle.
    let reset_result = unsafe { ResetEvent(wait.handle().as_raw_handle()) };
    let reset_error = std::io::Error::last_os_error().raw_os_error();
    assert_access_denied(
        reset_result,
        reset_error,
        "ResetEvent on WaitableHandle::handle",
    );

    wait.arm(None);
    event.set().expect("set through the owning Event");
    let (set_result, set_error, reset_result, reset_error) = receiver
        .recv_timeout(Duration::from_secs(30))
        .expect("wait-only activation arrives");
    wait.stop_and_drain();

    assert_access_denied(set_result, set_error, "SetEvent on WaitActivation::handle");
    assert_access_denied(
        reset_result,
        reset_error,
        "ResetEvent on WaitActivation::handle",
    );
}

#[test]
fn a_wait_built_from_event_retains_set_and_reset_access() {
    let event = Event::new(ResetMode::Manual, false).expect("create an event");
    let waitable = WaitableHandle::from(event.clone());
    let (sender, receiver) = mpsc::channel();
    let wait = ThreadpoolWait::new(
        waitable,
        move |activation| {
            let raw = activation.handle().as_raw_handle();
            // SAFETY: the Event-backed activation lends a live event handle.
            let set_result = unsafe { SetEvent(raw) };
            // SAFETY: the Event-backed activation lends a live event handle.
            let reset_result = unsafe { ResetEvent(raw) };
            let _ = sender.send((set_result, reset_result));
        },
        None,
    )
    .expect("create wait");

    // SAFETY: the wait lends a live event handle.
    assert_ne!(unsafe { SetEvent(wait.handle().as_raw_handle()) }, FALSE);
    // SAFETY: the wait lends a live event handle.
    assert_ne!(unsafe { ResetEvent(wait.handle().as_raw_handle()) }, FALSE);
    wait.arm(None);
    event.set().expect("signal the event");
    let (set_result, reset_result) = receiver
        .recv_timeout(Duration::from_secs(30))
        .expect("Event-backed activation arrives");
    wait.stop_and_drain();

    assert_ne!(set_result, FALSE, "activation handle can set the Event");
    assert_ne!(reset_result, FALSE, "activation handle can reset the Event");
}

#[test]
fn the_wait_keeps_a_wait_only_handle_open_after_other_holders_drop() {
    let event = Event::new(ResetMode::Manual, true).expect("create a signalled event");
    let wait_only = event
        .duplicate_wait_only()
        .expect("duplicate a wait-only handle");
    let other = wait_only.clone();
    let seen = Activations::new();
    let recorder = Arc::clone(&seen);
    let wait = ThreadpoolWait::new(
        WaitableHandle::from(wait_only),
        move |activation| recorder.record(activation.result()),
        None,
    )
    .expect("create wait");
    drop(other);
    drop(event);

    assert_eq!(
        // SAFETY: the wait keeps this borrowed handle open.
        unsafe { WaitForSingleObject(wait.handle().as_raw_handle(), 0) },
        WAIT_OBJECT_0,
        "the wait's handle remains valid after other holders drop"
    );
    wait.arm(None);
    let results = seen.wait_for(1);
    wait.stop_and_drain();
    assert_eq!(results, vec![WaitResult::Signalled]);
}

#[test]
fn into_handle_returns_the_wait_only_handle_when_it_is_the_last_holder() {
    let event = Event::new(ResetMode::Manual, false).expect("create an event");
    let wait_only = event
        .duplicate_wait_only()
        .expect("duplicate a wait-only handle");
    let other = wait_only.clone();
    let waitable = WaitableHandle::from(wait_only);
    let raw = waitable.handle().as_raw_handle();
    assert_eq!(raw, other.as_raw_handle(), "the wait shares the handle");

    let waitable = waitable
        .into_handle()
        .expect_err("the other wait-only clone still holds the handle");
    drop(other);
    drop(event);
    let handle = waitable
        .into_handle()
        .expect("the wait target is now the last holder");
    assert_eq!(
        // SAFETY: the returned OwnedHandle keeps the event handle open.
        unsafe { WaitForSingleObject(handle.as_raw_handle(), 0) },
        WAIT_TIMEOUT,
        "into_handle returns a still-open, unsignalled event handle"
    );
}
