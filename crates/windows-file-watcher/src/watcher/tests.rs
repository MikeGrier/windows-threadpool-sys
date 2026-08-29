// Copyright (c) 2026 Mike Grier
//! Unit tests for the detailed single-directory watcher.
//!
//! Every test drives a real `ReadDirectoryChangesW` against a real temp
//! directory, because what is under test is the arm/complete/re-arm loop against
//! the kernel, not a model of it.

use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use windows_sys::Win32::Foundation::ERROR_NOT_SUPPORTED;

use super::{ArmGate, DirectoryWatcher, ReadBuffer, lock};
use crate::directory::{
    DirectoryHandle, FailureCode, FaultDetail, OpenFailure, VolumeIdentity, classify_detail,
};
use crate::notify::{ChangeKind, DesyncCause};
use crate::queue::{Notification, Receiver, Sender, WatchId, channel, channel_with_bound};
use crate::retry::{FaultOperation, WatchMode};
use crate::route::{Route, RouteScope};
use crate::testing::TempDir;
use crate::watch::{RetryMode, VolumeChangeDecision, VolumeChangePolicy};

/// Upper bound for waiting on a notification the kernel really should deliver.
const NOTIFY_TIMEOUT: Duration = Duration::from_secs(30);

/// The subscription every test in this module watches under.
fn test_watch() -> WatchId {
    WatchId::from_raw(1)
}

