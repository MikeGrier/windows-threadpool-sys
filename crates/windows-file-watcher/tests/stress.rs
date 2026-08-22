// Copyright (c) 2026 Mike Grier
//! Opt-in stress suite (M7.4): change churn, fault storms, teardown races, and
//! coalesced multi-subscription load, at volumes well beyond what the
//! ordinary integration suite exercises.
//!
//! Gated behind the `WINDOWS_FILE_WATCHER_STRESS` environment variable rather
//! than `#[ignore]`, so a normal `cargo test`/`cargo nextest run` still
//! compiles and enumerates every test here (catching a build break), but each
//! one returns immediately unless a caller opts in:
//!
//! ```text
//! $env:WINDOWS_FILE_WATCHER_STRESS = "1"
//! cargo test --test stress -- --test-threads=1
//! ```
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use windows_file_watcher::{Monitor, Notification, WatchOptions};

/// Whether the stress suite should actually run. Checked at the top of every
/// test, which is what makes this opt-in rather than `#[ignore]`: the tests
/// still compile and are still visible to `cargo test --list`.
fn stress_enabled() -> bool {
    std::env::var_os("WINDOWS_FILE_WATCHER_STRESS").is_some()
}

/// Upper bound generous enough for a loaded CI runner; a genuine wedge still
/// fails the run rather than hanging it forever.
const STRESS_TIMEOUT: Duration = Duration::from_secs(120);

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
            "windows-file-watcher-stress-{label}-{}-{nonce}",
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

/// Block until `predicate` holds, failing rather than hanging.
fn wait_for<F: FnMut() -> bool>(what: &str, mut predicate: F) {
    let deadline = Instant::now() + STRESS_TIMEOUT;
    while !predicate() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn change_churn_is_never_silently_lost() {
    if !stress_enabled() {
        return;
    }
    let dir = TempDir::new("churn");
    let monitor = Monitor::new().expect("create the monitor");
    let (session, receiver) = monitor.session();
    let watch = session
        .subscribe(dir.path(), WatchOptions::new())
        .expect("register");

    const FILES: usize = 20_000;
    let writer = {
        let target = dir.path().to_path_buf();
        std::thread::spawn(move || {
            for index in 0..FILES {
                std::fs::write(target.join(format!("churn-{index}.txt")), b"x")
                    .expect("create a file");
            }
        })
    };

    // Every batch and every desync is accounted for by construction (D-12): the
    // property under test is that the watch never wedges (stops delivering
    // anything at all) under sustained load, not that zero desyncs occur -- a
    // burst this large is expected to overflow the kernel's own buffer at least
    // once, and that is a correctly reported loss, not a bug.
    let mut seen_any = false;
    let mut settled = false;
    let deadline = Instant::now() + STRESS_TIMEOUT;
    while !settled {
        assert!(Instant::now() < deadline, "the watch stalled under churn");
        if let Some(notification) = receiver.recv_timeout(Duration::from_millis(200)) {
            seen_any = true;
            if matches!(notification, Notification::Desync { .. }) {
                // A desync is an explicit, honest "you may have missed
                // something" -- exactly what this crate promises never to hide.
            }
        } else if writer.is_finished() {
            settled = true;
        }
    }
    writer.join().expect("writer thread");
    assert!(
        seen_any,
        "no notification arrived during a 20,000-file churn"
    );

    drop(watch);
    drop(monitor);
    dir.cleanup();
}

#[test]
fn a_fault_storm_of_repeated_delete_recreate_always_reestablishes() {
    if !stress_enabled() {
        return;
    }
    let dir = TempDir::new("fault-storm");
    let monitor = Monitor::new().expect("create the monitor");
    let (session, receiver) = monitor.session();
    let watch = session
        .subscribe(dir.path(), WatchOptions::new().report_liveness(true))
        .expect("register");

    const ROUNDS: usize = 25;
    for round in 0..ROUNDS {
        std::fs::write(dir.path().join("marker.txt"), b"x").expect("mark the round");
        wait_for(&format!("round {round}'s marker"), || !receiver.is_empty());
        // Drain whatever arrived so the queue never saturates across 25 rounds.
        while receiver.try_recv().is_some() {}

        std::fs::remove_dir_all(dir.path()).expect("delete the watched directory");
        wait_for(&format!("round {round}'s suspension"), || {
            let mut suspended = false;
            while let Some(notification) = receiver.try_recv() {
                if matches!(notification, Notification::Suspended { .. }) {
                    suspended = true;
                }
            }
            suspended
        });

        std::fs::create_dir_all(dir.path()).expect("recreate the watched directory");
        wait_for(&format!("round {round}'s resumption"), || {
            let mut resumed = false;
            while let Some(notification) = receiver.try_recv() {
                if matches!(notification, Notification::Resumed { .. }) {
                    resumed = true;
                }
            }
            resumed
        });
    }

    drop(watch);
    drop(monitor);
    dir.cleanup();
}

#[test]
fn many_watchers_tear_down_concurrently_under_sustained_churn() {
    if !stress_enabled() {
        return;
    }
    const WATCHERS: usize = 64;

    let dir = TempDir::new("teardown-race");
    let monitor = Monitor::new().expect("create the monitor");
    let (session, receiver) = monitor.session();

    let mut watches = Vec::with_capacity(WATCHERS);
    for _ in 0..WATCHERS {
        watches.push(
            session
                .subscribe(dir.path(), WatchOptions::new())
                .expect("register"),
        );
    }
    monitor.quiesce();
    assert_eq!(monitor.watcher_count(), WATCHERS);

    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let churn = {
        let target = dir.path().to_path_buf();
        let stop = std::sync::Arc::clone(&stop);
        std::thread::spawn(move || {
            let mut index = 0;
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                let _ = std::fs::write(target.join(format!("churn-{index}.txt")), b"x");
                index += 1;
            }
        })
    };

    // Drop every watch concurrently, from several threads, while completions
    // are actively arriving -- the scenario D-23's gate exists to make safe.
    let started = Instant::now();
    let handles: Vec<_> = watches
        .into_iter()
        .map(|watch| std::thread::spawn(move || drop(watch)))
        .collect();
    for handle in handles {
        handle.join().expect("teardown thread");
    }
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "concurrent teardown of {WATCHERS} watchers wedged"
    );

    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    churn.join().expect("churn thread");

    drop(receiver);
    drop(monitor);
    dir.cleanup();
}

