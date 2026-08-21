// Copyright (c) 2026 Mike Grier
//! Unit tests for the monitor.
//!
//! The servicing path's own guarantees are tested in `src/servicing/tests.rs`;
//! what is here is the monitor's ownership of watchers and the blocking teardown
//! of D-20.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::Monitor;
use crate::directory::DirectoryHandle;
use crate::queue::{Receiver, WatchId, channel};
use crate::watcher::DirectoryWatcher;

/// What teardown is allowed to take. Cancellation retires an outstanding read at
/// once, so this only fires if teardown waited for a change instead.
const TEARDOWN_BUDGET: Duration = Duration::from_secs(5);

/// A uniquely named temp directory, removed when the test passes.
///
/// Cleanup is deliberately not RAII: a failure leaves the tree for post-mortem
/// inspection.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "windows-file-watcher-monitor-{label}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&path).expect("create temp dir");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn cleanup(self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// A watcher over `dir` with a read already armed, plus the receiver its sender
/// feeds. The receiver disconnects when the watcher is torn down, which is how a
/// test observes that teardown actually released it.
fn armed_watcher(dir: &Path, watch: u64) -> (DirectoryWatcher, Receiver) {
    let handle = DirectoryHandle::open(dir).expect("open the watched directory");
    let (sender, receiver) = channel();
    let watcher = DirectoryWatcher::start(handle, false, WatchId::from_raw(watch), sender)
        .expect("arm the first read");
    (watcher, receiver)
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
fn an_adopted_watcher_is_counted() {
    let dir = TempDir::new("adopt");
    let monitor = Monitor::new().expect("create the monitor");
    let (watcher, _receiver) = armed_watcher(dir.path(), 1);

    monitor.adopt(watcher);
    assert_eq!(monitor.watcher_count(), 1);

    drop(monitor);
    dir.cleanup();
}

#[test]
fn shutdown_releases_every_adopted_watcher() {
    let dir = TempDir::new("release");
    let monitor = Monitor::new().expect("create the monitor");

    let mut receivers = Vec::new();
    for watch in 0..4 {
        let (watcher, receiver) = armed_watcher(dir.path(), watch);
        monitor.adopt(watcher);
        receivers.push(receiver);
    }
    assert_eq!(monitor.watcher_count(), 4);

    monitor.shut_down();
    assert_eq!(monitor.watcher_count(), 0);

    // A watcher owns its queue sender, so a disconnected receiver is proof the
    // monitor really released it rather than merely forgetting the table entry.
    for receiver in &receivers {
        assert!(
            receiver.is_disconnected(),
            "teardown must release the watcher, not just drop the reference to it"
        );
    }

    dir.cleanup();
}

#[test]
fn teardown_with_reads_outstanding_converges_promptly() {
    let dir = TempDir::new("outstanding");
    let monitor = Monitor::new().expect("create the monitor");

    for watch in 0..8 {
        let (watcher, _receiver) = armed_watcher(dir.path(), watch);
        monitor.adopt(watcher);
    }

    // Nothing will change this directory again, so only cancellation can retire
    // the eight outstanding reads. A teardown that waited would sit here until the
    // budget expired.
    let started = Instant::now();
    monitor.shut_down();
    let elapsed = started.elapsed();
    assert!(
        elapsed < TEARDOWN_BUDGET,
        "teardown took {elapsed:?}, which means it waited rather than cancelled"
    );

    dir.cleanup();
}

#[test]
fn dropping_a_monitor_tears_down_its_watchers() {
    let dir = TempDir::new("drop");
    let monitor = Monitor::new().expect("create the monitor");
    let (watcher, receiver) = armed_watcher(dir.path(), 1);
    monitor.adopt(watcher);

    drop(monitor);

    assert!(
        receiver.is_disconnected(),
        "`Drop` must run the same teardown as an explicit shutdown"
    );

    dir.cleanup();
}

#[test]
fn teardown_from_a_thread_other_than_the_creator_is_safe() {
    let dir = TempDir::new("thread");
    let monitor = Monitor::new().expect("create the monitor");
    let (watcher, receiver) = armed_watcher(dir.path(), 1);
    monitor.adopt(watcher);

    std::thread::spawn(move || drop(monitor))
        .join()
        .expect("teardown thread");

    assert!(receiver.is_disconnected());
    dir.cleanup();
}

#[test]
fn many_monitors_tear_down_concurrently_without_wedging() {
    let dir = TempDir::new("concurrent");
    let root = dir.path().to_path_buf();

    let workers: Vec<_> = (0..8)
        .map(|index| {
            let root = root.clone();
            std::thread::spawn(move || {
                let monitor = Monitor::new().expect("create the monitor");
                let (watcher, receiver) = armed_watcher(&root, index);
                monitor.adopt(watcher);
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
    let dir = TempDir::new("debug");
    let monitor = Monitor::new().expect("create the monitor");
    let (watcher, _receiver) = armed_watcher(dir.path(), 1);
    monitor.adopt(watcher);

    let rendered = format!("{monitor:?}");
    assert!(rendered.contains("running: true"), "{rendered}");
    assert!(rendered.contains("watchers: 1"), "{rendered}");

    drop(monitor);
    dir.cleanup();
}
