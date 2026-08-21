// Copyright (c) 2026 Mike Grier
//! Unit tests for the monitor.
//!
//! The servicing path's own guarantees are tested in `src/servicing/tests.rs`;
//! what is here is the monitor's ownership of watchers -- now reached the way a
//! client reaches it, through a session's subscribe -- and the blocking teardown
//! of D-20.

use std::time::{Duration, Instant};

use super::Monitor;
use crate::queue::Receiver;
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
