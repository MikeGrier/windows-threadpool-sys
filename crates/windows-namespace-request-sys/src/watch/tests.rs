// Copyright (c) Mike Grier.

//! Tests for the `FindFirstChangeNotificationW` entry.
//!
//! The watch is exercised against a real directory and a real change, because
//! the one thing worth proving here -- that the handle actually signals -- is
//! not observable in memory. The waits are bounded so a platform that never
//! signals fails the test rather than hanging the suite.

use std::time::Duration;

use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE,
};
use windows_sys::Win32::System::Threading::WaitForSingleObject;
use wtf_string::Wtf16String;

use super::{ChangeNotification, NotifyFilter, WatchDirectory};
use crate::handle::tests::{Fixture, handle_allocation};
use crate::prepare;

/// Long enough that a working watch signals well inside it, short enough that a
/// broken one fails the suite rather than hanging it.
const SIGNAL_TIMEOUT: Duration = Duration::from_secs(5);

fn watch_for(fixture: &Fixture, filter: NotifyFilter) -> WatchDirectory {
    let text = fixture
        .directory()
        .to_str()
        .expect("the fixture path is valid UTF-8");

    WatchDirectory::new(prepare(&Wtf16String::from(text)).expect("prepare the fixture path"))
        .with_filter(filter)
}

/// Waits for the notification to signal, returning whether it did.
fn signalled_within(notification: &ChangeNotification, timeout: Duration) -> bool {
    let millis = u32::try_from(timeout.as_millis()).expect("the timeout fits in a u32");
    // SAFETY: the handle is live for the notification's lifetime, and
    // WaitForSingleObject only reads it.
    let outcome = unsafe { WaitForSingleObject(notification.as_raw(), millis) };

    match outcome {
        WAIT_OBJECT_0 => true,
        WAIT_TIMEOUT => false,
        other => panic!("the wait failed unexpectedly: {other}"),
    }
}

#[test]
fn a_watch_starts_on_a_real_directory() {
    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");
    let fixture = Fixture::new("watch-start");

    let notification = watch_for(&fixture, NotifyFilter::FILE_NAME)
        .perform()
        .expect("start watching the fixture directory");

    assert!(!notification.as_raw().is_null());
}

#[test]
fn a_watch_signals_when_a_file_appears() {
    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");
    let fixture = Fixture::new("watch-signal");
    let notification = watch_for(&fixture, NotifyFilter::FILE_NAME)
        .perform()
        .expect("start watching the fixture directory");

    std::fs::write(fixture.directory().join("appeared.t"), b"x").expect("create a file");

    assert!(
        signalled_within(&notification, SIGNAL_TIMEOUT),
        "a new file must signal a FILE_NAME watch"
    );
}

#[test]
fn a_watch_can_be_rearmed_and_signals_again() {
    // A notification signals once and stays signalled until rearmed, so a
    // consumer waiting in a loop needs this.
    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");
    let fixture = Fixture::new("watch-rearm");
    let notification = watch_for(&fixture, NotifyFilter::FILE_NAME)
        .perform()
        .expect("start watching the fixture directory");

    std::fs::write(fixture.directory().join("first.t"), b"x").expect("create the first file");
    assert!(signalled_within(&notification, SIGNAL_TIMEOUT));

    notification.rearm().expect("rearm the watch");

    std::fs::write(fixture.directory().join("second.t"), b"x").expect("create the second file");
    assert!(
        signalled_within(&notification, SIGNAL_TIMEOUT),
        "a rearmed watch must signal the next change"
    );
}

#[test]
fn a_watch_does_not_signal_for_a_change_outside_its_filter() {
    // The negative that gives the positive its meaning: a suite that only ever
    // saw a watch signal could not tell a working filter from an ignored one.
    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");
    let fixture = Fixture::new("watch-filter");
    // Watch only for directory names, then create a *file*.
    let notification = watch_for(&fixture, NotifyFilter::DIR_NAME)
        .perform()
        .expect("start watching the fixture directory");

    std::fs::write(fixture.directory().join("a-file.t"), b"x").expect("create a file");

    assert!(
        !signalled_within(&notification, Duration::from_millis(300)),
        "a DIR_NAME watch must not signal for a file"
    );
}

#[test]
fn a_subtree_watch_signals_for_a_change_in_a_child_directory() {
    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");
    let fixture = Fixture::new("watch-subtree");
    let child = fixture.directory().join("child");
    std::fs::create_dir(&child).expect("create a child directory");

    let notification = watch_for(&fixture, NotifyFilter::FILE_NAME)
        .with_subtree(true)
        .perform()
        .expect("start watching the subtree");

    std::fs::write(child.join("deep.t"), b"x").expect("create a file in the child");

    assert!(
        signalled_within(&notification, SIGNAL_TIMEOUT),
        "a subtree watch must see a change below the watched directory"
    );
}