#[test]
fn a_deeply_coalesced_directory_delivers_to_every_subscription_under_load() {
    if !stress_enabled() {
        return;
    }
    const SUBSCRIPTIONS: usize = 128;
    const FILES: usize = 2_000;

    let dir = TempDir::new("coalesced-load");
    let monitor = Monitor::new().expect("create the monitor");
    let (session, receiver) = monitor.session();

    let mut watches = Vec::with_capacity(SUBSCRIPTIONS);
    for _ in 0..SUBSCRIPTIONS {
        watches.push(
            session
                .subscribe(dir.path(), WatchOptions::new())
                .expect("register"),
        );
    }
    monitor.quiesce();
    assert_eq!(
        monitor.watcher_count(),
        SUBSCRIPTIONS,
        "every subscription is registered, whether or not its watcher is coalesced (D-6)"
    );

    for index in 0..FILES {
        std::fs::write(dir.path().join(format!("f-{index}.txt")), b"x").expect("create");
    }

    let mut seen: std::collections::HashSet<windows_file_watcher::WatchId> =
        std::collections::HashSet::new();
    let deadline = Instant::now() + STRESS_TIMEOUT;
    while seen.len() < SUBSCRIPTIONS {
        assert!(
            Instant::now() < deadline,
            "only {} of {SUBSCRIPTIONS} subscriptions ever saw a change",
            seen.len()
        );
        if let Some(notification) = receiver.recv_timeout(Duration::from_millis(200))
            && let Notification::Batch { watch, changes } = notification
            && !changes.is_empty()
        {
            seen.insert(watch);
        }
    }

    drop(watches);
    drop(monitor);
    dir.cleanup();
}
