// Copyright (c) 2026 Mike Grier
//! Unit tests for the affine subscription handle.
//!
//! What is under test is the lifetime contract -- registration begins a watch,
//! and both ways of ending one end it -- plus the binding between a subscription
//! and the receiver its notifications reach.

use std::time::{Duration, Instant};

use super::{RetryMode, WatchOptions};
use crate::monitor::Monitor;
use crate::notify::ChangeKind;
use crate::queue::{Notification, Receiver, WatchId};
use crate::testing::TempDir;

/// Upper bound for waiting on a change the kernel really should report.
const NOTIFY_TIMEOUT: Duration = Duration::from_secs(30);

/// Drain until a change with `name` arrives, returning the subscription it was
/// tagged with. Fails rather than hanging.
fn await_change(receiver: &Receiver, name: &str) -> WatchId {
    let deadline = Instant::now() + NOTIFY_TIMEOUT;
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .unwrap_or_default();
        assert!(
            !remaining.is_zero(),
            "timed out waiting for a change to {name}"
        );
        let Some(item) = receiver.recv_timeout(remaining) else {
            continue;
        };
        if let Notification::Batch { watch, changes } = item
            && changes.iter().any(|change| {
                change.kind == ChangeKind::Added
                    && change.name.to_os_string().to_string_lossy() == name
            })
        {
            return watch;
        }
    }
}

#[test]
fn options_default_to_a_shallow_autonomous_watch() {
    let options = WatchOptions::new();
    assert!(!options.subtree);
    assert_eq!(options.retry, RetryMode::Defaults);
    assert_eq!(options, WatchOptions::default());
}

#[test]
fn options_carry_what_the_client_states() {
    let options = WatchOptions::new()
        .subtree(true)
        .retry(RetryMode::Interactive);
    assert!(options.subtree);
    assert_eq!(options.retry, RetryMode::Interactive);
}

#[test]
fn subscribing_begins_a_watch() {
    let dir = TempDir::new("watch-begin");
    let monitor = Monitor::new().expect("create the monitor");
    let (session, _receiver) = monitor.session();

    let watch = session
        .subscribe(dir.path(), WatchOptions::new())
        .expect("register");
    monitor.quiesce();

    assert!(monitor.is_watching(watch.id()));

    drop(watch);
    drop(monitor);
    dir.cleanup();
}

#[test]
fn every_subscription_gets_its_own_identifier() {
    let dir = TempDir::new("watch-ids");
    let monitor = Monitor::new().expect("create the monitor");
    let (first_session, _first) = monitor.session();
    let (second_session, _second) = monitor.session();

    // Identifiers are minted per monitor, not per session: they key resident
    // state, so two sessions must never mint the same one.
    let a = first_session
        .subscribe(dir.path(), WatchOptions::new())
        .expect("register");
    let b = second_session
        .subscribe(dir.path(), WatchOptions::new())
        .expect("register");
    assert_ne!(a.id(), b.id());

    drop((a, b));
    drop(monitor);
    dir.cleanup();
}

#[test]
fn cancelling_ends_the_watch() {
    let dir = TempDir::new("watch-cancel");
    let monitor = Monitor::new().expect("create the monitor");
    let (session, _receiver) = monitor.session();

    let watch = session
        .subscribe(dir.path(), WatchOptions::new())
        .expect("register");
    monitor.quiesce();
    let id = watch.id();
    assert!(monitor.is_watching(id));

    watch.cancel();
    monitor.quiesce();
    assert!(!monitor.is_watching(id), "cancel must release the watcher");
    assert_eq!(monitor.watcher_count(), 0);

    drop(monitor);
    dir.cleanup();
}

#[test]
fn dropping_a_watch_ends_it_too() {
    // The affine half of D-5: `Drop` is cancellation, which is what makes the
    // handle's `#[must_use]` load-bearing rather than decorative.
    let dir = TempDir::new("watch-drop");
    let monitor = Monitor::new().expect("create the monitor");
    let (session, _receiver) = monitor.session();

    let watch = session
        .subscribe(dir.path(), WatchOptions::new())
        .expect("register");
    monitor.quiesce();
    let id = watch.id();

    drop(watch);
    monitor.quiesce();
    assert!(!monitor.is_watching(id));

    drop(monitor);
    dir.cleanup();
}

#[test]
fn a_cancelled_watch_does_not_cancel_twice_when_dropped() {
    // `cancel` consumes the handle, so `Drop` still runs; enqueueing a second
    // cancellation would be a request for a subscription that no longer exists.
    let dir = TempDir::new("watch-twice");
    let monitor = Monitor::new().expect("create the monitor");
    let (session, _receiver) = monitor.session();

    let watch = session
        .subscribe(dir.path(), WatchOptions::new())
        .expect("register");
    monitor.quiesce();
    watch.cancel();
    monitor.quiesce();

    // A second subscription must be unaffected by the first's teardown.
    let next = session
        .subscribe(dir.path(), WatchOptions::new())
        .expect("register");
    monitor.quiesce();
    assert!(monitor.is_watching(next.id()));
    assert_eq!(monitor.watcher_count(), 1);

    drop(next);
    drop(monitor);
    dir.cleanup();
}

