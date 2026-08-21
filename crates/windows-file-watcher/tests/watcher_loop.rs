// Copyright (c) 2026 Mike Grier
//! Integration test for the M2 arm / complete / re-arm loop, driven end to end
//! against a real directory.
//!
//! The unit tests in `src/watcher/tests.rs` establish that the loop works. What
//! is here is what only shows up at scale or at the edges: that the raw actions
//! and relative names the kernel reports survive decoding in order, that a burst
//! large enough to outrun the completion buffer is reported as a
//! [`DesyncCause::Overflow`] rather than silently dropped, and that teardown with
//! a read genuinely outstanding converges promptly instead of waiting for a
//! change that will never come.
//!
//! The loop is not reachable from the crate's public surface -- that ships only
//! the decoder until M3 -- so this test drives the `unstable-internals` module.
//! It is deleted along with that module when the `Monitor` / `Session` / `Watch`
//! surface lands.
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows_file_watcher::unstable::{
    ArmGate, DirectoryHandle, DirectoryWatcher, Notification, Receiver, WatchId, channel,
};
use windows_file_watcher::{ChangeKind, DesyncCause};

/// Upper bound for waiting on something the kernel really should deliver. Long
/// enough that a loaded CI runner does not fail spuriously; short enough that a
/// genuine wedge fails the run rather than stalling it.
const NOTIFY_TIMEOUT: Duration = Duration::from_secs(30);

/// What teardown is allowed to take. Cancellation retires the outstanding read
/// immediately, so this is generous by two orders of magnitude -- it is here to
/// catch a teardown that *waits* rather than cancels, which would otherwise hang
/// until some unrelated change happened to complete the read.
const TEARDOWN_BUDGET: Duration = Duration::from_secs(5);

/// How many files the scale test creates. The point is a volume that a single
/// 64 KiB completion cannot carry, so the assertion exercises many completions
/// and the re-arm between them.
const SCALE_FILES: usize = 1_000;

/// A completion buffer too small to hold even one record, so the kernel has no
/// choice but to signal an overflow on the very first change.
///
/// Overflow is otherwise a race the test cannot win reliably: the kernel only
/// discards when records pile up during the window between a completion and the
/// re-arm, and nothing outside the crate can widen that window. Driving a burst
/// against a roomy buffer *does* overflow, but it was measured between 1.5 and
/// 15 seconds for the same assertion -- a flake waiting for a loaded runner.
/// Undersizing the buffer forces the same kernel path in bounded time.
const UNDERSIZED_BUFFER_BYTES: usize = 16;

/// A uniquely named temp directory, removed when the test passes.
///
/// Cleanup is deliberately not RAII: an assertion failure leaves the tree behind
/// for post-mortem inspection, which for a watcher test is often the only record
/// of what the kernel actually saw.
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
            "windows-file-watcher-loop-{label}-{}-{nonce}",
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

/// Drains the queue on a thread of its own, exactly as a client would.
///
/// Nothing in a test ever runs on the crate's cadence path: the watcher enqueues
/// from a pool callback and this receives elsewhere, so a slow assertion here
/// cannot perturb the loop under test.
struct Collected {
    seen: Arc<Mutex<Vec<Notification>>>,
    pump: std::thread::JoinHandle<()>,
}

impl Collected {
    fn start(receiver: Receiver) -> Self {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        // `recv` yields `None` once every sender is gone, so this thread ends at
        // teardown without needing a stop flag.
        let pump = std::thread::spawn(move || {
            while let Some(item) = receiver.recv() {
                sink.lock().expect("record").push(item);
            }
        });
        Self { seen, pump }
    }

    fn notifications(&self) -> Vec<Notification> {
        self.seen.lock().expect("read").clone()
    }

    /// Every change across every delivered batch, flattened, in delivery order.
    fn changes(&self) -> Vec<(ChangeKind, String)> {
        self.notifications()
            .into_iter()
            .filter_map(|item| match item {
                Notification::Batch { changes, .. } => Some(changes),
                Notification::Desync { .. } | Notification::Completion { .. } => None,
            })
            .flatten()
            .map(|change| {
                (
                    change.kind,
                    change.name.to_os_string().to_string_lossy().into_owned(),
                )
            })
            .collect()
    }

    fn desyncs(&self) -> Vec<DesyncCause> {
        self.notifications()
            .into_iter()
            .filter_map(|item| match item {
                Notification::Desync { cause, .. } => Some(cause),
                Notification::Batch { .. } | Notification::Completion { .. } => None,
            })
            .collect()
    }

