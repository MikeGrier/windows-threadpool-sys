// Copyright (c) 2026 Mike Grier
//! Unit tests for the detailed single-directory watcher.
//!
//! Every test drives a real `ReadDirectoryChangesW` against a real temp
//! directory, because what is under test is the arm/complete/re-arm loop against
//! the kernel, not a model of it.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use super::{ArmGate, DirectoryWatcher, ReadBuffer};
use crate::directory::DirectoryHandle;
use crate::notify::{ChangeKind, DesyncCause};
use crate::queue::{Notification, Receiver, WatchId, channel};
use crate::testing::TempDir;

/// Upper bound for waiting on a notification the kernel really should deliver.
const NOTIFY_TIMEOUT: Duration = Duration::from_secs(30);

/// The subscription every test in this module watches under.
fn test_watch() -> WatchId {
    WatchId::from_raw(1)
}

/// Drains the queue in the background, so a test can assert on what has arrived
/// without ever running test code on the crate's cadence path.
///
/// This mirrors what a real client does: the crate enqueues, the client receives
/// on a thread of its own choosing.
struct Drained {
    seen: Arc<std::sync::Mutex<Vec<Notification>>>,
    _pump: std::thread::JoinHandle<()>,
}

impl Drained {
    fn start(receiver: Receiver) -> Self {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        // `recv` returns None once every sender is gone, so this thread exits on
        // teardown rather than needing a stop flag.
        let pump = std::thread::spawn(move || {
            while let Some(item) = receiver.recv() {
                sink.lock().expect("record").push(item);
            }
        });
        Self { seen, _pump: pump }
    }

    fn notifications(&self) -> Vec<Notification> {
        self.seen.lock().expect("read").clone()
    }

    /// Every change across every delivered batch, flattened.
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

