// Copyright (c) 2026 Mike Grier
//! Unit tests for `Event`.

use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle};
use std::sync::mpsc;
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    CompareObjectHandles, DuplicateHandle, ERROR_ACCESS_DENIED, FALSE, HANDLE, WAIT_OBJECT_0,
    WAIT_TIMEOUT,
};
use windows_sys::Win32::System::Threading::{
    EVENT_MODIFY_STATE, GetCurrentProcess, ResetEvent, SYNCHRONIZATION_SYNCHRONIZE, SetEvent,
    WaitForSingleObject,
};

use super::{Event, ResetMode, WaitOnlyEvent};
use crate::{WaitOutcome, wait_for_single_object};

/// How long a test waits for something that should happen, in milliseconds.
const BOUND_MS: u32 = 5_000;

/// How long a test watches for something that must not happen.
///
/// Short, because it is spent on every passing run. It is evidence rather
/// than proof -- a waiter that would have returned at 51ms is not seen -- so
/// each test that relies on it is paired with one that observes the same
/// property without a timing window.
const QUIET: Duration = Duration::from_millis(50);

fn bound() -> Duration {
    Duration::from_millis(u64::from(BOUND_MS))
}

fn wait(handle: BorrowedHandle<'_>, timeout_ms: u32) -> u32 {
    // SAFETY: a borrowed handle is open for the borrow's lifetime.
    unsafe { WaitForSingleObject(handle.as_raw_handle(), timeout_ms) }
}

/// Whether the event is signalled now, without waiting.
///
/// **This consumes an auto-reset event's signal**, as any satisfied wait does,
/// which the auto-reset tests rely on.
fn take(event: &impl AsHandle) -> bool {
    match wait(event.as_handle(), 0) {
        WAIT_OBJECT_0 => true,
        WAIT_TIMEOUT => false,
        other => panic!("WaitForSingleObject returned {other:#x}"),
    }
}

/// A second handle to `event` with only the given access.
fn duplicate_with(event: &Event, access: u32) -> OwnedHandle {
    let mut duplicate: HANDLE = std::ptr::null_mut();
    // SAFETY: both process handles are the current-process pseudo-handle, the
    // source is open for the borrow of `event`, and `duplicate` is a valid out
    // pointer.
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
    assert_ne!(
        ok,
        FALSE,
        "DuplicateHandle: {}",
        std::io::Error::last_os_error()
    );
    // SAFETY: the call succeeded, so `duplicate` is a new handle owned by
    // nothing else.
    unsafe { OwnedHandle::from_raw_handle(duplicate) }
}

fn same_object(a: &impl AsRawHandle, b: &impl AsRawHandle) -> bool {
    // SAFETY: both handles are open for the duration of the borrows.
    unsafe { CompareObjectHandles(a.as_raw_handle(), b.as_raw_handle()) != FALSE }
}

fn event(reset: ResetMode, initially_signalled: bool) -> Event {
    Event::new(reset, initially_signalled).expect("create an event")
}

#[test]
fn an_auto_reset_event_created_unsignalled_is_not_signalled() {
    assert!(!take(&event(ResetMode::Auto, false)));
}

#[test]
fn an_auto_reset_event_created_signalled_satisfies_one_wait_then_resets() {
    let event = event(ResetMode::Auto, true);
    assert!(take(&event), "created signalled");
    assert!(!take(&event), "the first wait returned it to unsignalled");
}

#[test]
fn a_manual_reset_event_created_unsignalled_is_not_signalled() {
    assert!(!take(&event(ResetMode::Manual, false)));
}

#[test]
fn a_manual_reset_event_created_signalled_stays_signalled_across_waits() {
    let event = event(ResetMode::Manual, true);
    for attempt in 0..3 {
        assert!(take(&event), "wait {attempt} found it still signalled");
    }
}

#[test]
fn set_signals_an_auto_reset_event_for_exactly_one_wait() {
    let event = event(ResetMode::Auto, false);
    event.set().expect("set");
    assert!(take(&event), "set signalled it");
    assert!(!take(&event), "one wait took the signal");
}

