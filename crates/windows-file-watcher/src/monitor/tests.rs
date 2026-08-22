// Copyright (c) 2026 Mike Grier
//! Unit tests for the monitor.
//!
//! The servicing path's own guarantees are tested in `src/servicing/tests.rs`;
//! what is here is the monitor's ownership of watchers -- now reached the way a
//! client reaches it, through a session's subscribe -- and the blocking teardown
//! of D-20.

use std::time::{Duration, Instant};

use super::Monitor;
use crate::queue::{Notification, Receiver};
use crate::session::Session;
use crate::testing::TempDir;
use crate::watch::{Watch, WatchOptions};

/// What teardown is allowed to take. Cancellation retires an outstanding read at
/// once, so this only fires if teardown waited for a change instead.
const TEARDOWN_BUDGET: Duration = Duration::from_secs(5);

/// A monitor watching `dir` through one session, with registration already
/// serviced.
fn watching(dir: &std::path::Path) -> (Monitor, Session, Receiver, Watch) {
    let monitor = Monitor::new().expect("create the monitor");
    let (session, receiver) = monitor.session();
    let watch = session
        .subscribe(dir, WatchOptions::new())
        .expect("register the subscription");
    monitor.quiesce();
    (monitor, session, receiver, watch)
}

#[test]
fn a_new_monitor_is_running_and_owns_nothing() {
    let monitor = Monitor::new().expect("create the monitor");
    assert!(monitor.is_running());
    assert_eq!(monitor.watcher_count(), 0);
}

#[test]
fn shutdown_stops_the_servicing_path() {
    let monitor = Monitor::new().expect("create the monitor");
    monitor.shut_down();
    assert!(!monitor.is_running());
}

#[test]
fn shutdown_is_idempotent_and_drop_after_it_is_safe() {
    let monitor = Monitor::new().expect("create the monitor");
    monitor.shut_down();
    monitor.shut_down();
    monitor.shut_down();
    assert!(!monitor.is_running());
    drop(monitor);
}

#[test]
fn dropping_an_idle_monitor_is_prompt() {
    let monitor = Monitor::new().expect("create the monitor");
    let started = Instant::now();
    drop(monitor);
    assert!(started.elapsed() < TEARDOWN_BUDGET);
}

#[test]
fn a_subscription_becomes_a_watcher() {
    let dir = TempDir::new("monitor-subscribe");
    let (monitor, _session, _receiver, watch) = watching(dir.path());

    assert_eq!(monitor.watcher_count(), 1);
    assert!(monitor.is_watching(watch.id()));

    drop(watch);
    drop(monitor);
    dir.cleanup();
}

#[test]
fn several_subscriptions_become_several_watchers() {
    let dir = TempDir::new("monitor-several");
    let monitor = Monitor::new().expect("create the monitor");
    let (session, _receiver) = monitor.session();

    let watches: Vec<_> = (0..8)
        .map(|_| {
            session
                .subscribe(dir.path(), WatchOptions::new())
                .expect("register")
        })
        .collect();
    monitor.quiesce();
    assert_eq!(monitor.watcher_count(), 8);

    // Each identifier is distinct, which is what lets a client demultiplex one
    // receiver back into its subscriptions.
    let mut ids: Vec<u64> = watches.iter().map(|watch| watch.id().get()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 8);

    drop(watches);
    drop(monitor);
    dir.cleanup();
}

#[test]
fn teardown_with_watchers_outstanding_converges_promptly() {
    let dir = TempDir::new("monitor-outstanding");
    let monitor = Monitor::new().expect("create the monitor");
    let (session, _receiver) = monitor.session();

    let _watches: Vec<_> = (0..8)
        .map(|_| {
            session
                .subscribe(dir.path(), WatchOptions::new())
                .expect("register")
        })
        .collect();
    monitor.quiesce();

    // Nothing will change this directory again, so only cancellation can retire
    // the eight outstanding reads. A teardown that waited would sit here until
    // the budget expired.
    let started = Instant::now();
    monitor.shut_down();
    let elapsed = started.elapsed();
    assert!(
        elapsed < TEARDOWN_BUDGET,
        "teardown took {elapsed:?}, which means it waited rather than cancelled"
    );
    assert_eq!(monitor.watcher_count(), 0);

    dir.cleanup();
}