    /// Block until `predicate` holds over the drained notifications, failing
    /// rather than hanging if it never does.
    fn wait_until<F>(&self, what: &str, predicate: F)
    where
        F: Fn(&Drained) -> bool,
    {
        let deadline = std::time::Instant::now() + NOTIFY_TIMEOUT;
        while !predicate(self) {
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for {what}; saw {:?}",
                self.notifications()
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn wait_for_any(&self) {
        self.wait_until("any notification", |d| !d.notifications().is_empty());
    }

    fn wait_for_name(&self, name: &str) {
        self.wait_until(&format!("a change named {name}"), |d| {
            d.changes().iter().any(|(_, seen)| seen == name)
        });
    }
}

/// Start a watcher over `dir`, draining everything it enqueues.
fn watch(dir: &Path, subtree: bool) -> (DirectoryWatcher, Drained) {
    let (sender, receiver) = channel();
    let handle = DirectoryHandle::open(dir).expect("open the directory");
    let watcher =
        DirectoryWatcher::start(handle, subtree, test_watch(), sender).expect("start the watcher");
    (watcher, Drained::start(receiver))
}

// --- the buffer ---

#[test]
fn the_completion_buffer_is_dword_aligned() {
    // ReadDirectoryChangesW requires DWORD alignment, which a Box<[u8]> would
    // not guarantee. This is the reason the buffer is u32-backed.
    for bytes in [1_usize, 3, 4, 17, 1024, 64 * 1024] {
        let mut buffer = ReadBuffer::new(bytes);
        let address = buffer.as_mut_ptr() as usize;
        assert_eq!(
            address % 4,
            0,
            "buffer of {bytes} bytes is not DWORD-aligned"
        );
    }
}

#[test]
fn the_completion_buffer_rounds_up_and_is_never_empty() {
    assert_eq!(
        ReadBuffer::new(0).byte_len(),
        4,
        "a zero request still allocates"
    );
    assert_eq!(ReadBuffer::new(1).byte_len(), 4);
    assert_eq!(ReadBuffer::new(4).byte_len(), 4);
    assert_eq!(ReadBuffer::new(5).byte_len(), 8);
    assert_eq!(ReadBuffer::new(64 * 1024).byte_len(), 64 * 1024);
}

#[test]
fn the_completion_buffer_clamps_an_overlong_fill() {
    // A length beyond the allocation must be clamped rather than trusted, or a
    // bad completion length would be an out-of-bounds read.
    let buffer = ReadBuffer::new(16);
    assert_eq!(buffer.filled(usize::MAX).len(), 16);
    assert_eq!(buffer.filled(8).len(), 8);
    assert_eq!(buffer.filled(0).len(), 0);
}

// --- arming ---

#[test]
fn starting_a_watch_arms_a_read() {
    let dir = TempDir::new("arm");
    let (watcher, _collected) = watch(dir.path(), false);
    assert!(watcher.is_watching());
    assert!(watcher.stop_reason().is_none());
    drop(watcher);
    dir.cleanup();
}

#[test]
fn a_watcher_can_be_dropped_with_a_read_outstanding() {
    // The read is still pending here; teardown must cancel and drain rather than
    // block forever or leak the operation.
    let dir = TempDir::new("drop-outstanding");
    let (watcher, _collected) = watch(dir.path(), false);
    drop(watcher);
    dir.cleanup();
}

#[test]
fn many_watchers_can_start_and_stop_on_one_directory() {
    let dir = TempDir::new("many-watchers");
    for _ in 0..16 {
        let (watcher, _collected) = watch(dir.path(), false);
        drop(watcher);
    }
    dir.cleanup();
}

// --- completions ---

#[test]
fn creating_a_file_reports_it_as_added() {
    let dir = TempDir::new("added");
    let (watcher, collected) = watch(dir.path(), false);

    std::fs::write(dir.path().join("created.txt"), b"x").expect("create a file");
    collected.wait_for_name("created.txt");

    let changes = collected.changes();
    assert!(
        changes
            .iter()
            .any(|(kind, name)| *kind == ChangeKind::Added && name == "created.txt"),
        "expected an Added for created.txt, saw {changes:?}"
    );
    assert!(
        collected
            .notifications()
            .iter()
            .all(|item| item.watch() == test_watch()),
        "every notification is tagged with the subscription it belongs to"
    );
    drop(watcher);
    dir.cleanup();
}

#[test]
fn deleting_a_file_reports_it_as_removed() {
    let dir = TempDir::new("removed");
    let target = dir.path().join("doomed.txt");
    std::fs::write(&target, b"x").expect("create a file");

    let (watcher, collected) = watch(dir.path(), false);
    std::fs::remove_file(&target).expect("remove the file");
    collected.wait_for_name("doomed.txt");

    let changes = collected.changes();
    assert!(
        changes
            .iter()
            .any(|(kind, name)| *kind == ChangeKind::Removed && name == "doomed.txt"),
        "expected a Removed for doomed.txt, saw {changes:?}"
    );
    drop(watcher);
    dir.cleanup();
}

#[test]
fn renaming_a_file_reports_both_halves_distinctly() {
    // D-9: the crate never joins a rename into one event.
    let dir = TempDir::new("renamed");
    let before = dir.path().join("before.txt");
    std::fs::write(&before, b"x").expect("create a file");

    let (watcher, collected) = watch(dir.path(), false);
    std::fs::rename(&before, dir.path().join("after.txt")).expect("rename");

    // Both halves can straddle a completion boundary, so wait until both are in.
    collected.wait_until("both rename halves", |d| {
        let changes = d.changes();
        changes
            .iter()
            .any(|(kind, name)| *kind == ChangeKind::RenamedOldName && name == "before.txt")
            && changes
                .iter()
                .any(|(kind, name)| *kind == ChangeKind::RenamedNewName && name == "after.txt")
    });

    drop(watcher);
    dir.cleanup();
}

#[test]
fn the_watcher_re_arms_and_reports_a_later_change() {
    // The point of the re-arm: a second, independent change after the first
    // completion has already been delivered.
    let dir = TempDir::new("re-arm");
    let (watcher, collected) = watch(dir.path(), false);

    std::fs::write(dir.path().join("first.txt"), b"x").expect("first");
    collected.wait_for_name("first.txt");
    std::fs::write(dir.path().join("second.txt"), b"x").expect("second");
    collected.wait_for_name("second.txt");

    assert!(watcher.is_watching(), "still watching after re-arming");
    drop(watcher);
    dir.cleanup();
}

#[test]
fn many_sequential_changes_are_all_reported() {
    let dir = TempDir::new("sequential");
    let (watcher, collected) = watch(dir.path(), false);

    const COUNT: usize = 12;
    for index in 0..COUNT {
        std::fs::write(dir.path().join(format!("file-{index}.txt")), b"x").expect("create");
        // One at a time, so each has its own completion to be reported in and
        // the test exercises repeated re-arming rather than one batch.
        collected.wait_for_name(&format!("file-{index}.txt"));
    }

    assert!(watcher.is_watching());
    drop(watcher);
    dir.cleanup();
}

#[test]
fn a_subtree_watch_reports_a_change_in_a_child_directory() {
    let dir = TempDir::new("subtree");
    let child = dir.path().join("child");
    std::fs::create_dir(&child).expect("create child dir");

    let (watcher, collected) = watch(dir.path(), true);
    std::fs::write(child.join("nested.txt"), b"x").expect("create nested");
    collected.wait_until("the nested file", |d| {
        d.changes()
            .iter()
            .any(|(_, name)| name.contains("nested.txt"))
    });

    let changes = collected.changes();
    // The name is relative to the opened directory (D-8), so it includes the
    // child component rather than being the bare leaf.
    assert!(
        changes
            .iter()
            .any(|(_, name)| name.contains('\\') || name.contains("child")),
        "the name is relative to the watched directory, saw {changes:?}"
    );
    drop(watcher);
    dir.cleanup();
}

#[test]
fn a_non_subtree_watch_ignores_a_change_in_a_child_directory() {
    let dir = TempDir::new("no-subtree");
    let child = dir.path().join("child");
    std::fs::create_dir(&child).expect("create child dir");

    let (watcher, collected) = watch(dir.path(), false);
    std::fs::write(child.join("nested.txt"), b"x").expect("create nested");
    // Then a change that *is* in scope, so there is a definite point by which
    // the out-of-scope one would have arrived if it were going to.
    std::fs::write(dir.path().join("direct.txt"), b"x").expect("create direct");
    collected.wait_for_name("direct.txt");

    let changes = collected.changes();
    assert!(
        !changes.iter().any(|(_, name)| name.contains("nested.txt")),
        "a non-recursive watch must not report a nested change, saw {changes:?}"
    );
    drop(watcher);
    dir.cleanup();
}

// --- overflow ---

#[test]
fn a_tiny_buffer_surfaces_the_kernel_overflow_as_a_desync() {
    // The kernel reports overflow as a zero-byte completion, which the decoder
    // turns into Desync { Overflow } (D-12). A 4-byte buffer cannot hold even one
    // record, so any change overflows it.
    let dir = TempDir::new("overflow");
    let (sender, receiver) = channel();
    let handle = DirectoryHandle::open(dir.path()).expect("open");
    let watcher =
        DirectoryWatcher::start_with(handle, false, 4, test_watch(), sender).expect("start");
    let collected = Drained::start(receiver);

    let deadline = std::time::Instant::now() + NOTIFY_TIMEOUT;
    let mut index = 0;
    loop {
        std::fs::write(dir.path().join(format!("burst-{index}.txt")), b"x").expect("create");
        index += 1;
        if collected.desyncs().contains(&DesyncCause::Overflow) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "a 4-byte buffer never produced an overflow desync"
        );
        // Throttled rather than spinning: an untimed loop here creates files as
        // fast as the filesystem accepts them, which buries the temp directory
        // and makes cleanup dominate the test.
        std::thread::sleep(Duration::from_millis(5));
    }

    drop(watcher);
    dir.cleanup();
}

// --- teardown ---

#[test]
fn dropping_while_changes_are_arriving_does_not_deadlock() {
    // Regression: teardown cancels the outstanding read and waits for rundown,
    // but a completion callback already running could re-arm *after* the
    // cancellation. Rundown would then wait forever, because only a future
    // directory change could complete that fresh read. Arming is gated against
    // teardown to make that impossible.
    for _ in 0..8 {
        let dir = TempDir::new("teardown-race");
        let (watcher, collected) = watch(dir.path(), false);

        let churn_dir = dir.path().to_path_buf();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let churn_stop = Arc::clone(&stop);
        let churn = std::thread::spawn(move || {
            let mut index = 0;
            while !churn_stop.load(std::sync::atomic::Ordering::Relaxed) {
                let _ = std::fs::write(churn_dir.join(format!("churn-{index}.txt")), b"x");
                index += 1;
                std::thread::sleep(Duration::from_millis(1));
            }
        });

        // Drop while completions are actively being delivered and re-armed.
        collected.wait_for_any();
        drop(watcher);

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        churn.join().expect("churn thread");
        dir.cleanup();
    }
}

#[test]
fn a_dropped_watcher_stops_delivering() {
    let dir = TempDir::new("stops-delivering");
    let (watcher, collected) = watch(dir.path(), false);

    std::fs::write(dir.path().join("before-drop.txt"), b"x").expect("create");
    collected.wait_for_name("before-drop.txt");
    drop(watcher);

    // Rundown has completed, so no further callback can run and the count is
    // settled; anything created now must never be delivered.
    let settled = collected.notifications().len();
    std::fs::write(dir.path().join("after-drop.txt"), b"x").expect("create");
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        collected.notifications().len(),
        settled,
        "a dropped watcher must deliver nothing further"
    );
    assert!(
        !collected
            .changes()
            .iter()
            .any(|(_, name)| name == "after-drop.txt"),
        "a change after teardown must never be reported"
    );