#[test]
fn set_signals_a_manual_reset_event_until_it_is_reset() {
    let event = event(ResetMode::Manual, false);
    event.set().expect("set");
    assert!(take(&event), "set signalled it");
    assert!(take(&event), "a wait did not take the signal");
    event.reset().expect("reset");
    assert!(!take(&event), "reset withdrew it");
}

#[test]
fn repeated_sets_are_not_counted() {
    let event = event(ResetMode::Auto, false);
    for _ in 0..3 {
        event.set().expect("set");
    }
    assert!(take(&event), "set signalled it");
    assert!(
        !take(&event),
        "three sets were one signal, and one wait took it"
    );
}

#[test]
fn resetting_an_unsignalled_event_succeeds_and_changes_nothing() {
    for reset in [ResetMode::Auto, ResetMode::Manual] {
        let event = event(reset, false);
        event.reset().expect("reset an unsignalled event");
        assert!(!take(&event), "{reset:?}: still unsignalled");
        event.set().expect("set");
        assert!(take(&event), "{reset:?}: the reset did not break it");
    }
}

#[test]
fn reset_withdraws_an_auto_reset_signal_nobody_has_taken() {
    let event = event(ResetMode::Auto, false);
    event.set().expect("set");
    event.reset().expect("reset");
    assert!(!take(&event));
}

#[test]
fn a_clone_is_the_same_object_and_a_new_event_is_not() {
    let original = event(ResetMode::Manual, false);
    let clone = original.clone();
    let other = event(ResetMode::Manual, false);
    assert!(same_object(&original, &clone), "a clone is the same event");
    assert!(
        !same_object(&original, &other),
        "and the comparison can tell two events apart"
    );
    assert_eq!(
        original.as_raw_handle(),
        clone.as_raw_handle(),
        "a clone is the same handle, held twice (WS-D-10)"
    );
}

#[test]
fn a_clone_and_its_original_signal_each_other() {
    let original = event(ResetMode::Manual, false);
    let clone = original.clone();
    original.set().expect("set the original");
    assert!(take(&clone), "the clone saw the original's set");
    original.reset().expect("reset the original");
    assert!(!take(&clone), "the clone saw the original's reset");
    clone.set().expect("set the clone");
    assert!(take(&original), "the original saw the clone's set");
    clone.reset().expect("reset the clone");
    assert!(!take(&original), "the original saw the clone's reset");
}

#[test]
fn a_clone_outlives_its_original() {
    let original = event(ResetMode::Manual, false);
    let clone = original.clone();
    drop(original);
    clone.set().expect("set the surviving clone");
    assert!(take(&clone));
}

#[test]
fn a_set_on_another_thread_releases_a_waiter_on_this_one() {
    let event = event(ResetMode::Auto, false);
    let signaller = event.clone();
    let thread = std::thread::spawn(move || signaller.set());
    assert_eq!(wait(event.as_handle(), BOUND_MS), WAIT_OBJECT_0);
    thread
        .join()
        .expect("the signalling thread")
        .expect("set on the other thread");
}

#[test]
fn a_reset_on_another_thread_is_seen_on_this_one() {
    let event = event(ResetMode::Manual, true);
    let resetter = event.clone();
    std::thread::spawn(move || resetter.reset())
        .join()
        .expect("the resetting thread")
        .expect("reset on the other thread");
    assert!(!take(&event));
}

/// Start `count` threads each waiting once on a clone of `event`, reporting
/// which thread it was and what its wait returned.
fn waiters(event: &Event, count: usize) -> mpsc::Receiver<(usize, u32)> {
    let (tx, rx) = mpsc::channel();
    for which in 0..count {
        let waiter = event.clone();
        let tx = tx.clone();
        std::thread::spawn(move || {
            let result = wait(waiter.as_handle(), BOUND_MS);
            let _ = tx.send((which, result));
        });
    }
    rx
}