#[test]
fn teardown_releases_every_watcher_so_the_receiver_disconnects() {
    let dir = TempDir::new("monitor-release");
    let (monitor, session, receiver, watch) = watching(dir.path());

    // Every holder of a sender must go: the session, the watch that clones it,
    // and the watcher the monitor owns.
    drop(watch);
    drop(session);
    monitor.shut_down();

    assert!(
        receiver.is_disconnected(),
        "teardown must release the watcher's sender, not just forget the table entry"
    );

    dir.cleanup();
}

#[test]
fn dropping_a_monitor_tears_down_its_watchers() {
    let dir = TempDir::new("monitor-drop");
    let (monitor, session, receiver, watch) = watching(dir.path());

    drop(watch);
    drop(session);
    drop(monitor);

    assert!(
        receiver.is_disconnected(),
        "`Drop` must run the same teardown as an explicit shutdown"
    );

    dir.cleanup();
}

#[test]
fn teardown_from_a_thread_other_than_the_creator_is_safe() {
    let dir = TempDir::new("monitor-thread");
    let (monitor, session, receiver, watch) = watching(dir.path());
    drop(watch);
    drop(session);

    std::thread::spawn(move || drop(monitor))
        .join()
        .expect("teardown thread");

    assert!(receiver.is_disconnected());
    dir.cleanup();
}

#[test]
fn many_monitors_tear_down_concurrently_without_wedging() {
    let dir = TempDir::new("monitor-concurrent");
    let root = dir.path().to_path_buf();

    let workers: Vec<_> = (0..8)
        .map(|_| {
            let root = root.clone();
            std::thread::spawn(move || {
                let (monitor, session, receiver, watch) = watching(&root);
                drop(watch);
                drop(session);
                drop(monitor);
                assert!(receiver.is_disconnected());
            })
        })
        .collect();

    let started = Instant::now();
    for worker in workers {
        worker.join().expect("worker");
    }
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "concurrent teardown wedged"
    );

    dir.cleanup();
}

#[test]
fn debug_reports_the_monitors_state() {
    let dir = TempDir::new("monitor-debug");
    let (monitor, _session, _receiver, watch) = watching(dir.path());

    let rendered = format!("{monitor:?}");
    assert!(rendered.contains("running: true"), "{rendered}");
    assert!(rendered.contains("watchers: 1"), "{rendered}");

    drop(watch);
    drop(monitor);
    dir.cleanup();
}

// --- coalescing by directory (D-6) ---

#[test]
fn two_subscriptions_on_the_same_directory_share_one_watcher() {
    let dir = TempDir::new("coalesce-same");
    let monitor = Monitor::new().expect("create the monitor");
    let (session, _receiver) = monitor.session();

    let a = session
        .subscribe(dir.path(), WatchOptions::new())
        .expect("register");
    let b = session
        .subscribe(dir.path(), WatchOptions::new())
        .expect("register");
    monitor.quiesce();

    assert_eq!(monitor.watcher_count(), 2, "two subscriptions");
    assert_eq!(
        monitor.directory_count(),
        1,
        "one directory, one coalesced watcher"
    );

    drop((a, b));
    drop(monitor);
    dir.cleanup();
}

#[test]
fn subscriptions_on_different_directories_get_different_watchers() {
    let first = TempDir::new("coalesce-a");
    let second = TempDir::new("coalesce-b");
    let monitor = Monitor::new().expect("create the monitor");
    let (session, _receiver) = monitor.session();

    let a = session
        .subscribe(first.path(), WatchOptions::new())
        .expect("register");
    let b = session
        .subscribe(second.path(), WatchOptions::new())
        .expect("register");
    monitor.quiesce();

    assert_eq!(monitor.directory_count(), 2);

    drop((a, b));
    drop(monitor);
    first.cleanup();
    second.cleanup();
}

#[test]
fn the_same_directory_reached_through_a_different_spelling_still_coalesces() {
    // Coalescing is by identity (D-6), not by string comparison: a trailing
    // separator names the same directory.
    let dir = TempDir::new("coalesce-spelling");
    let mut with_separator = dir.path().to_path_buf();
    with_separator.push(""); // appends the trailing separator

    let monitor = Monitor::new().expect("create the monitor");
    let (session, _receiver) = monitor.session();
    let a = session
        .subscribe(dir.path(), WatchOptions::new())
        .expect("register");
    let b = session
        .subscribe(&with_separator, WatchOptions::new())
        .expect("register");
    monitor.quiesce();

    assert_eq!(monitor.directory_count(), 1);

    drop((a, b));
    drop(monitor);
    dir.cleanup();
}