    dir.cleanup();
}

// --- teardown (M2.4) ---

#[test]
fn stop_is_idempotent_and_drop_after_it_is_safe() {
    // Teardown is one operation with two triggers, so an explicit stop followed
    // by the implicit one must not double-cancel or double-drain.
    let dir = TempDir::new("stop-idempotent");
    let (watcher, _collected) = watch(dir.path(), false);

    watcher.stop();
    watcher.stop();
    watcher.stop();
    assert_eq!(watcher.gate(), ArmGate::TornDown);
    assert!(!watcher.is_watching());

    drop(watcher);
    dir.cleanup();
}

#[test]
fn stop_closes_the_gate_permanently() {
    // A watcher never re-opens: teardown is the one permanent reason for the
    // not-re-arming state, unlike the fault and backpressure reasons to come.
    let dir = TempDir::new("gate-permanent");
    let (watcher, collected) = watch(dir.path(), false);
    assert_eq!(watcher.gate(), ArmGate::Open);
    assert!(watcher.is_watching());

    watcher.stop();
    assert_eq!(watcher.gate(), ArmGate::TornDown);

    // A change after teardown must not re-open anything or produce a delivery.
    std::fs::write(dir.path().join("after-stop.txt"), b"x").expect("create");
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        !collected
            .changes()
            .iter()
            .any(|(_, name)| name == "after-stop.txt"),
        "a stopped watcher must not report anything further"
    );

    drop(watcher);
    dir.cleanup();
}