#[test]
fn an_auto_reset_set_releases_one_of_two_waiters() {
    let event = event(ResetMode::Auto, false);
    let released = waiters(&event, 2);

    event.set().expect("the first set");
    let (first, result) = released.recv_timeout(bound()).expect("one waiter released");
    assert_eq!(result, WAIT_OBJECT_0);
    assert!(
        released.recv_timeout(QUIET).is_err(),
        "one set released a second waiter"
    );

    event.set().expect("the second set");
    let (second, result) = released
        .recv_timeout(bound())
        .expect("the other waiter released");
    assert_eq!(result, WAIT_OBJECT_0);
    assert_ne!(first, second, "each set released a different waiter");
}

#[test]
fn a_manual_reset_set_releases_every_waiter() {
    let event = event(ResetMode::Manual, false);
    let released = waiters(&event, 2);
    event.set().expect("one set");
    let mut seen = Vec::new();
    for _ in 0..2 {
        let (which, result) = released.recv_timeout(bound()).expect("a waiter released");
        assert_eq!(result, WAIT_OBJECT_0);
        seen.push(which);
    }
    seen.sort_unstable();
    assert_eq!(seen, [0, 1]);
}

#[test]
fn set_and_reset_report_the_error_on_a_handle_without_modify_state() {
    let event = event(ResetMode::Manual, false);
    // SAFETY: a duplicate of an event is an event, and it carries SYNCHRONIZE.
    let reduced =
        unsafe { Event::from_owned_handle(duplicate_with(&event, SYNCHRONIZATION_SYNCHRONIZE)) };

    let refused = reduced.set().expect_err("set without EVENT_MODIFY_STATE");
    assert_eq!(refused.raw_os_error(), Some(ERROR_ACCESS_DENIED as i32));
    assert!(!take(&event), "the refused set changed nothing");

    event.set().expect("set through the full-access handle");
    let refused = reduced
        .reset()
        .expect_err("reset without EVENT_MODIFY_STATE");
    assert_eq!(refused.raw_os_error(), Some(ERROR_ACCESS_DENIED as i32));
    assert!(take(&event), "the refused reset changed nothing");
    assert!(take(&reduced), "the reduced handle can still be waited on");
}

#[test]
fn set_and_reset_succeed_on_an_adopted_handle_with_modify_state() {
    let event = event(ResetMode::Manual, false);
    // SAFETY: a duplicate of an event is an event, and it carries SYNCHRONIZE.
    let adopted = unsafe {
        Event::from_owned_handle(duplicate_with(
            &event,
            EVENT_MODIFY_STATE | SYNCHRONIZATION_SYNCHRONIZE,
        ))
    };
    adopted.set().expect("set through the adopted handle");
    assert!(take(&event), "the adopted handle set the original");
    adopted.reset().expect("reset through the adopted handle");
    assert!(!take(&event), "the adopted handle reset the original");
}

#[test]
fn as_handle_lends_the_handle_as_raw_handle_names() {
    let event = event(ResetMode::Auto, false);
    assert_eq!(event.as_handle().as_raw_handle(), event.as_raw_handle());
}

#[test]
fn an_event_is_send_and_sync() {
    fn assert_send_and_sync<T: Send + Sync>() {}
    assert_send_and_sync::<Event>();
}

fn wait_only(event: &Event) -> WaitOnlyEvent {
    event.duplicate_wait_only().expect("a wait-only duplicate")
}

/// The error a raw `SetEvent` on `handle` reports, `None` if it succeeded.
fn raw_set_error(handle: &impl AsRawHandle) -> Option<i32> {
    // SAFETY: the handle is open for the duration of the borrow.
    if unsafe { SetEvent(handle.as_raw_handle()) } == FALSE {
        return std::io::Error::last_os_error().raw_os_error();
    }
    None
}