#[test]
fn a_non_subtree_watch_ignores_a_change_in_a_child_directory() {
    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");
    let fixture = Fixture::new("watch-no-subtree");
    let child = fixture.directory().join("child");
    std::fs::create_dir(&child).expect("create a child directory");

    let notification = watch_for(&fixture, NotifyFilter::FILE_NAME)
        .with_subtree(false)
        .perform()
        .expect("start watching without the subtree");

    std::fs::write(child.join("deep.t"), b"x").expect("create a file in the child");

    assert!(
        !signalled_within(&notification, Duration::from_millis(300)),
        "the subtree flag must actually be carried"
    );
}

#[test]
fn a_missing_directory_reports_the_raw_code() {
    let fixture = Fixture::new("watch-missing");
    let absent = fixture.directory().join("no-such-directory");
    let text = absent.to_str().expect("the fixture path is valid UTF-8");

    let outcome = WatchDirectory::new(prepare(&Wtf16String::from(text)).expect("prepare the path"))
        .with_filter(NotifyFilter::FILE_NAME)
        .perform();

    assert!(
        outcome.is_err(),
        "watching a directory that does not exist must fail, unaltered"
    );
}

#[test]
fn an_empty_filter_is_refused_by_windows_rather_than_by_this_crate() {
    let fixture = Fixture::new("watch-empty-filter");

    let outcome = watch_for(&fixture, NotifyFilter::NONE).perform();

    assert!(
        outcome.is_err(),
        "an empty filter reaches Windows and is judged there"
    );
}

#[test]
fn the_filter_constants_match_their_win32_values() {
    // Bound to the platform's constants rather than restated as literals, so
    // the two cannot drift.
    assert_eq!(NotifyFilter::FILE_NAME.bits(), FILE_NOTIFY_CHANGE_FILE_NAME);
    assert_eq!(NotifyFilter::DIR_NAME.bits(), FILE_NOTIFY_CHANGE_DIR_NAME);
    assert_eq!(
        NotifyFilter::LAST_WRITE.bits(),
        FILE_NOTIFY_CHANGE_LAST_WRITE
    );
}

#[test]
fn filters_combine_and_report_what_they_contain() {
    let combined = NotifyFilter::FILE_NAME | NotifyFilter::DIR_NAME;

    assert!(combined.contains(NotifyFilter::FILE_NAME));
    assert!(combined.contains(NotifyFilter::DIR_NAME));
    assert!(!combined.contains(NotifyFilter::SIZE));
    assert_eq!(
        combined.bits(),
        FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME
    );
}

#[test]
fn an_unknown_filter_bit_is_carried_rather_than_refused() {
    // The type is a newtype over a bitmask, not an enum, so a bit this crate
    // has never heard of still reaches Windows.
    let unknown = NotifyFilter::from_bits(0x8000_0000);

    assert_eq!(unknown.bits(), 0x8000_0000);
}

#[test]
fn a_new_request_watches_nothing_recursively() {
    let fixture = Fixture::new("watch-defaults");

    let text = fixture
        .directory()
        .to_str()
        .expect("the fixture path is valid UTF-8");
    let request = WatchDirectory::new(prepare(&Wtf16String::from(text)).expect("prepare the path"));

    assert!(!request.subtree());
    assert_eq!(request.filter(), NotifyFilter::NONE);
}

#[test]
fn a_request_is_cloneable_and_the_clone_watches_the_same_directory() {
    // Unlike the open entries, this request owns no handle, so an infallible
    // Clone is honest here.
    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");
    let fixture = Fixture::new("watch-clone");
    let request = watch_for(&fixture, NotifyFilter::FILE_NAME);

    let clone = request.clone();
    drop(request);

    let notification = clone.perform().expect("the clone starts a watch");
    assert!(!notification.as_raw().is_null());
}

#[test]
fn a_watch_moves_to_another_thread_and_still_signals() {
    const fn assert_send<T: Send>() {}
    const fn assert_sync<T: Sync>() {}

    assert_send::<WatchDirectory>();
    assert_sync::<WatchDirectory>();
    assert_send::<ChangeNotification>();
    assert_sync::<ChangeNotification>();

    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");
    let fixture = Fixture::new("watch-thread");
    let request = watch_for(&fixture, NotifyFilter::FILE_NAME);
    let target = fixture.directory().join("from-worker.t");

    let notification = request.perform().expect("start the watch");
    let waiter = std::thread::spawn(move || signalled_within(&notification, SIGNAL_TIMEOUT));

    std::fs::write(&target, b"x").expect("create a file");

    assert!(
        waiter.join().expect("the worker did not panic"),
        "a notification handle is usable from a thread that did not create it"
    );
}