/// A route with no fault-recovery involvement: `Defaults` retry, no liveness
/// reporting, no standing fault slot. What every test in this module needs
/// unless it is specifically exercising M5's fault machinery.
fn plain_route(watch: WatchId, scope: RouteScope, sink: Sender) -> Route {
    Route {
        watch,
        scope,
        sink,
        retry: RetryMode::Defaults,
        report_liveness: false,
        on_volume_change: VolumeChangePolicy::AutoContinue,
        fault_slot: None,
    }
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
                Notification::Desync { .. }
                | Notification::Completion { .. }
                | Notification::Suspended { .. }
                | Notification::Resumed { .. }
                | Notification::Established { .. }
                | Notification::RetryQuestion { .. }
                | Notification::VolumeChanged { .. } => None,
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
                Notification::Batch { .. }
                | Notification::Completion { .. }
                | Notification::Suspended { .. }
                | Notification::Resumed { .. }
                | Notification::Established { .. }
                | Notification::RetryQuestion { .. }
                | Notification::VolumeChanged { .. } => None,
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
    let route = plain_route(test_watch(), RouteScope::Directory { subtree }, sender);
    let watcher =
        DirectoryWatcher::start(handle, dir.to_path_buf(), route).expect("start the watcher");
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
    let route = plain_route(
        test_watch(),
        RouteScope::Directory { subtree: false },
        sender,
    );
    let watcher =
        DirectoryWatcher::start_with(handle, dir.path().to_path_buf(), 4, route).expect("start");
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

    // Received inline rather than through `Drained`, deliberately. What this
    // test claims is that teardown makes a later change *unobservable*, and a
    // background pump puts an unsynchronised thread between the queue and the
    // assertion: a notification enqueued before teardown but drained after the
    // count was sampled is indistinguishable from a post-teardown delivery. That
    // is not hypothetical -- it is what made this test intermittent on CI, and a
    // longer sleep would only have widened the window it depends on. Receiving
    // here lets the drain be a checked fact instead of a race against a sleep.
    let (sender, receiver) = channel();
    let handle = DirectoryHandle::open(dir.path()).expect("open the directory");
    let route = plain_route(
        test_watch(),
        RouteScope::Directory { subtree: false },
        sender,
    );
    let watcher = DirectoryWatcher::start(handle, dir.path().to_path_buf(), route)
        .expect("start the watcher");

    fn record(item: Notification, into: &mut Vec<String>) {
        if let Notification::Batch { changes, .. } = item {
            into.extend(
                changes
                    .into_iter()
                    .map(|change| change.name.to_os_string().to_string_lossy().into_owned()),
            );
        }
    }

    // One `std::fs::write` is a create *and* a write, so it yields more than one
    // notification, and they need not share a completion. Waiting only for the
    // first change named `before-drop.txt` therefore leaves the rest in flight
    // on purpose: an in-flight completion is precisely the state teardown has to
    // be correct in, and the state this test exists to cover.
    let mut delivered: Vec<String> = Vec::new();
    std::fs::write(dir.path().join("before-drop.txt"), b"x").expect("create");
    let deadline = std::time::Instant::now() + NOTIFY_TIMEOUT;
    while !delivered.iter().any(|name| name == "before-drop.txt") {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for a change named before-drop.txt; saw {delivered:?}"
        );
        if let Some(item) = receiver.recv_timeout(Duration::from_millis(50)) {
            record(item, &mut delivered);
        }
    }

    drop(watcher);
    std::fs::write(dir.path().join("after-drop.txt"), b"x").expect("create");

    // Teardown releases the last sender, so the queue can be drained to the end
    // of the stream rather than to an arbitrary deadline. Drain until
    // `recv_timeout` yields `None`, then assert `is_disconnected` -- that second
    // step is load-bearing rather than decorative, because `recv_timeout`
    // reports `None` on a timeout too, and only the disconnection distinguishes
    // "no sender remains" from "nothing arrived in time". A timeout would end
    // the loop early and leave the assertions below covering part of the
    // delivered set while appearing to cover all of it.
    //
    // Past a genuine disconnection nothing can ever be added, so everything the
    // watcher will ever deliver is in `delivered` -- including anything a
    // still-running callback managed to enqueue, since a live callback implies a
    // live sender and would keep the drain going.
    while let Some(item) = receiver.recv_timeout(NOTIFY_TIMEOUT) {
        record(item, &mut delivered);
    }
    assert!(
        receiver.is_disconnected(),
        "teardown must release the last sender; the drain above stopped on a \
         timeout rather than on the end of the stream, so the assertions below \
         would not cover the whole delivered set. Saw {delivered:?}"
    );

    assert!(
        !delivered.iter().any(|name| name == "after-drop.txt"),
        "a change after teardown must never be reported; saw {delivered:?}"
    );
    assert!(
        delivered.iter().all(|name| name == "before-drop.txt"),
        "a dropped watcher must deliver nothing further; saw {delivered:?}"
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
    let route = plain_route(
        test_watch(),
        RouteScope::Directory { subtree: false },
        sender,
    );
    let watcher = DirectoryWatcher::start(handle, dir.path().to_path_buf(), route)
        .expect("start the watcher");

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

// --- backpressure (D-29) ---

/// A watcher over `dir` feeding a queue of exactly `bound` notifications, with no
/// one draining it.
fn watch_bounded(dir: &Path, bound: usize) -> (DirectoryWatcher, Receiver) {
    let handle = DirectoryHandle::open(dir).expect("open the watched directory");
    let (sender, receiver) =
        channel_with_bound(NonZeroUsize::new(bound).expect("a non-zero bound"));
    let route = plain_route(
        test_watch(),
        RouteScope::Directory { subtree: false },
        sender,
    );
    let watcher =
        DirectoryWatcher::start(handle, dir.to_path_buf(), route).expect("arm the first read");
    (watcher, receiver)
}

/// Block until `predicate` holds, failing rather than hanging.
fn wait_for<F: FnMut() -> bool>(what: &str, mut predicate: F) {
    let deadline = std::time::Instant::now() + NOTIFY_TIMEOUT;
    while !predicate() {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Make changes until `predicate` holds, failing rather than hanging.
fn churn_until<F: Fn() -> bool>(dir: &Path, what: &str, predicate: F) {
    let deadline = std::time::Instant::now() + NOTIFY_TIMEOUT;
    let mut index = 0_usize;
    while !predicate() {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what} after {index} changes"
        );
        std::fs::write(dir.join(format!("churn-{index}.txt")), b"x").expect("create");
        index += 1;
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_full_queue_stops_the_re_arm_rather_than_dropping() {
    let dir = TempDir::new("backpressure-stop");
    let (watcher, receiver) = watch_bounded(dir.path(), 1);

    churn_until(dir.path(), "the watcher to pause", || {
        watcher.gate() == ArmGate::Backpressured
    });

    assert_eq!(watcher.gate(), ArmGate::Backpressured);
    assert!(
        watcher.is_watching(),
        "a paused watcher is not a stopped one: {:?}",
        watcher.stop_reason()
    );
    assert!(receiver.len() <= 1, "the bound still holds");

    drop(watcher);
    dir.cleanup();
}

#[test]
fn draining_resumes_a_paused_watcher() {
    let dir = TempDir::new("backpressure-resume");
    let (watcher, receiver) = watch_bounded(dir.path(), 1);

    churn_until(dir.path(), "the watcher to pause", || {
        watcher.gate() == ArmGate::Backpressured
    });

    // Nothing else will prod it: the resume has to come from the client draining.
    while receiver.try_recv().is_some() {}
    wait_for("the watcher to resume", || watcher.gate() == ArmGate::Open);

    // And it is really watching again, not merely reporting that it is.
    std::fs::write(dir.path().join("after.txt"), b"after").expect("create");
    wait_for("a change after the pause", || {
        !receiver.is_empty() || receiver.latched() > 0
    });

    drop(watcher);
    dir.cleanup();
}

#[test]
fn a_pause_is_a_grace_period_rather_than_a_loss() {
    // The property that makes refusing to re-arm better than dropping at the
    // enqueue: with no read outstanding the kernel keeps accumulating, so a
    // client that drains in time loses nothing at all (D-29).
    let dir = TempDir::new("backpressure-grace");
    let (watcher, receiver) = watch_bounded(dir.path(), 4);

    churn_until(dir.path(), "the watcher to pause", || {
        watcher.gate() == ArmGate::Backpressured
    });

    // Made while no read is armed, so this exists only in the kernel's buffer.
    std::fs::write(dir.path().join("during-the-pause.txt"), b"x").expect("create");

    let mut seen: Vec<String> = Vec::new();
    let deadline = std::time::Instant::now() + NOTIFY_TIMEOUT;
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "the change made during the pause never arrived; saw {seen:?}"
        );
        while let Some(item) = receiver.try_recv() {
            if let Notification::Batch { changes, .. } = item {
                seen.extend(
                    changes
                        .iter()
                        .map(|c| c.name.to_os_string().to_string_lossy().into_owned()),
                );
            }
        }
        if seen.iter().any(|name| name == "during-the-pause.txt") {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    drop(watcher);
    dir.cleanup();
}

#[test]
fn teardown_while_paused_is_prompt() {
    // A paused watcher has no read outstanding, so teardown must not wait for a
    // completion that is never coming.
    let dir = TempDir::new("backpressure-teardown");
    let (watcher, _receiver) = watch_bounded(dir.path(), 1);

    churn_until(dir.path(), "the watcher to pause", || {
        watcher.gate() == ArmGate::Backpressured
    });

    let started = std::time::Instant::now();
    watcher.stop();
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "teardown of a paused watcher took {:?}",
        started.elapsed()
    );
    assert_eq!(watcher.gate(), ArmGate::TornDown);

    drop(watcher);
    dir.cleanup();
}

#[test]
fn a_torn_down_watcher_is_not_resumed_by_a_drain() {
    // The gate's reasons are not interchangeable: teardown is permanent, and a
    // drain that prods every registered producer must not re-open it.
    let dir = TempDir::new("backpressure-torn");
    let (watcher, receiver) = watch_bounded(dir.path(), 1);

    churn_until(dir.path(), "the watcher to pause", || {
        watcher.gate() == ArmGate::Backpressured
    });
    watcher.stop();

    while receiver.try_recv().is_some() {}
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(
        watcher.gate(),
        ArmGate::TornDown,
        "a drain must not resurrect a torn-down watcher"
    );

    drop(watcher);
    dir.cleanup();
}

#[test]
fn pausing_and_resuming_repeatedly_does_not_wedge() {
    // The lost-wakeup hazard the gate's re-check exists for: a resume that runs
    // between the room check and the gate being set would otherwise leave the
    // watcher parked with room available and nothing left to prod it. Cycling
    // many times makes that window likely to be hit.
    let dir = TempDir::new("backpressure-cycle");
    let (watcher, receiver) = watch_bounded(dir.path(), 2);

    for round in 0..20 {
        churn_until(dir.path(), "the watcher to pause", || {
            watcher.gate() == ArmGate::Backpressured
        });
        while receiver.try_recv().is_some() {}
        wait_for("the watcher to resume", || watcher.gate() == ArmGate::Open);
        assert!(
            watcher.is_watching(),
            "round {round}: {:?}",
            watcher.stop_reason()
        );
    }

    drop(watcher);
    dir.cleanup();
}

#[test]
fn an_overflowed_read_is_reported_as_a_desync_and_the_watch_continues() {
    // Moved here from the integration suite when M3.8 retired the test-only
    // surface: forcing an overflow needs the completion buffer undersized below a
    // single record, and buffer size is deliberately not a client's business.
    //
    // Overflow is otherwise a race a test cannot win reliably -- the kernel only
    // discards when records pile up in the window between a completion and the
    // re-arm, and nothing outside this crate can widen that window. Driving a
    // burst against a roomy buffer does overflow, but was measured between 1.5
    // and 15 seconds for the same assertion.
    const UNDERSIZED_BUFFER_BYTES: usize = 16;

    let dir = TempDir::new("watcher-overflow");
    let handle = DirectoryHandle::open(dir.path()).expect("open the watched directory");
    let (sender, receiver) = channel();
    let route = plain_route(
        test_watch(),
        RouteScope::Directory { subtree: false },
        sender,
    );
    let watcher = DirectoryWatcher::start_with(
        handle,
        dir.path().to_path_buf(),
        UNDERSIZED_BUFFER_BYTES,
        route,
    )
    .expect("arm the first read");

    let overflows = |receiver: &Receiver| {
        let mut count = 0;
        while let Some(item) = receiver.try_recv() {
            if matches!(
                item,
                Notification::Desync {
                    cause: DesyncCause::Overflow,
                    ..
                }
            ) {
                count += 1;
            }
        }
        count
    };

    let mut seen = 0;
    std::fs::write(dir.path().join("first.txt"), b"first").expect("the first change");
    wait_for("the overflow", || {
        seen += overflows(&receiver);
        seen >= 1
    });

    // A second one can only arrive if the completion path re-armed after the
    // first: an overflow is a report, not a stop.
    std::fs::write(dir.path().join("second.txt"), b"second").expect("the second change");
    wait_for("a second overflow, which requires a re-arm", || {
        seen += overflows(&receiver);
        seen >= 2
    });

    assert!(
        watcher.is_watching(),
        "an overflow must not stop the watcher: {:?}",
        watcher.stop_reason()
    );

    drop(watcher);
    dir.cleanup();
}

// --- adding and removing routes without disturbing the others (D-6/M4.4) ---

/// Start a watcher over `dir` with one route, returning the sender too so a
/// test can add a second route delivering to the same drained receiver.
fn watch_with_sink(dir: &Path, subtree: bool) -> (DirectoryWatcher, Sender, Drained) {
    let (sender, receiver) = channel();
    let handle = DirectoryHandle::open(dir).expect("open the directory");
    let route = plain_route(
        test_watch(),
        RouteScope::Directory { subtree },
        sender.clone(),
    );
    let watcher =
        DirectoryWatcher::start(handle, dir.to_path_buf(), route).expect("start the watcher");
    (watcher, sender, Drained::start(receiver))
}

#[test]
fn adding_a_shallow_route_to_a_shallow_watcher_needs_no_rearm() {
    let dir = TempDir::new("route-add-shallow");
    let (watcher, sink, collected) = watch_with_sink(dir.path(), false);

    watcher.add_route(
        plain_route(
            WatchId::from_raw(2),
            RouteScope::Directory { subtree: false },
            sink,
        ),
        DirectoryHandle::open(dir.path()).expect("open a second handle"),
    );

    std::fs::write(dir.path().join("a.txt"), b"x").expect("create");
    collected.wait_for_name("a.txt");

    drop(watcher);
    dir.cleanup();
}

#[test]
fn a_recursive_route_added_to_a_shallow_watcher_widens_its_reach() {
    // The case D-6/M4.4 exists for: a second subscription asks for recursion the
    // first one never needed, and the *first* subscription's own read has to
    // start reaching nested changes too, since there is only one kernel read per
    // directory. Widening reopens the directory (see `WatcherInner::reopen`):
    // cancelling and resubmitting on the same handle was tried first and
    // measured not to work -- the kernel kept reporting only direct children.
    let dir = TempDir::new("route-widen");
    let (watcher, sink, collected) = watch_with_sink(dir.path(), false);

    let nested = dir.path().join("nested");
    std::fs::create_dir(&nested).expect("create the nested directory");
    std::fs::write(nested.join("deep.txt"), b"x").expect("create a nested file");
    std::thread::sleep(Duration::from_millis(100));
    assert!(
        !collected
            .changes()
            .iter()
            .any(|(_, name)| name.contains("deep.txt")),
        "a shallow watcher must not have reported the nested file yet"
    );

    watcher.add_route(
        plain_route(
            WatchId::from_raw(2),
            RouteScope::Directory { subtree: true },
            sink,
        ),
        DirectoryHandle::open(dir.path()).expect("open a second handle"),
    );

    std::fs::write(nested.join("deep2.txt"), b"y").expect("create a second nested file");
    collected.wait_for_name("nested\\deep2.txt");

    drop(watcher);
    dir.cleanup();
}

#[test]
fn a_concurrent_widen_and_retry_reestablish_do_not_corrupt_the_watcher() {
    // PR #20 review response: `install`'s ArmGate::Reopening marks state but
    // did not serialize the reopen transaction. A widening `add_route` and
    // the retry timer's `retry_reestablish` could both pass the "not
    // TornDown" gate check and run their own teardown/establish/arm sequence
    // concurrently, each capable of tearing down or replacing the endpoint
    // the other had just installed. Driving both at once here, repeatedly,
    // is the regression test for `WatcherInner::reopen_lock` serializing
    // them: whichever ordering the scheduler picks, the watcher must end up
    // functional rather than wedged or panicking.
    let dir = TempDir::new("route-widen-concurrent-reopen");
    let (watcher, sink, collected) = watch_with_sink(dir.path(), false);
    let watcher = std::sync::Arc::new(watcher);

    let nested = dir.path().join("nested");
    std::fs::create_dir(&nested).expect("create the nested directory");

    let retrying = {
        let watcher = std::sync::Arc::clone(&watcher);
        std::thread::spawn(move || {
            for _ in 0..20 {
                watcher.inner.retry_reestablish();
            }
        })
    };
    watcher.add_route(
        plain_route(
            WatchId::from_raw(2),
            RouteScope::Directory { subtree: true },
            sink,
        ),
        DirectoryHandle::open(dir.path()).expect("open a second handle"),
    );
    retrying.join().expect("the retry thread panicked");

    assert_ne!(
        watcher.gate(),
        ArmGate::TornDown,
        "the watcher must still be functional after the race"
    );
    std::fs::write(nested.join("after-race.txt"), b"z").expect("create a nested file");
    collected.wait_for_name("nested\\after-race.txt");

    drop(watcher);
    dir.cleanup();
}

#[test]
fn removing_the_only_recursive_route_leaves_the_watcher_functional() {
    // Contraction is never forced (see `DirectoryWatcher::add_route`'s docs): the
    // remaining shallow route just gets an over-broad read filtered back down,
    // which costs nothing but a few decoded bytes.
    let dir = TempDir::new("route-remove-recursive");
    let (watcher, sink, collected) = watch_with_sink(dir.path(), true);

    watcher.add_route(
        plain_route(
            WatchId::from_raw(2),
            RouteScope::Directory { subtree: false },
            sink,
        ),
        DirectoryHandle::open(dir.path()).expect("open a second handle"),
    );
    assert_eq!(watcher.remove_route(test_watch()).0, 1, "one route remains");

    std::fs::write(dir.path().join("a.txt"), b"x").expect("create");
    collected.wait_for_name("a.txt");

    drop(watcher);
    dir.cleanup();
}

#[test]
fn removing_every_route_is_observed_by_the_caller() {
    let dir = TempDir::new("route-remove-all");
    let (watcher, collected) = watch(dir.path(), false);
    assert_eq!(watcher.remove_route(test_watch()).0, 0);
    drop(collected);
    drop(watcher);
    dir.cleanup();
}

// --- fault recovery and the retry protocol (D-14/D-15/D-27, M5.1-M5.6) ---

/// An interactive route (D-27), with its standing fault-question reservation
/// (D-28) taken from `sink`.
fn interactive_route(watch: WatchId, sink: Sender, report_liveness: bool) -> Route {
    let fault_slot = sink
        .reserve_standing()
        .expect("room for the standing fault-question slot");
    Route {
        watch,
        scope: RouteScope::Directory { subtree: false },
        sink,
        retry: RetryMode::Interactive,
        report_liveness,
        on_volume_change: VolumeChangePolicy::AutoContinue,
        fault_slot: Some(fault_slot),
    }
}

#[test]
fn a_fresh_watcher_reports_not_faulted() {
    let dir = TempDir::new("fault-not-yet");
    let (watcher, _collected) = watch(dir.path(), false);
    assert!(!watcher.is_faulted());
    drop(watcher);
    dir.cleanup();
}

#[test]
fn the_current_fault_reports_its_classification_and_code() {
    let dir = TempDir::new("fault-detail");
    let (watcher, _collected) = watch(dir.path(), false);
    assert_eq!(watcher.fault_detail(), None);

    let error = std::io::Error::from_raw_os_error(ERROR_NOT_SUPPORTED as i32);
    watcher
        .inner
        .enter_fault(classify_detail(&error), FaultOperation::Arm);

    assert_eq!(
        watcher.fault_detail(),
        Some(FaultDetail {
            failure: OpenFailure::Unsupported,
            code: FailureCode::Win32(ERROR_NOT_SUPPORTED),
        })
    );

    drop(watcher);
    dir.cleanup();
}

#[test]
fn a_fault_with_only_default_routes_closes_the_gate_until_the_timer_resolves_it() {
    let dir = TempDir::new("fault-default");
    let (watcher, _collected) = watch(dir.path(), false);

    watcher.inner.enter_fault(
        classify_detail(&std::io::Error::other("synthetic")),
        FaultOperation::Arm,
    );

    assert!(watcher.is_faulted());
    assert_eq!(watcher.gate(), ArmGate::Faulted);
    assert!(
        watcher.is_watching(),
        "a fault is recovering, not a permanent stop"
    );

    drop(watcher);
    dir.cleanup();
}

#[test]
fn an_interactive_route_is_asked_and_its_answer_resolves_the_fault() {
    let dir = TempDir::new("fault-interactive");
    let (sender, receiver) = channel();
    let handle = DirectoryHandle::open(dir.path()).expect("open");
    let route = interactive_route(test_watch(), sender, false);
    let watcher = DirectoryWatcher::start(handle, dir.path().to_path_buf(), route).expect("start");
    let collected = Drained::start(receiver);

    watcher.inner.enter_fault(
        classify_detail(&std::io::Error::other("synthetic")),
        FaultOperation::Arm,
    );
    assert!(watcher.is_faulted());
    collected.wait_until("the retry question", |d| {
        d.notifications().iter().any(
            |n| matches!(n, Notification::RetryQuestion { watch, .. } if *watch == test_watch()),
        )
    });

    watcher.answer(test_watch(), Some(Duration::from_millis(1)));

    let deadline = std::time::Instant::now() + NOTIFY_TIMEOUT;
    while watcher.is_faulted() {
        assert!(
            std::time::Instant::now() < deadline,
            "the fault never resolved after being answered"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(watcher.is_watching());

    drop(watcher);
    dir.cleanup();
}

#[test]
fn the_earliest_of_several_answers_wins_and_a_decliner_counts_at_the_default() {
    let dir = TempDir::new("fault-earliest");
    let (sender_a, _receiver_a) = channel();
    let (sender_b, _receiver_b) = channel();
    let handle = DirectoryHandle::open(dir.path()).expect("open");
    let watch_a = WatchId::from_raw(10);
    let watch_b = WatchId::from_raw(11);
    let route_a = interactive_route(watch_a, sender_a, false);
    let watcher =
        DirectoryWatcher::start(handle, dir.path().to_path_buf(), route_a).expect("start");
    let fresh = DirectoryHandle::open(dir.path()).expect("a second handle");
    watcher.add_route(interactive_route(watch_b, sender_b, false), fresh);

    watcher.inner.enter_fault(
        classify_detail(&std::io::Error::other("synthetic")),
        FaultOperation::Arm,
    );
    assert!(watcher.is_faulted());

    // `watch_a` declines (counted at the 500ms default); `watch_b` names the
    // floor. The earliest of the two must win regardless of answer order.
    watcher.answer(watch_a, None);
    watcher.answer(watch_b, Some(Duration::from_millis(1)));

    let deadline = std::time::Instant::now() + NOTIFY_TIMEOUT;
    while watcher.is_faulted() {
        assert!(
            std::time::Instant::now() < deadline,
            "the fault never resolved once both routes answered"
        );
        std::thread::sleep(Duration::from_millis(5));
    }

    drop(watcher);
    dir.cleanup();
}

#[test]
fn removing_the_only_awaited_route_resolves_the_fault_rather_than_wedging_it() {
    let dir = TempDir::new("fault-cancel-awaited");
    let (sender, _receiver) = channel();
    let handle = DirectoryHandle::open(dir.path()).expect("open");
    let route = interactive_route(test_watch(), sender, false);
    let watcher = DirectoryWatcher::start(handle, dir.path().to_path_buf(), route).expect("start");

    watcher.inner.enter_fault(
        classify_detail(&std::io::Error::other("synthetic")),
        FaultOperation::Arm,
    );
    assert!(watcher.is_faulted());

    assert_eq!(
        watcher.remove_route(test_watch()).0,
        0,
        "the only route is gone"
    );

    let deadline = std::time::Instant::now() + NOTIFY_TIMEOUT;
    while watcher.is_faulted() {
        assert!(
            std::time::Instant::now() < deadline,
            "removing the last awaited route must resolve the fault, not wedge it"
        );
        std::thread::sleep(Duration::from_millis(5));
    }

    drop(watcher);
    dir.cleanup();
}

#[test]
fn a_resolved_fault_reports_reestablished_and_the_opt_in_liveness_brackets() {
    let dir = TempDir::new("fault-resolved-notify");
    let (sender, receiver) = channel();
    let handle = DirectoryHandle::open(dir.path()).expect("open");
    let route = Route {
        watch: test_watch(),
        scope: RouteScope::Directory { subtree: false },
        sink: sender,
        retry: RetryMode::Interactive,
        report_liveness: true,
        on_volume_change: VolumeChangePolicy::AutoContinue,
        fault_slot: None,
    };
    // Interactive with no fault_slot: never asked (there is nowhere to ask), so
    // this resolves at the default delay exactly like a Defaults route would.
    let watcher = DirectoryWatcher::start(handle, dir.path().to_path_buf(), route).expect("start");
    let collected = Drained::start(receiver);

    watcher.inner.enter_fault(
        classify_detail(&std::io::Error::other("synthetic")),
        FaultOperation::Arm,
    );
    collected.wait_until("suspended", |d| {
        d.notifications()
            .iter()
            .any(|n| matches!(n, Notification::Suspended { .. }))
    });

    let deadline = std::time::Instant::now() + NOTIFY_TIMEOUT;
    while watcher.is_faulted() {
        assert!(
            std::time::Instant::now() < deadline,
            "the default-delay fault never resolved"
        );
        std::thread::sleep(Duration::from_millis(5));
    }

    collected.wait_until("resumed, established, and reestablished", |d| {
        let seen = d.notifications();
        seen.iter()
            .any(|n| matches!(n, Notification::Resumed { .. }))
            && seen
                .iter()
                .any(|n| matches!(n, Notification::Established { .. }))
            && seen.iter().any(|n| {
                matches!(
                    n,
                    Notification::Desync {
                        cause: DesyncCause::Reestablished,
                        ..
                    }
                )
            })
    });

    drop(watcher);
    dir.cleanup();
}

// --- the coarse fallback (D-17, M6) ---

#[test]
fn forcing_coarse_establishes_in_coarse_mode() {
    let dir = TempDir::new("coarse-forced");
    let (sender, _receiver) = channel();
    let handle = DirectoryHandle::open(dir.path()).expect("open");
    let route = plain_route(
        test_watch(),
        RouteScope::Directory { subtree: false },
        sender,
    );
    let watcher = DirectoryWatcher::start_forcing_coarse(handle, dir.path().to_path_buf(), route)
        .expect("start in forced-coarse mode");

    assert_eq!(watcher.mode(), WatchMode::Coarse);
    assert!(watcher.is_watching());
    assert!(!watcher.is_faulted());

    drop(watcher);
    dir.cleanup();
}

#[test]
fn a_forced_coarse_watch_reports_a_change_as_desync_coarse() {
    let dir = TempDir::new("coarse-desync");
    let (sender, receiver) = channel();
    let handle = DirectoryHandle::open(dir.path()).expect("open");
    let route = plain_route(
        test_watch(),
        RouteScope::Directory { subtree: false },
        sender,
    );
    let watcher = DirectoryWatcher::start_forcing_coarse(handle, dir.path().to_path_buf(), route)
        .expect("start in forced-coarse mode");
    let collected = Drained::start(receiver);

    std::fs::write(dir.path().join("changed.txt"), b"x").expect("create a file");
    collected.wait_until("a coarse desync", |d| {
        d.desyncs().contains(&DesyncCause::Coarse)
    });

    assert!(
        watcher.is_watching(),
        "a coarse activation must not stop the watcher: {:?}",
        watcher.stop_reason()
    );

    drop(watcher);
    dir.cleanup();
}

#[test]
fn a_recovered_fault_in_forced_coarse_mode_reports_established_coarse() {
    let dir = TempDir::new("coarse-established");
    let (sender, receiver) = channel();
    let handle = DirectoryHandle::open(dir.path()).expect("open");
    let route = Route {
        watch: test_watch(),
        scope: RouteScope::Directory { subtree: false },
        sink: sender,
        retry: RetryMode::Defaults,
        report_liveness: true,
        on_volume_change: VolumeChangePolicy::AutoContinue,
        fault_slot: None,
    };
    let watcher = DirectoryWatcher::start_forcing_coarse(handle, dir.path().to_path_buf(), route)
        .expect("start in forced-coarse mode");
    let collected = Drained::start(receiver);

    watcher.inner.enter_fault(
        classify_detail(&std::io::Error::other("synthetic")),
        FaultOperation::Arm,
    );

    collected.wait_until("resumed, established coarse, and reestablished", |d| {
        let seen = d.notifications();
        seen.iter()
            .any(|n| matches!(n, Notification::Resumed { .. }))
            && seen.iter().any(|n| {
                matches!(
                    n,
                    Notification::Established {
                        mode: WatchMode::Coarse,
                        ..
                    }
                )
            })
            && seen.iter().any(|n| {
                matches!(
                    n,
                    Notification::Desync {
                        cause: DesyncCause::Reestablished,
                        ..
                    }
                )
            })
    });
    assert_eq!(watcher.mode(), WatchMode::Coarse);

    drop(watcher);
    dir.cleanup();
}

#[test]
fn teardown_of_a_forced_coarse_watch_is_prompt() {
    let dir = TempDir::new("coarse-teardown");
    let (sender, _receiver) = channel();
    let handle = DirectoryHandle::open(dir.path()).expect("open");
    let route = plain_route(
        test_watch(),
        RouteScope::Directory { subtree: false },
        sender,
    );
    let watcher = DirectoryWatcher::start_forcing_coarse(handle, dir.path().to_path_buf(), route)
        .expect("start in forced-coarse mode");

    let started = std::time::Instant::now();
    drop(watcher);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "coarse teardown did not converge promptly"
    );
    dir.cleanup();
}

// --- volume-change confirmation (D-78, M12) ---

/// A route that has opted in to confirming a volume change (M12.1), with its
/// standing question reservation taken from `sink`.
fn confirm_route(watch: WatchId, sink: Sender) -> Route {
    let fault_slot = sink
        .reserve_standing()
        .expect("room for the standing question slot");
    Route {
        watch,
        scope: RouteScope::Directory { subtree: false },
        sink,
        retry: RetryMode::Defaults,
        report_liveness: false,
        on_volume_change: VolumeChangePolicy::Confirm,
        fault_slot: Some(fault_slot),
    }
}

#[test]
fn only_the_confirm_route_is_asked_and_continuing_keeps_both_routes() {
    let dir = TempDir::new("volume-change-continue");
    let (sender_a, receiver_a) = channel();
    let (sender_b, receiver_b) = channel();
    let watch_a = WatchId::from_raw(20);
    let watch_b = WatchId::from_raw(21);

    let handle = DirectoryHandle::open(dir.path()).expect("open");
    let route_a = confirm_route(watch_a, sender_a);
    let watcher =
        DirectoryWatcher::start(handle, dir.path().to_path_buf(), route_a).expect("start");
    let fresh_handle = DirectoryHandle::open(dir.path()).expect("open again to coalesce");
    watcher.add_route(
        plain_route(watch_b, RouteScope::Directory { subtree: false }, sender_b),
        fresh_handle,
    );

    // Rig the recorded baseline so the *real* current volume identity this
    // watcher reads fresh below is guaranteed to differ from it -- a real
    // removable-media swap is not otherwise reproducible in an automated test.
    *lock(&watcher.inner.volume_identity) = Some(VolumeIdentity::synthetic(
        0xDEAD_BEEF,
        "FAKE-FS",
        "FAKE-LABEL",
    ));
    watcher.inner.retry_reestablish();

    assert_eq!(watcher.gate(), ArmGate::VolumeChangePending);

    let collected_a = Drained::start(receiver_a);
    collected_a.wait_until("the volume-change question", |d| {
        d.notifications()
            .iter()
            .any(|n| matches!(n, Notification::VolumeChanged { watch, .. } if *watch == watch_a))
    });
    assert!(
        receiver_b.try_recv().is_none(),
        "an AutoContinue route must never be asked"
    );

    let (remaining, _stopped) = watcher
        .answer_volume_change(watch_a, VolumeChangeDecision::Continue)
        .expect("this answer resolves the question");
    assert_eq!(remaining, 2, "continuing keeps both routes");

    let deadline = std::time::Instant::now() + NOTIFY_TIMEOUT;
    while watcher.gate() == ArmGate::VolumeChangePending {
        assert!(
            std::time::Instant::now() < deadline,
            "the watcher never resumed arming after the volume change was confirmed"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(watcher.gate(), ArmGate::Open);

    drop(watcher);
    dir.cleanup();
}

#[test]
fn stopping_a_volume_change_removes_only_that_route() {
    let dir = TempDir::new("volume-change-stop");
    let (sender_a, receiver_a) = channel();
    let (sender_b, receiver_b) = channel();
    let watch_a = WatchId::from_raw(22);
    let watch_b = WatchId::from_raw(23);

    let handle = DirectoryHandle::open(dir.path()).expect("open");
    let route_a = confirm_route(watch_a, sender_a);
    let watcher =
        DirectoryWatcher::start(handle, dir.path().to_path_buf(), route_a).expect("start");
    let fresh_handle = DirectoryHandle::open(dir.path()).expect("open again to coalesce");
    watcher.add_route(
        plain_route(watch_b, RouteScope::Directory { subtree: false }, sender_b),
        fresh_handle,
    );

    *lock(&watcher.inner.volume_identity) = Some(VolumeIdentity::synthetic(
        0xDEAD_BEEF,
        "FAKE-FS",
        "FAKE-LABEL",
    ));
    watcher.inner.retry_reestablish();
    assert_eq!(watcher.gate(), ArmGate::VolumeChangePending);

    let collected_a = Drained::start(receiver_a);
    collected_a.wait_until("the volume-change question", |d| {
        d.notifications()
            .iter()
            .any(|n| matches!(n, Notification::VolumeChanged { watch, .. } if *watch == watch_a))
    });

    let (remaining, stopped) = watcher
        .answer_volume_change(watch_a, VolumeChangeDecision::Stop)
        .expect("this answer resolves the question");
    assert_eq!(remaining, 1, "only the declining route is removed");
    assert_eq!(
        stopped,
        vec![watch_a],
        "the declining route is reported stopped"
    );

    let deadline = std::time::Instant::now() + NOTIFY_TIMEOUT;
    while watcher.gate() == ArmGate::VolumeChangePending {
        assert!(
            std::time::Instant::now() < deadline,
            "the watcher never resumed arming after the volume change resolved"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(watcher.gate(), ArmGate::Open);

    // The remaining (AutoContinue) route is still served: a real change still
    // reaches it.
    let collected_b = Drained::start(receiver_b);
    std::fs::write(dir.path().join("after.txt"), b"x").expect("create a file");
    collected_b.wait_until("a change after the volume-change resolved", |d| {
        d.changes()
            .iter()
            .any(|(kind, name)| *kind == ChangeKind::Added && name == "after.txt")
    });

    drop(watcher);
    dir.cleanup();
}