/// The error a raw `ResetEvent` on `handle` reports, `None` if it succeeded.
fn raw_reset_error(handle: &impl AsRawHandle) -> Option<i32> {
    // SAFETY: the handle is open for the duration of the borrow.
    if unsafe { ResetEvent(handle.as_raw_handle()) } == FALSE {
        return std::io::Error::last_os_error().raw_os_error();
    }
    None
}

#[test]
fn a_wait_only_clone_is_the_same_object_and_a_new_event_is_not() {
    let event = event(ResetMode::Manual, false);
    let other = self::event(ResetMode::Manual, false);
    let waiter = wait_only(&event);
    assert!(same_object(&event, &waiter));
    assert!(!same_object(&other, &waiter));
}

#[test]
fn a_wait_only_clone_sees_the_original_set_and_reset() {
    let event = event(ResetMode::Manual, false);
    let waiter = wait_only(&event);
    assert!(!take(&waiter));
    event.set().expect("set");
    assert!(take(&waiter));
    event.reset().expect("reset");
    assert!(!take(&waiter));
}

#[test]
fn a_wait_through_a_wait_only_clone_takes_an_auto_reset_signal() {
    let event = event(ResetMode::Auto, true);
    let waiter = wait_only(&event);
    assert!(take(&waiter), "the wait took the signal");
    assert!(!take(&event), "and the original no longer has it");
}

#[test]
fn a_wait_only_clone_can_neither_set_nor_reset_the_event() {
    let event = event(ResetMode::Manual, false);
    let waiter = wait_only(&event);
    let denied = Some(ERROR_ACCESS_DENIED as i32);

    assert_eq!(raw_set_error(&waiter), denied);
    assert!(!take(&event), "the refused set changed nothing");

    event.set().expect("set the original");
    assert_eq!(raw_reset_error(&waiter), denied);
    assert!(take(&event), "the refused reset changed nothing");
}

#[test]
fn a_wait_only_clone_converted_to_a_handle_and_adopted_as_an_event_cannot_set_it() {
    let event = event(ResetMode::Manual, false);
    let handle = wait_only(&event).try_into_owned().expect("the only holder");
    // SAFETY: a duplicate of an event is an event, and this one carries SYNCHRONIZE.
    let adopted = unsafe { Event::from_owned_handle(handle) };
    let refused = adopted.set().expect_err("the access was narrowed away");
    assert_eq!(refused.raw_os_error(), Some(ERROR_ACCESS_DENIED as i32));
    assert!(!take(&event));
}

#[test]
fn a_clone_of_a_wait_only_clone_is_wait_only_too() {
    let event = event(ResetMode::Manual, false);
    let first = wait_only(&event);
    let second = first.clone();
    assert!(same_object(&first, &second));
    assert!(same_object(&event, &second));
    assert_eq!(raw_set_error(&second), Some(ERROR_ACCESS_DENIED as i32));
    event.set().expect("set");
    assert!(take(&second));
}

#[test]
fn a_wait_only_clone_outlives_the_event_it_came_from() {
    let event = event(ResetMode::Manual, false);
    let keeper = event.clone();
    let waiter = wait_only(&event);
    drop(event);
    keeper.set().expect("set through the kept clone");
    drop(keeper);
    assert!(take(&waiter), "the object lives while any handle does");
}

#[test]
fn a_wait_only_clone_is_waited_on_by_the_crates_wait() {
    let event = event(ResetMode::Auto, false);
    let waiter = wait_only(&event);
    assert_eq!(
        wait_for_single_object(&waiter, Some(Duration::ZERO)).expect("wait"),
        WaitOutcome::TimedOut
    );
    event.set().expect("set");
    assert_eq!(
        wait_for_single_object(&waiter, Some(bound())).expect("wait"),
        WaitOutcome::Signalled
    );
}

#[test]
fn a_set_on_another_thread_releases_a_wait_only_waiter_on_this_one() {
    let event = event(ResetMode::Auto, false);
    let waiter = wait_only(&event);
    let signaller = event.clone();
    let worker = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(10));
        signaller.set()
    });
    assert_eq!(
        wait_for_single_object(&waiter, Some(bound())).expect("wait"),
        WaitOutcome::Signalled
    );
    worker.join().expect("worker").expect("set");
}

