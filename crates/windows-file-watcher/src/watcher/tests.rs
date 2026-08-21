// Copyright (c) 2026 Mike Grier
//! Unit tests for the detailed single-directory watcher.
//!
//! Every test drives a real `ReadDirectoryChangesW` against a real temp
//! directory, because what is under test is the arm/complete/re-arm loop against
//! the kernel, not a model of it.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use super::{DirectoryWatcher, ReadBuffer};
use crate::directory::DirectoryHandle;
use crate::notify::{ChangeKind, DecodedBatch, DesyncCause};

/// Upper bound for waiting on a notification the kernel really should deliver.
const NOTIFY_TIMEOUT: Duration = Duration::from_secs(30);

/// A uniquely named temp directory, removed when the test passes.
///
/// Cleanup is deliberately not RAII: an assertion failure leaves the directory
/// behind for post-mortem inspection.
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
            "windows-file-watcher-watch-{label}-{}-{nonce}",
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

/// Collects delivered batches and lets a test block until enough have arrived.
struct Collected {
    batches: Mutex<Vec<DecodedBatch>>,
    arrived: Condvar,
}

impl Collected {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            batches: Mutex::new(Vec::new()),
            arrived: Condvar::new(),
        })
    }

    fn push(&self, batch: DecodedBatch) {
        let mut batches = self.batches.lock().expect("record a batch");
        batches.push(batch);
        self.arrived.notify_all();
    }

    fn batches(&self) -> Vec<DecodedBatch> {
        self.batches.lock().expect("read batches").clone()
    }

    /// Block until at least `count` batches have arrived, failing rather than
    /// hanging if they never do.
    fn wait_for(&self, count: usize) -> Vec<DecodedBatch> {
        let batches = self.batches.lock().expect("await batches");
        let (batches, timeout) = self
            .arrived
            .wait_timeout_while(batches, NOTIFY_TIMEOUT, |batches| batches.len() < count)
            .expect("await batches");
        assert!(
            !timeout.timed_out(),
            "timed out waiting for {count} batch(es); saw {}",
            batches.len()
        );
        batches.clone()
    }

    /// Every change across every delivered batch, flattened.
    fn changes(&self) -> Vec<(ChangeKind, String)> {
        self.batches()
            .into_iter()
            .filter_map(|batch| match batch {
                DecodedBatch::Changes(changes) => Some(changes),
                DecodedBatch::Desync(_) => None,
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
        self.batches()
            .into_iter()
            .filter_map(|batch| match batch {
                DecodedBatch::Desync(cause) => Some(cause),
                DecodedBatch::Changes(_) => None,
            })
            .collect()
    }
}

/// Start a watcher over `dir`, collecting everything it delivers.
fn watch(dir: &Path, subtree: bool) -> (DirectoryWatcher, Arc<Collected>) {
    let collected = Collected::new();
    let sink = Arc::clone(&collected);
    let handle = DirectoryHandle::open(dir).expect("open the directory");
    let watcher = DirectoryWatcher::start(handle, subtree, move |batch| sink.push(batch))
        .expect("start the watcher");
    (watcher, collected)
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
    collected.wait_for(1);

    let changes = collected.changes();
    assert!(
        changes
            .iter()
            .any(|(kind, name)| *kind == ChangeKind::Added && name == "created.txt"),
        "expected an Added for created.txt, saw {changes:?}"
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
    collected.wait_for(1);

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
    collected.wait_for(1);

    // Both halves can straddle a completion boundary, so wait until both are in.
    let deadline = std::time::Instant::now() + NOTIFY_TIMEOUT;
    loop {
        let changes = collected.changes();
        let old = changes
            .iter()
            .any(|(kind, name)| *kind == ChangeKind::RenamedOldName && name == "before.txt");
        let new = changes
            .iter()
            .any(|(kind, name)| *kind == ChangeKind::RenamedNewName && name == "after.txt");
        if old && new {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "expected both rename halves, saw {changes:?}"
        );
        std::thread::yield_now();
    }

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
    collected.wait_for(1);
    std::fs::write(dir.path().join("second.txt"), b"x").expect("second");

    let deadline = std::time::Instant::now() + NOTIFY_TIMEOUT;
    loop {
        let changes = collected.changes();
        if changes.iter().any(|(_, name)| name == "second.txt") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the watcher did not re-arm; saw {changes:?}"
        );
        std::thread::yield_now();
    }

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
        let deadline = std::time::Instant::now() + NOTIFY_TIMEOUT;
        loop {
            if collected
                .changes()
                .iter()
                .any(|(_, name)| name == &format!("file-{index}.txt"))
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "file-{index}.txt was never reported"
            );
            std::thread::yield_now();
        }
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
    collected.wait_for(1);

    let changes = collected.changes();
    assert!(
        changes.iter().any(|(_, name)| name.contains("nested.txt")),
        "a subtree watch must see the nested file, saw {changes:?}"
    );
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
    collected.wait_for(1);

    let deadline = std::time::Instant::now() + NOTIFY_TIMEOUT;
    loop {
        if collected
            .changes()
            .iter()
            .any(|(_, name)| name == "direct.txt")
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "direct.txt never arrived"
        );
        std::thread::yield_now();
    }

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
    let collected = Collected::new();
    let sink = Arc::clone(&collected);
    let handle = DirectoryHandle::open(dir.path()).expect("open");
    let watcher = DirectoryWatcher::start_with(handle, false, 4, move |batch| sink.push(batch))
        .expect("start");

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
        collected.wait_for(1);
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
    collected.wait_for(1);
    drop(watcher);

    // Rundown has completed, so no further callback can run and the batch count
    // is settled; anything created now must never be delivered.
    let settled = collected.batches().len();
    std::fs::write(dir.path().join("after-drop.txt"), b"x").expect("create");
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        collected.batches().len(),
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