#[test]
fn a_watch_delivers_to_its_sessions_receiver_tagged_with_its_own_id() {
    let dir = TempDir::new("watch-deliver");
    let monitor = Monitor::new().expect("create the monitor");
    let (session, receiver) = monitor.session();

    let watch = session
        .subscribe(dir.path(), WatchOptions::new())
        .expect("register");
    monitor.quiesce();

    std::fs::write(dir.path().join("alpha.txt"), b"a").expect("create");
    assert_eq!(
        await_change(&receiver, "alpha.txt"),
        watch.id(),
        "a notification carries the subscription that produced it"
    );

    drop(watch);
    drop(monitor);
    dir.cleanup();
}

#[test]
fn two_subscriptions_on_one_session_are_distinguishable() {
    let first = TempDir::new("watch-first");
    let second = TempDir::new("watch-second");
    let monitor = Monitor::new().expect("create the monitor");
    let (session, receiver) = monitor.session();

    let a = session
        .subscribe(first.path(), WatchOptions::new())
        .expect("register");
    let b = session
        .subscribe(second.path(), WatchOptions::new())
        .expect("register");
    monitor.quiesce();

    std::fs::write(first.path().join("in-first.txt"), b"a").expect("create");
    std::fs::write(second.path().join("in-second.txt"), b"b").expect("create");

    // One receiver, two streams: the tag is what lets a client tell them apart.
    assert_eq!(await_change(&receiver, "in-first.txt"), a.id());
    assert_eq!(await_change(&receiver, "in-second.txt"), b.id());

    drop((a, b));
    drop(monitor);
    first.cleanup();
    second.cleanup();
}

#[test]
fn a_subtree_subscription_reports_below_itself() {
    let dir = TempDir::new("watch-subtree");
    let monitor = Monitor::new().expect("create the monitor");
    let (session, receiver) = monitor.session();

    let watch = session
        .subscribe(dir.path(), WatchOptions::new().subtree(true))
        .expect("register");
    monitor.quiesce();

    let nested = dir.path().join("nested");
    std::fs::create_dir(&nested).expect("create the subdirectory");
    std::fs::write(nested.join("deep.txt"), b"deep").expect("create the nested file");

    assert_eq!(await_change(&receiver, "nested\\deep.txt"), watch.id());

    drop(watch);
    drop(monitor);
    dir.cleanup();
}

#[test]
fn the_retry_mode_stated_at_registration_is_recorded() {
    // M5.3 is what acts on this, but registration is the only place a client can
    // state it (D-27), so it is carried and observable from the start.
    let dir = TempDir::new("watch-retry");
    let monitor = Monitor::new().expect("create the monitor");
    let (session, _receiver) = monitor.session();

    let defaults = session
        .subscribe(dir.path(), WatchOptions::new())
        .expect("register");
    let interactive = session
        .subscribe(
            dir.path(),
            WatchOptions::new().retry(RetryMode::Interactive),
        )
        .expect("register");
    monitor.quiesce();

    assert_eq!(monitor.retry_mode(defaults.id()), Some(RetryMode::Defaults));
    assert_eq!(
        monitor.retry_mode(interactive.id()),
        Some(RetryMode::Interactive)
    );

    drop((defaults, interactive));
    drop(monitor);
    dir.cleanup();
}

#[test]
fn subscribing_to_a_path_that_cannot_be_watched_starts_no_watcher() {
    // It also says nothing, which is the gap M3.6 closes: a permanent failure
    // (D-22) currently leaves a client holding a `Watch` that can never fire.
    let dir = TempDir::new("watch-missing");
    let monitor = Monitor::new().expect("create the monitor");
    let (session, _receiver) = monitor.session();

    let watch = session
        .subscribe(dir.path().join("no-such-directory"), WatchOptions::new())
        .expect("the request is accepted; whether it can be watched is not known yet");
    monitor.quiesce();

    assert!(!monitor.is_watching(watch.id()));
    assert_eq!(monitor.watcher_count(), 0);

    drop(watch);
    drop(monitor);
    dir.cleanup();
}

#[test]
fn subscribing_after_shutdown_fails() {
    let dir = TempDir::new("watch-closed");
    let monitor = Monitor::new().expect("create the monitor");
    let (session, _receiver) = monitor.session();
    monitor.shut_down();

    let error = session
        .subscribe(dir.path(), WatchOptions::new())
        .expect_err("a shut-down monitor cannot register anything");
    assert_eq!(error.kind(), std::io::ErrorKind::NotConnected);

    dir.cleanup();
}

#[test]
fn a_watch_outliving_its_monitor_drops_without_wedging() {
    // Cancellation is a request, and a shut-down monitor refuses requests -- so
    // `Drop` must treat that rejection as nothing to do rather than an error.
    let dir = TempDir::new("watch-outlives");
    let watch = {
        let monitor = Monitor::new().expect("create the monitor");
        let (session, _receiver) = monitor.session();
        let watch = session
            .subscribe(dir.path(), WatchOptions::new())
            .expect("register");
        monitor.quiesce();
        watch
    };

    let started = Instant::now();
    drop(watch);
    assert!(started.elapsed() < Duration::from_secs(5));

    dir.cleanup();
}