#[test]
fn cancelling_one_of_two_coalesced_subscriptions_keeps_the_watcher_alive() {
    let dir = TempDir::new("coalesce-cancel-one");
    let monitor = Monitor::new().expect("create the monitor");
    let (session, _receiver) = monitor.session();

    let a = session
        .subscribe(dir.path(), WatchOptions::new())
        .expect("register");
    let b = session
        .subscribe(dir.path(), WatchOptions::new())
        .expect("register");
    monitor.quiesce();

    a.cancel();
    monitor.quiesce();

    assert_eq!(monitor.watcher_count(), 1, "only b remains");
    assert_eq!(
        monitor.directory_count(),
        1,
        "the watcher survives while a subscriber remains"
    );
    assert!(monitor.is_watching(b.id()));

    drop(b);
    drop(monitor);
    dir.cleanup();
}

#[test]
fn cancelling_the_last_coalesced_subscription_tears_down_the_watcher() {
    let dir = TempDir::new("coalesce-cancel-last");
    let monitor = Monitor::new().expect("create the monitor");
    let (session, _receiver) = monitor.session();

    let a = session
        .subscribe(dir.path(), WatchOptions::new())
        .expect("register");
    let b = session
        .subscribe(dir.path(), WatchOptions::new())
        .expect("register");
    monitor.quiesce();

    a.cancel();
    b.cancel();
    monitor.quiesce();

    assert_eq!(monitor.watcher_count(), 0);
    assert_eq!(monitor.directory_count(), 0, "the watcher is torn down");

    drop(monitor);
    dir.cleanup();
}

#[test]
fn many_subscriptions_on_one_directory_all_coalesce() {
    let dir = TempDir::new("coalesce-many");
    let monitor = Monitor::new().expect("create the monitor");
    let (session, _receiver) = monitor.session();

    let watches: Vec<Watch> = (0..16)
        .map(|_| {
            session
                .subscribe(dir.path(), WatchOptions::new())
                .expect("register")
        })
        .collect();
    monitor.quiesce();

    assert_eq!(monitor.watcher_count(), 16);
    assert_eq!(monitor.directory_count(), 1);

    drop(watches);
    drop(monitor);
    dir.cleanup();
}

// --- open-fault re-establishment (D-14/D-15/D-27, M5.1/M5.3) ---

#[test]
fn a_target_that_does_not_exist_yet_establishes_once_it_appears() {
    let dir = TempDir::new("monitor-retry-appears");
    let target = dir.path().join("not-yet");
    let monitor = Monitor::new().expect("create the monitor");
    let (session, receiver) = monitor.session();
    let watch = session
        .subscribe(&target, WatchOptions::new())
        .expect("register the subscription");
    monitor.quiesce();

    assert_eq!(monitor.is_faulted(watch.id()), Some(true));
    assert!(!monitor.is_watching(watch.id()));

    std::fs::create_dir(&target).expect("create the target directory");

    let deadline = Instant::now() + Duration::from_secs(10);
    while !monitor.is_watching(watch.id()) {
        assert!(
            Instant::now() < deadline,
            "the subscription never established once its target appeared"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(monitor.is_faulted(watch.id()), Some(false));

    drop(receiver);
    drop(watch);
    drop(monitor);
    dir.cleanup();
}

#[test]
fn an_interactive_subscription_is_asked_and_its_answer_speeds_up_establishment() {
    let dir = TempDir::new("monitor-retry-interactive");
    let target = dir.path().join("not-yet");
    let monitor = Monitor::new().expect("create the monitor");
    let (session, receiver) = monitor.session();
    let watch = session
        .subscribe(
            &target,
            WatchOptions::new().retry(crate::watch::RetryMode::Interactive),
        )
        .expect("register the subscription");
    monitor.quiesce();

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut asked = false;
    while !asked {
        assert!(
            Instant::now() < deadline,
            "the interactive subscription was never asked"
        );
        if let Some(notification) = receiver.try_recv()
            && matches!(notification, Notification::RetryQuestion { watch: id, .. } if id == watch.id())
        {
            asked = true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    std::fs::create_dir(&target).expect("create the target directory");
    session.answer(watch.id(), Some(Duration::from_millis(1)));

    let deadline = Instant::now() + Duration::from_secs(10);
    while !monitor.is_watching(watch.id()) {
        assert!(
            Instant::now() < deadline,
            "the subscription never established after answering"
        );
        std::thread::sleep(Duration::from_millis(5));
    }

    drop(receiver);
    drop(watch);
    drop(monitor);
    dir.cleanup();
}
