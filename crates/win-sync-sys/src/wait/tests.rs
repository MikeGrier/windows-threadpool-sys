// Copyright (c) 2026 Mike Grier
//! Unit tests for `wait_for_single_object`.

use std::io;
use std::os::windows::io::{AsHandle, AsRawHandle, FromRawHandle, OwnedHandle};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    DuplicateHandle, ERROR_ACCESS_DENIED, ERROR_INVALID_HANDLE, FALSE, HANDLE, SetLastError,
    WAIT_ABANDONED, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::System::Threading::{
    CreateMutexW, EVENT_MODIFY_STATE, GetCurrentProcess, INFINITE, SYNCHRONIZATION_SYNCHRONIZE,
};

use super::{WaitOutcome, milliseconds, outcome, wait_for_single_object};
use crate::{Event, ResetMode};

/// How long a test waits for something that should happen.
const BOUND: Duration = Duration::from_secs(5);

/// What a wait for `timeout` must at least take, allowing for a timer that expires up to a tick
/// early against a high-resolution clock. Evidence of a timeout honoured, not proof of how
/// precisely.
const EARLY: Duration = Duration::from_millis(40);

fn auto(signalled: bool) -> Event {
    Event::new(ResetMode::Auto, signalled).expect("an event")
}

fn manual(signalled: bool) -> Event {
    Event::new(ResetMode::Manual, signalled).expect("an event")
}

fn wait(object: impl AsHandle, timeout: Option<Duration>) -> WaitOutcome {
    wait_for_single_object(object, timeout).expect("the wait is answered")
}

/// A second handle to `event` with only the given access.
fn duplicate_with(event: &Event, access: u32) -> OwnedHandle {
    let mut duplicate: HANDLE = std::ptr::null_mut();
    // SAFETY: both process handles are the current-process pseudo-handle, the source is open for
    // the borrow of `event`, and `duplicate` is a valid out pointer.
    let ok = unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            event.as_raw_handle(),
            GetCurrentProcess(),
            &mut duplicate,
            access,
            FALSE,
            0,
        )
    };
    assert_ne!(ok, FALSE, "DuplicateHandle: {}", io::Error::last_os_error());
    // SAFETY: the call returned a new handle that nothing else owns.
    unsafe { OwnedHandle::from_raw_handle(duplicate) }
}

/// A mutex whose owner has ended without releasing it.
fn abandoned_mutex() -> OwnedHandle {
    std::thread::spawn(|| {
        // SAFETY: an unnamed mutex with default security, initially owned by this thread, which
        // ends without releasing it.
        let raw = unsafe { CreateMutexW(std::ptr::null(), 1, std::ptr::null()) };
        assert!(
            !raw.is_null(),
            "CreateMutexW: {}",
            io::Error::last_os_error()
        );
        // SAFETY: the call returned a new handle that nothing else owns.
        unsafe { OwnedHandle::from_raw_handle(raw) }
    })
    .join()
    .expect("the owning thread")
}

#[test]
fn a_signalled_event_is_taken_at_once() {
    let event = manual(true);
    assert_eq!(wait(&event, Some(BOUND)), WaitOutcome::Signalled);
}

#[test]
fn a_wait_for_an_auto_reset_event_consumes_its_signal() {
    let event = auto(true);
    assert_eq!(wait(&event, Some(Duration::ZERO)), WaitOutcome::Signalled);
    assert_eq!(
        wait(&event, Some(Duration::ZERO)),
        WaitOutcome::TimedOut,
        "the first wait took it"
    );
}

#[test]
fn a_wait_for_a_manual_reset_event_leaves_it_signalled() {
    let event = manual(true);
    for _ in 0..3 {
        assert_eq!(wait(&event, Some(Duration::ZERO)), WaitOutcome::Signalled);
    }
}

#[test]
fn a_set_after_the_wait_began_is_seen() {
    let event = auto(false);
    let signaller = event.try_clone().expect("a second handle");
    let setter = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        signaller.set().expect("set");
    });
    // No limit: a timeout mistaken for zero would return TimedOut before the set.
    assert_eq!(wait(&event, None), WaitOutcome::Signalled);
    setter.join().expect("the setter");
}

#[test]
fn an_unsignalled_event_times_out_after_its_timeout() {
    let event = auto(false);
    let timeout = Duration::from_millis(100);
    let started = Instant::now();
    assert_eq!(wait(&event, Some(timeout)), WaitOutcome::TimedOut);
    let elapsed = started.elapsed();
    assert!(elapsed >= timeout - EARLY, "returned after {elapsed:?}");
    assert!(elapsed < BOUND, "returned after {elapsed:?}");
}

#[test]
fn a_zero_timeout_polls_and_does_not_wait() {
    let event = auto(false);
    let started = Instant::now();
    assert_eq!(wait(&event, Some(Duration::ZERO)), WaitOutcome::TimedOut);
    assert!(
        started.elapsed() < BOUND,
        "a poll took {:?}",
        started.elapsed()
    );
    event.set().expect("set");
    assert_eq!(
        wait(&event, Some(Duration::ZERO)),
        WaitOutcome::Signalled,
        "the same poll sees a signal, which no timing window decides"
    );
}