    /// Block until `predicate` holds, failing rather than hanging if it never
    /// does.
    fn wait_until<F>(&self, what: &str, predicate: F)
    where
        F: Fn(&Collected) -> bool,
    {
        let deadline = Instant::now() + NOTIFY_TIMEOUT;
        while !predicate(self) {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}; saw {:?}",
                self.notifications()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

/// The watcher every test drives, plus its drained queue.
fn watch(dir: &Path, subtree: bool, buffer_bytes: usize) -> (DirectoryWatcher, Collected) {
    let handle = DirectoryHandle::open(dir).expect("open the watched directory");
    let (sender, receiver) = channel();
    let watcher =
        DirectoryWatcher::start_with(handle, subtree, buffer_bytes, WatchId::from_raw(1), sender)
            .expect("arm the first read");
    (watcher, Collected::start(receiver))
}

/// The default completion-buffer size, for the tests that are not about
/// overflow. Mirrors the crate's own default rather than picking a number.
fn roomy_buffer() -> usize {
    windows_file_watcher::unstable::DEFAULT_BUFFER_BYTES
}

/// The index of the first change matching `kind` and `name`, if any.
fn position(changes: &[(ChangeKind, String)], kind: ChangeKind, name: &str) -> Option<usize> {
    changes
        .iter()
        .position(|(seen_kind, seen_name)| *seen_kind == kind && seen_name == name)
}

fn has(changes: &[(ChangeKind, String)], kind: ChangeKind, name: &str) -> bool {
    position(changes, kind, name).is_some()
}

#[test]
fn create_modify_and_delete_are_reported_with_relative_names_in_order() {
    let dir = TempDir::new("crud");
    let (watcher, collected) = watch(dir.path(), false, roomy_buffer());

    let file = dir.path().join("alpha.txt");
    std::fs::write(&file, b"one").expect("create");
    collected.wait_until("the create", |c| {
        has(&c.changes(), ChangeKind::Added, "alpha.txt")
    });

    std::fs::write(&file, b"two, rather longer than one").expect("modify");
    collected.wait_until("the modify", |c| {
        has(&c.changes(), ChangeKind::Modified, "alpha.txt")
    });

    std::fs::remove_file(&file).expect("delete");
    collected.wait_until("the delete", |c| {
        has(&c.changes(), ChangeKind::Removed, "alpha.txt")
    });

    // Order is a promise, not an accident: the queue preserves the sequence the
    // kernel reported, so a client replaying it reaches the same end state.
    let changes = collected.changes();
    let added = position(&changes, ChangeKind::Added, "alpha.txt").expect("the create");
    let removed = position(&changes, ChangeKind::Removed, "alpha.txt").expect("the delete");
    assert!(
        added < removed,
        "the create must be reported before the delete, saw {changes:?}"
    );

    // The name is relative to the watched directory, never the full path.
    assert!(
        changes.iter().all(|(_, name)| !name.contains('\\')),
        "a non-recursive watch reports bare names, saw {changes:?}"
    );

    drop(watcher);
    dir.cleanup();
}

#[test]
fn a_rename_is_reported_as_an_old_name_and_a_new_name() {
    let dir = TempDir::new("rename");
    let before = dir.path().join("beta.txt");
    std::fs::write(&before, b"beta").expect("seed");

    let (watcher, collected) = watch(dir.path(), false, roomy_buffer());

    std::fs::rename(&before, dir.path().join("gamma.txt")).expect("rename");

    collected.wait_until("both halves of the rename", |c| {
        let changes = c.changes();
        has(&changes, ChangeKind::RenamedOldName, "beta.txt")
            && has(&changes, ChangeKind::RenamedNewName, "gamma.txt")
    });

    // The two halves are one event split across two records; a client pairing
    // them relies on the old name arriving first.
    let changes = collected.changes();
    let old = position(&changes, ChangeKind::RenamedOldName, "beta.txt").expect("old name");
    let new = position(&changes, ChangeKind::RenamedNewName, "gamma.txt").expect("new name");
    assert!(
        old < new,
        "the old name must precede the new name, saw {changes:?}"
    );

    drop(watcher);
    dir.cleanup();
}

#[test]
fn a_subtree_watch_reports_names_relative_to_the_watched_root() {
    let dir = TempDir::new("subtree");
    let (watcher, collected) = watch(dir.path(), true, roomy_buffer());

    let nested = dir.path().join("one").join("two");
    std::fs::create_dir_all(&nested).expect("create the nested directories");
    std::fs::write(nested.join("deep.txt"), b"deep").expect("create the nested file");

    collected.wait_until("the nested create", |c| {
        has(&c.changes(), ChangeKind::Added, "one\\two\\deep.txt")
    });

    drop(watcher);
    dir.cleanup();
}

#[test]
fn a_non_recursive_watch_ignores_changes_below_the_watched_directory() {
    let dir = TempDir::new("shallow");
    let (watcher, collected) = watch(dir.path(), false, roomy_buffer());

    let nested = dir.path().join("sub");
    std::fs::create_dir(&nested).expect("create the subdirectory");
    std::fs::write(nested.join("hidden.txt"), b"hidden").expect("create below it");

    // Absence only means something once the loop has demonstrably moved past the
    // nested write. Changes are reported in order, so waiting for a change made
    // *after* it establishes that: if the nested file were ever going to be
    // reported, it would already have been.
    std::fs::write(dir.path().join("marker.txt"), b"marker").expect("create the marker");
    collected.wait_until("a change made after the nested one", |c| {
        has(&c.changes(), ChangeKind::Added, "marker.txt")
    });

    let changes = collected.changes();
    assert!(
        has(&changes, ChangeKind::Added, "sub"),
        "the subdirectory itself is a change in the watched directory, saw {changes:?}"
    );
    assert!(
        !changes.iter().any(|(_, name)| name.contains("hidden.txt")),
        "a non-recursive watch must not report below itself, saw {changes:?}"
    );

    drop(watcher);
    dir.cleanup();
}

#[test]
fn an_overflowed_read_is_reported_as_a_desync_and_the_watch_continues() {
    let dir = TempDir::new("overflow");
    let (watcher, collected) = watch(dir.path(), false, UNDERSIZED_BUFFER_BYTES);

    std::fs::write(dir.path().join("first.txt"), b"first").expect("the first change");
    collected.wait_until("the overflow", |c| {
        c.desyncs().contains(&DesyncCause::Overflow)
    });

    // Losing changes is acceptable; losing them silently is not, and neither is
    // stopping because of it. The kernel keeps the watch armed after signalling
    // an overflow, and so must this crate -- a second one can only arrive if the
    // completion path re-armed after the first.
    std::fs::write(dir.path().join("second.txt"), b"second").expect("the second change");
    collected.wait_until("a second overflow, which requires a re-arm", |c| {
        c.desyncs()
            .iter()
            .filter(|cause| **cause == DesyncCause::Overflow)
            .count()
            >= 2
    });

    assert!(
        watcher.is_watching(),
        "an overflow must not stop the watcher: {:?}",
        watcher.stop_reason()
    );

    // Nothing was decoded, so nothing may be claimed as decoded. An overflow
    // that also delivered changes would mean the crate had invented them.
    assert!(
        collected.changes().is_empty(),
        "an overflowed read carries no records, saw {:?}",
        collected.changes()
    );

    drop(watcher);
    dir.cleanup();
}

#[test]
fn a_thousand_creates_are_either_all_reported_or_explicitly_desynced() {
    let dir = TempDir::new("scale");
    let (watcher, collected) = watch(dir.path(), false, roomy_buffer());

    for index in 0..SCALE_FILES {
        std::fs::write(dir.path().join(format!("file-{index:04}.txt")), b"scale")
            .expect("create at scale");
    }

    let last = format!("file-{:04}.txt", SCALE_FILES - 1);
    collected.wait_until("the whole burst to settle", |c| {
        has(&c.changes(), ChangeKind::Added, &last) || !c.desyncs().is_empty()
    });

    // The crate's central promise (D-12): a change is either delivered or its
    // loss is reported. "Neither" is the failure this asserts against, and at
    // this volume it spans many completions and the re-arms between them.
    if collected.desyncs().is_empty() {
        let changes = collected.changes();
        let missing: Vec<String> = (0..SCALE_FILES)
            .map(|index| format!("file-{index:04}.txt"))
            .filter(|name| !has(&changes, ChangeKind::Added, name))
            .collect();
        assert!(
            missing.is_empty(),
            "{} of {SCALE_FILES} creates were lost with no desync reported, first few: {:?}",
            missing.len(),
            &missing[..missing.len().min(5)]
        );
    }

    drop(watcher);
    dir.cleanup();
}

#[test]
fn teardown_with_a_read_outstanding_converges_promptly() {
    let dir = TempDir::new("teardown");
    let (watcher, collected) = watch(dir.path(), false, roomy_buffer());

    // Wait for a completion before tearing down, so the watcher has re-armed and
    // a fresh read is genuinely outstanding. Without this the test could tear
    // down before the first read was even issued and prove nothing.
    std::fs::write(dir.path().join("live.txt"), b"live").expect("create");
    collected.wait_until("the loop to prove it is armed", |c| {
        has(&c.changes(), ChangeKind::Added, "live.txt")
    });
    assert!(watcher.is_watching());

    // Nothing will change this directory again, so only cancellation can retire
    // the outstanding read. A teardown that waited instead would sit here until
    // the budget expired.
    let started = Instant::now();
    watcher.stop();
    let elapsed = started.elapsed();
    assert!(
        elapsed < TEARDOWN_BUDGET,
        "teardown took {elapsed:?}, which means it waited rather than cancelled"
    );
    assert_eq!(watcher.gate(), ArmGate::TornDown);
    assert!(!watcher.is_watching());

    // Releasing the watcher releases the queue's sender, so the client's drain
    // loop ends rather than blocking forever on a queue nothing can fill again.
    drop(watcher);
    let pump = collected.pump;
    let started = Instant::now();
    pump.join().expect("the drain thread ends at teardown");
    assert!(
        started.elapsed() < TEARDOWN_BUDGET,
        "the receiver did not observe the disconnect"
    );

    dir.cleanup();
}