#[test]
fn as_handle_lends_a_wait_only_clones_handle_as_raw_handle_names() {
    let event = event(ResetMode::Auto, false);
    let waiter = wait_only(&event);
    assert_eq!(waiter.as_handle().as_raw_handle(), waiter.as_raw_handle());
    let raw = waiter.as_raw_handle();
    let owned = waiter.try_into_owned().expect("the only holder");
    assert_eq!(owned.as_raw_handle(), raw, "the handle itself moved out");
}

#[test]
fn a_wait_only_event_is_send_and_sync() {
    fn assert_send_and_sync<T: Send + Sync>() {}
    assert_send_and_sync::<WaitOnlyEvent>();
}

// --- clones share one handle (WS-2.1, WS-D-10) ---

#[test]
fn a_clone_shares_the_handle_rather_than_making_one() {
    let event = event(ResetMode::Manual, false);
    let clone = event.clone();
    assert_eq!(
        clone.as_raw_handle(),
        event.as_raw_handle(),
        "one handle, held twice"
    );
}

#[test]
fn a_set_through_a_clone_is_seen_through_the_original_and_a_reset_back() {
    let event = event(ResetMode::Manual, false);
    let clone = event.clone();
    clone.set().expect("set through the clone");
    assert!(take(&event), "the original sees the clone's set");
    event.reset().expect("reset through the original");
    assert!(!take(&clone), "the clone sees the original's reset");
}

#[test]
fn a_clone_keeps_the_handle_open_after_the_original_drops() {
    let event = event(ResetMode::Manual, false);
    let clone = event.clone();
    let raw = event.as_raw_handle();
    drop(event);
    clone.set().expect("set after the original dropped");
    assert!(take(&clone));
    assert_eq!(clone.as_raw_handle(), raw, "the same handle is still open");
}

#[test]
fn an_event_gives_its_handle_back_only_to_its_last_holder() {
    let event = event(ResetMode::Manual, false);
    let clone = event.clone();
    let raw = event.as_raw_handle();
    let event = event.try_into_owned().expect_err("a clone is still held");
    drop(clone);
    let owned = event.try_into_owned().expect("the last holder");
    assert_eq!(
        owned.as_raw_handle(),
        raw,
        "the handle itself, not a duplicate"
    );
}

#[test]
fn an_event_converted_into_a_shared_handle_is_its_handle() {
    let event = event(ResetMode::Manual, false);
    let clone = event.clone();
    let shared = win_shared_os_owned_handle::SharedHandle::from(event);
    assert_eq!(shared.as_raw_handle(), clone.as_raw_handle());
    clone.set().expect("set through the clone");
    assert!(take(&shared), "the shared handle sees the set");
}

#[test]
fn duplicate_wait_only_makes_a_new_handle_that_cannot_signal() {
    let event = event(ResetMode::Manual, false);
    let waiter = event.duplicate_wait_only().expect("a wait-only duplicate");
    assert_ne!(
        waiter.as_raw_handle(),
        event.as_raw_handle(),
        "a new handle"
    );
    assert!(same_object(&waiter, &event), "to the same event");
    assert_eq!(raw_set_error(&waiter), Some(ERROR_ACCESS_DENIED as i32));
}

#[test]
fn a_clone_of_a_wait_only_event_shares_its_handle_and_its_access() {
    let event = event(ResetMode::Manual, false);
    let waiter = event.duplicate_wait_only().expect("a wait-only duplicate");
    let clone = waiter.clone();
    assert_eq!(
        clone.as_raw_handle(),
        waiter.as_raw_handle(),
        "one handle, held twice"
    );
    assert_eq!(raw_set_error(&clone), Some(ERROR_ACCESS_DENIED as i32));
    let waiter = waiter.try_into_owned().expect_err("a clone is still held");
    drop(clone);
    assert!(waiter.try_into_owned().is_ok(), "the last holder");
}