#[test]
fn any_handle_that_lends_itself_can_be_waited_on() {
    let event = manual(true);
    let owned: OwnedHandle = event.try_clone().expect("a clone").into();
    assert_eq!(wait(&owned, Some(BOUND)), WaitOutcome::Signalled);
    assert_eq!(wait(owned.as_handle(), Some(BOUND)), WaitOutcome::Signalled);
    assert_eq!(wait(event, Some(BOUND)), WaitOutcome::Signalled);
}

#[test]
fn a_mutex_whose_owner_ended_is_abandoned_once_and_then_owned() {
    let mutex = abandoned_mutex();
    assert_eq!(wait(&mutex, Some(BOUND)), WaitOutcome::Abandoned);
    // The waiting thread owns it now, and a mutex is recursive for its owner.
    assert_eq!(wait(&mutex, Some(BOUND)), WaitOutcome::Signalled);
}

#[test]
fn a_wait_that_fails_reports_the_system_error() {
    let event = auto(true);
    let no_wait_access = duplicate_with(&event, EVENT_MODIFY_STATE);
    let error = wait_for_single_object(&no_wait_access, Some(BOUND)).expect_err("cannot wait");
    assert_eq!(
        error.raw_os_error(),
        i32::try_from(ERROR_ACCESS_DENIED).ok()
    );
    // With the access it needs, the same object waits.
    let with = duplicate_with(&event, SYNCHRONIZATION_SYNCHRONIZE);
    assert_eq!(wait(&with, Some(BOUND)), WaitOutcome::Signalled);
}

#[test]
fn a_timeout_longer_than_the_call_can_express_is_refused_and_nothing_is_waited() {
    let event = auto(true);
    let too_long = Duration::from_millis(u64::from(u32::MAX));
    let error = wait_for_single_object(&event, Some(too_long)).expect_err("refused");
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    assert_eq!(
        wait(&event, Some(Duration::ZERO)),
        WaitOutcome::Signalled,
        "the refused wait did not take the signal"
    );
}

#[test]
fn a_timeout_is_converted_to_whole_milliseconds_rounded_up() {
    let cases: [(Option<Duration>, u32); 12] = [
        (None, INFINITE),
        (Some(Duration::ZERO), 0),
        (Some(Duration::from_nanos(1)), 1),
        (Some(Duration::from_nanos(999_999)), 1),
        (Some(Duration::from_millis(1)), 1),
        (Some(Duration::from_nanos(1_000_001)), 2),
        (Some(Duration::from_micros(1_500)), 2),
        (Some(Duration::from_millis(2)), 2),
        (Some(Duration::from_secs(1)), 1_000),
        (Some(Duration::from_secs(60 * 60)), 3_600_000),
        (
            Some(Duration::from_millis(u64::from(u32::MAX) - 1)),
            u32::MAX - 1,
        ),
        (
            Some(Duration::from_nanos(4_294_967_293_000_001)),
            u32::MAX - 1,
        ),
    ];
    for (timeout, expected) in cases {
        assert_eq!(
            milliseconds(timeout).expect("expressible"),
            expected,
            "for {timeout:?}"
        );
    }
}

#[test]
fn a_timeout_the_call_cannot_express_is_invalid_input() {
    let cases = [
        Duration::from_millis(u64::from(u32::MAX)),
        Duration::from_nanos(4_294_967_294_000_001),
        Duration::from_millis(u64::from(u32::MAX) + 1),
        Duration::from_secs(u64::from(u32::MAX)),
        Duration::MAX,
    ];
    for timeout in cases {
        let error = milliseconds(Some(timeout)).expect_err("too long");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput, "for {timeout:?}");
    }
}

#[test]
fn every_code_the_call_returns_is_an_outcome_or_an_error() {
    assert_eq!(outcome(WAIT_OBJECT_0).expect("ok"), WaitOutcome::Signalled);
    assert_eq!(outcome(WAIT_ABANDONED).expect("ok"), WaitOutcome::Abandoned);
    assert_eq!(outcome(WAIT_TIMEOUT).expect("ok"), WaitOutcome::TimedOut);

    // SAFETY: sets this thread's last-error value, which nothing else reads.
    unsafe { SetLastError(ERROR_INVALID_HANDLE) };
    let failed = outcome(WAIT_FAILED).expect_err("a failed wait");
    assert_eq!(
        failed.raw_os_error(),
        i32::try_from(ERROR_INVALID_HANDLE).ok()
    );

    // `WAIT_IO_COMPLETION`, which a non-alertable wait never returns, and a value no wait does.
    for code in [0xC0_u32, 0x1234] {
        let unexpected = outcome(code).expect_err("not an outcome");
        assert_eq!(unexpected.kind(), io::ErrorKind::Other);
        assert!(
            unexpected.to_string().contains(&format!("{code:#x}")),
            "{unexpected}"
        );
    }
}