#[test]
fn teardown_releases_the_sender_so_the_receiver_disconnects() {
    // A client's drain loop terminates on `recv() -> None`. If teardown left the
    // sender alive the loop would block forever on a queue nothing can fill,
    // which is a hang rather than a shutdown.
    let dir = TempDir::new("teardown-disconnect");
    let (sender, receiver) = channel();
    let handle = DirectoryHandle::open(dir.path()).expect("open");
    let watcher =
        DirectoryWatcher::start(handle, false, test_watch(), sender).expect("start the watcher");

    assert!(!receiver.is_disconnected(), "live while the watcher exists");
    drop(watcher);

    assert!(
        receiver.is_disconnected(),
        "dropping the watcher must release the queue sender"
    );
    assert!(
        receiver.recv().is_none(),
        "a drain loop must terminate rather than block"
    );

    dir.cleanup();
}

#[test]
fn teardown_with_a_read_outstanding_completes_promptly() {
    // Nothing is changing in the directory, so the outstanding read can only be
    // retired by the cancellation. If teardown waited on the read instead of
    // cancelling it, this would hang rather than fail.
    let dir = TempDir::new("teardown-outstanding");
    let (watcher, _collected) = watch(dir.path(), false);

    let started = std::time::Instant::now();
    drop(watcher);
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_secs(5),
        "teardown took {elapsed:?}; the outstanding read was not cancelled"
    );
    dir.cleanup();
}

#[test]
fn teardown_is_prompt_even_with_a_subtree_watch_over_a_populated_tree() {
    // A recursive watch over a deep tree has more for the kernel to unwind, so
    // it is the case most likely to expose a teardown that waits rather than
    // cancels.
    let dir = TempDir::new("teardown-deep");
    let mut nested = dir.path().to_path_buf();
    for level in 0..12 {
        nested = nested.join(format!("level-{level}"));
    }
    std::fs::create_dir_all(&nested).expect("create deep tree");
    for index in 0..32 {
        std::fs::write(nested.join(format!("f-{index}.txt")), b"x").expect("write");
    }

    let (watcher, _collected) = watch(dir.path(), true);
    let started = std::time::Instant::now();
    drop(watcher);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "recursive teardown did not converge promptly"
    );
    dir.cleanup();
}

#[test]
fn many_watchers_tear_down_concurrently_without_wedging() {
    // Each watcher runs its own cancel-and-drain against a shared pool, so this
    // is where a teardown that blocked a pool thread would starve the others.
    let dir = TempDir::new("teardown-concurrent");
    let mut watchers = Vec::new();
    let mut drains = Vec::new();
    for _ in 0..16 {
        let (watcher, collected) = watch(dir.path(), false);
        watchers.push(watcher);
        drains.push(collected);
    }

    // Churn so completions are in flight while the teardowns run.
    for index in 0..8 {
        std::fs::write(dir.path().join(format!("churn-{index}.txt")), b"x").expect("write");
    }

    let started = std::time::Instant::now();
    let handles: Vec<_> = watchers
        .into_iter()
        .map(|watcher| std::thread::spawn(move || drop(watcher)))
        .collect();
    for handle in handles {
        handle.join().expect("teardown thread");
    }
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "concurrent teardown wedged"
    );

    dir.cleanup();
}

#[test]
fn teardown_from_a_thread_other_than_the_creator_is_safe() {
    // `DirectoryWatcher` is owned, and its teardown blocks; moving it to another
    // thread to be dropped is the shape a monitor will use when it releases a
    // watcher from its servicing path rather than from the caller's.
    let dir = TempDir::new("teardown-moved");
    let (watcher, collected) = watch(dir.path(), false);
    std::fs::write(dir.path().join("before-move.txt"), b"x").expect("write");
    collected.wait_for_name("before-move.txt");

    std::thread::spawn(move || drop(watcher))
        .join()
        .expect("teardown thread");

    dir.cleanup();
}
