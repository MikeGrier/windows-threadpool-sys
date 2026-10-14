// Copyright (c) 2026 Mike Grier
//! The Windows backend's notification, waited on (DI-7.3): the `Waitable` a `CompletionEvent` is.

use std::time::{Duration, Instant};

use win_sync_sys::{Access, Event, ResetMode, duplicate_handle_with_access};

use super::CompletionEvent;
use durable_ioring::Waitable;

const BOUND: Duration = Duration::from_secs(5);

/// An event, and the `CompletionEvent` a backend would hand out for it.
fn event(reset: ResetMode, signalled: bool) -> (Event, CompletionEvent) {
    let event = Event::new(reset, signalled).expect("an event");
    let handle = CompletionEvent::new(event.try_clone().expect("a duplicate").into());
    (event, handle)
}

#[test]
fn a_signalled_notification_is_seen_once_and_consumed() {
    let (_event, mut handle) = event(ResetMode::Auto, true);
    assert!(handle.wait(Some(BOUND)).expect("a wait"), "it was raised");
    assert!(
        !handle.wait(Some(Duration::ZERO)).expect("a wait"),
        "the wait took the signal, which is one edge"
    );
}

#[test]
fn an_unsignalled_notification_times_out() {
    let (_event, mut handle) = event(ResetMode::Auto, false);
    let timeout = Duration::from_millis(100);
    let started = Instant::now();
    assert!(!handle.wait(Some(timeout)).expect("a wait"));
    // A timer may expire a tick early against a high-resolution clock.
    assert!(
        started.elapsed() >= timeout - Duration::from_millis(40),
        "returned after {:?}",
        started.elapsed()
    );
}

#[test]
fn a_zero_timeout_asks_and_does_not_wait() {
    let (event, mut handle) = event(ResetMode::Auto, false);
    assert!(!handle.wait(Some(Duration::ZERO)).expect("a poll"));
    event.set().expect("set");
    assert!(
        handle.wait(Some(Duration::ZERO)).expect("a poll"),
        "no timing window decides this"
    );
}

#[test]
fn a_wait_that_fails_is_reported_and_not_taken_for_a_timeout() {
    // A handle without `SYNCHRONIZE` access cannot be waited on, and `win-sync-sys` makes one
    // without `unsafe`: a duplicate that may only set and reset.
    let event = Event::new(ResetMode::Auto, true).expect("an event");
    let blind = duplicate_handle_with_access(&event, Access::EVENT_MODIFY_STATE)
        .expect("a duplicate that cannot be waited on");
    let mut handle = CompletionEvent::new(blind);

    let error = handle
        .wait(Some(Duration::ZERO))
        .expect_err("the wait cannot be made, so it is not a timeout");

    assert_eq!(
        error.kind(),
        std::io::ErrorKind::PermissionDenied,
        "{error}"
    );
}

#[test]
fn a_raising_from_another_thread_wakes_a_wait_with_no_limit() {
    let (event, mut handle) = event(ResetMode::Auto, false);
    let setter = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        event.set().expect("set");
    });
    assert!(handle.wait(None).expect("a wait"));
    setter.join().expect("the setter");
}
