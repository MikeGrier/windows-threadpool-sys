// Copyright (c) 2026 Mike Grier
//! Integration tests for the queue-mediated watcher, driven entirely through the
//! crate's public surface.
//!
//! The unit tests establish that each piece works. What is here is what only
//! shows up when a client uses them together: several subscriptions sharing one
//! receiver, both ways of ending a watch, a client that stops draining, and the
//! volumes at which the arm / complete / re-arm loop actually earns its keep.
#![cfg(windows)]

use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use windows_file_watcher::{
    ChangeKind, DesyncCause, Monitor, Notification, Outcome, Receiver, Session, Watch, WatchId,
    WatchOptions,
};

/// Upper bound for waiting on something the kernel really should deliver. Long
/// enough that a loaded CI runner does not fail spuriously; short enough that a
/// genuine wedge fails the run rather than stalling it.
const NOTIFY_TIMEOUT: Duration = Duration::from_secs(30);

/// What teardown is allowed to take. Cancellation retires an outstanding read at
/// once, so this only fires if teardown waited for a change instead.
const TEARDOWN_BUDGET: Duration = Duration::from_secs(5);

/// How many files the scale test creates. The point is a volume no single
/// completion can carry, so the assertion spans many completions and the re-arms
/// between them.
const SCALE_FILES: usize = 1_000;

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
            "windows-file-watcher-it-{label}-{}-{nonce}",
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

/// Everything a client has drained so far, in arrival order.
#[derive(Default)]
struct Drained {
    items: Vec<Notification>,
}

impl Drained {
    /// Take everything currently available without blocking.
    fn pump(&mut self, receiver: &Receiver) {
        while let Some(item) = receiver.try_recv() {
            self.items.push(item);
        }
    }

    /// Drain until `predicate` holds, failing rather than hanging.
    fn drain_until<F>(&mut self, receiver: &Receiver, what: &str, predicate: F)
    where
        F: Fn(&Drained) -> bool,
    {
        let deadline = Instant::now() + NOTIFY_TIMEOUT;
        loop {
            self.pump(receiver);
            if predicate(self) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}; saw {} notification(s)",
                self.items.len()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// The names of every change reported for `watch`, in order.
    fn names(&self, watch: WatchId) -> Vec<String> {
        self.items
            .iter()
            .filter_map(|item| match item {
                Notification::Batch {
                    watch: tag,
                    changes,
                } if *tag == watch => Some(changes),
                _ => None,
            })
            .flatten()
            .map(|change| change.name.to_os_string().to_string_lossy().into_owned())
            .collect()
    }

    /// The `(kind, name)` pairs reported for `watch`, in order.
    fn changes(&self, watch: WatchId) -> Vec<(ChangeKind, String)> {
        self.items
            .iter()
            .filter_map(|item| match item {
                Notification::Batch {
                    watch: tag,
                    changes,
                } if *tag == watch => Some(changes),
                _ => None,
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

    fn outcomes(&self, watch: WatchId) -> Vec<Outcome> {
        self.items
            .iter()
            .filter_map(|item| match item {
                Notification::Completion {
                    watch: tag,
                    outcome,
                } if *tag == watch => Some(*outcome),
                _ => None,
            })
            .collect()
    }

    fn desyncs(&self, watch: WatchId) -> Vec<DesyncCause> {
        self.items
            .iter()
            .filter_map(|item| match item {
                Notification::Desync { watch: tag, cause } if *tag == watch => Some(*cause),
                _ => None,
            })
            .collect()
    }

    /// The index of the completion carrying `outcome` for `watch`, if any.
    fn position_of(&self, watch: WatchId, outcome: Outcome) -> Option<usize> {
        self.items.iter().position(|item| {
            matches!(item, Notification::Completion { watch: tag, outcome: seen }
                if *tag == watch && *seen == outcome)
        })
    }

    /// The index of the last notification of any kind for `watch`.
    fn last_mention_of(&self, watch: WatchId) -> Option<usize> {
        self.items.iter().rposition(|item| item.watch() == watch)
    }

    fn has(&self, watch: WatchId, kind: ChangeKind, name: &str) -> bool {
        self.changes(watch)
            .iter()
            .any(|(seen_kind, seen_name)| *seen_kind == kind && seen_name == name)
    }
}

/// A monitor with one session, its receiver, and a drain buffer.
fn client() -> (Monitor, Session, Receiver, Drained) {
    let monitor = Monitor::new().expect("create the monitor");
    let (session, receiver) = monitor.session();
    (monitor, session, receiver, Drained::default())
}

/// Subscribe and wait for the registration to be reported.
fn subscribe(
    session: &Session,
    receiver: &Receiver,
    drained: &mut Drained,
    dir: &Path,
    options: WatchOptions,
) -> Watch {
    let watch = session.subscribe(dir, options).expect("register");
    let id = watch.id();
    drained.drain_until(receiver, "the registration", |seen| {
        !seen.outcomes(id).is_empty()
    });
    assert_eq!(
        drained.outcomes(id),
        vec![Outcome::Subscribed],
        "the subscription should have been established"
    );
    watch
}

#[test]
fn create_modify_and_delete_are_reported_with_relative_names_in_order() {
    let dir = TempDir::new("crud");
    let (monitor, session, receiver, mut drained) = client();
    let watch = subscribe(
        &session,
        &receiver,
        &mut drained,
        dir.path(),
        WatchOptions::new(),
    );
    let id = watch.id();

    let file = dir.path().join("alpha.txt");
    std::fs::write(&file, b"one").expect("create");
    drained.drain_until(&receiver, "the create", |seen| {
        seen.has(id, ChangeKind::Added, "alpha.txt")
    });

    std::fs::write(&file, b"two, rather longer than one").expect("modify");
    drained.drain_until(&receiver, "the modify", |seen| {
        seen.has(id, ChangeKind::Modified, "alpha.txt")
    });

    std::fs::remove_file(&file).expect("delete");
    drained.drain_until(&receiver, "the delete", |seen| {
        seen.has(id, ChangeKind::Removed, "alpha.txt")
    });

    // Order is a promise, not an accident: the queue preserves the sequence the
    // kernel reported, so a client replaying it reaches the same end state.
    let changes = drained.changes(id);
    let added = changes
        .iter()
        .position(|(kind, name)| *kind == ChangeKind::Added && name == "alpha.txt")
        .expect("the create");
    let removed = changes
        .iter()
        .position(|(kind, name)| *kind == ChangeKind::Removed && name == "alpha.txt")
        .expect("the delete");
    assert!(added < removed, "saw {changes:?}");

    // The name is relative to the watched directory, never the full path.
    assert!(
        changes.iter().all(|(_, name)| !name.contains('\\')),
        "a non-recursive watch reports bare names, saw {changes:?}"
    );

    drop(watch);
    drop(monitor);
    dir.cleanup();
}

#[test]
fn a_rename_is_reported_as_an_old_name_and_a_new_name() {
    let dir = TempDir::new("rename");
    let before = dir.path().join("beta.txt");
    std::fs::write(&before, b"beta").expect("seed");

    let (monitor, session, receiver, mut drained) = client();
    let watch = subscribe(
        &session,
        &receiver,
        &mut drained,
        dir.path(),
        WatchOptions::new(),
    );
    let id = watch.id();

    std::fs::rename(&before, dir.path().join("gamma.txt")).expect("rename");
    drained.drain_until(&receiver, "both halves of the rename", |seen| {
        seen.has(id, ChangeKind::RenamedOldName, "beta.txt")
            && seen.has(id, ChangeKind::RenamedNewName, "gamma.txt")
    });

    // The two halves are one event split across two records; a client pairing
    // them relies on the old name arriving first.
    let changes = drained.changes(id);
    let old = changes
        .iter()
        .position(|(kind, name)| *kind == ChangeKind::RenamedOldName && name == "beta.txt")
        .expect("old name");
    let new = changes
        .iter()
        .position(|(kind, name)| *kind == ChangeKind::RenamedNewName && name == "gamma.txt")
        .expect("new name");
    assert!(old < new, "saw {changes:?}");

    drop(watch);
    drop(monitor);
    dir.cleanup();
}

#[test]
fn a_subtree_subscription_reports_names_relative_to_the_watched_root() {
    let dir = TempDir::new("subtree");
    let (monitor, session, receiver, mut drained) = client();
    let watch = subscribe(
        &session,
        &receiver,
        &mut drained,
        dir.path(),
        WatchOptions::new().subtree(true),
    );
    let id = watch.id();

    let nested = dir.path().join("one").join("two");
    std::fs::create_dir_all(&nested).expect("create the nested directories");
    std::fs::write(nested.join("deep.txt"), b"deep").expect("create the nested file");

    drained.drain_until(&receiver, "the nested create", |seen| {
        seen.has(id, ChangeKind::Added, "one\\two\\deep.txt")
    });

    drop(watch);
    drop(monitor);
    dir.cleanup();
}

#[test]
fn a_shallow_subscription_ignores_changes_below_the_watched_directory() {
    let dir = TempDir::new("shallow");
    let (monitor, session, receiver, mut drained) = client();
    let watch = subscribe(
        &session,
        &receiver,
        &mut drained,
        dir.path(),
        WatchOptions::new(),
    );
    let id = watch.id();

    let nested = dir.path().join("sub");
    std::fs::create_dir(&nested).expect("create the subdirectory");
    std::fs::write(nested.join("hidden.txt"), b"hidden").expect("create below it");

    // Absence only means something once the loop has demonstrably moved past the
    // nested write. Changes are reported in order, so waiting for one made
    // *after* it establishes that.
    std::fs::write(dir.path().join("marker.txt"), b"marker").expect("create the marker");
    drained.drain_until(&receiver, "a change made after the nested one", |seen| {
        seen.has(id, ChangeKind::Added, "marker.txt")
    });

    assert!(drained.has(id, ChangeKind::Added, "sub"));
    assert!(
        !drained
            .names(id)
            .iter()
            .any(|name| name.contains("hidden.txt")),
        "a shallow watch must not report below itself, saw {:?}",
        drained.names(id)
    );

    drop(watch);
    drop(monitor);
    dir.cleanup();
}

#[test]
fn several_subscriptions_share_one_receiver_and_stay_distinguishable() {
    let first = TempDir::new("share-first");
    let second = TempDir::new("share-second");
    let third = TempDir::new("share-third");
    let (monitor, session, receiver, mut drained) = client();

    let a = subscribe(
        &session,
        &receiver,
        &mut drained,
        first.path(),
        WatchOptions::new(),
    );
    let b = subscribe(
        &session,
        &receiver,
        &mut drained,
        second.path(),
        WatchOptions::new(),
    );
    let c = subscribe(
        &session,
        &receiver,
        &mut drained,
        third.path(),
        WatchOptions::new(),
    );

    // Interleaved, so a per-subscription order that survives is genuinely being
    // demultiplexed rather than happening to arrive in blocks.
    for index in 0..50 {
        std::fs::write(first.path().join(format!("a-{index}.txt")), b"a").expect("create");
        std::fs::write(second.path().join(format!("b-{index}.txt")), b"b").expect("create");
        std::fs::write(third.path().join(format!("c-{index}.txt")), b"c").expect("create");
    }

    let last = |prefix: char| format!("{prefix}-49.txt");
    drained.drain_until(&receiver, "every subscription's last change", |seen| {
        seen.has(a.id(), ChangeKind::Added, &last('a'))
            && seen.has(b.id(), ChangeKind::Added, &last('b'))
            && seen.has(c.id(), ChangeKind::Added, &last('c'))
    });

    // In-order within a subscription, and nothing from one leaking into another.
    for (watch, prefix) in [(&a, 'a'), (&b, 'b'), (&c, 'c')] {
        let creates: Vec<String> = drained
            .changes(watch.id())
            .into_iter()
            .filter(|(kind, _)| *kind == ChangeKind::Added)
            .map(|(_, name)| name)
            .collect();
        let expected: Vec<String> = (0..50)
            .map(|index| format!("{prefix}-{index}.txt"))
            .collect();
        assert_eq!(creates, expected, "subscription {prefix}");
    }

    drop((a, b, c));
    drop(monitor);
    first.cleanup();
    second.cleanup();
    third.cleanup();
}

#[test]
fn cancelling_ends_the_stream_for_that_subscription_and_no_other() {
    let watched = TempDir::new("cancel-watched");
    let other = TempDir::new("cancel-other");
    let (monitor, session, receiver, mut drained) = client();

    let ending = subscribe(
        &session,
        &receiver,
        &mut drained,
        watched.path(),
        WatchOptions::new(),
    );
    let surviving = subscribe(
        &session,
        &receiver,
        &mut drained,
        other.path(),
        WatchOptions::new(),
    );
    let ending_id = ending.id();

    for index in 0..100 {
        std::fs::write(watched.path().join(format!("f-{index}.txt")), b"x").expect("create");
    }
    ending.cancel();
    drained.drain_until(&receiver, "the cancellation", |seen| {
        seen.position_of(ending_id, Outcome::Cancelled).is_some()
    });

    // Keep changing the now-unwatched directory; nothing may follow.
    for index in 0..100 {
        std::fs::write(watched.path().join(format!("after-{index}.txt")), b"x").expect("create");
    }
    std::fs::write(other.path().join("survivor.txt"), b"x").expect("create");
    drained.drain_until(&receiver, "the surviving subscription", |seen| {
        seen.has(surviving.id(), ChangeKind::Added, "survivor.txt")
    });

    let cancelled_at = drained
        .position_of(ending_id, Outcome::Cancelled)
        .expect("the cancellation");
    assert_eq!(
        drained.last_mention_of(ending_id),
        Some(cancelled_at),
        "nothing for a cancelled subscription may follow its cancellation"
    );

    drop(surviving);
    drop(monitor);
    watched.cleanup();
    other.cleanup();
}

#[test]
fn dropping_a_watch_ends_it_the_same_way() {
    let dir = TempDir::new("drop-cancel");
    let (monitor, session, receiver, mut drained) = client();
    let watch = subscribe(
        &session,
        &receiver,
        &mut drained,
        dir.path(),
        WatchOptions::new(),
    );
    let id = watch.id();

    std::fs::write(dir.path().join("before.txt"), b"x").expect("create");
    drained.drain_until(&receiver, "a change before the drop", |seen| {
        seen.has(id, ChangeKind::Added, "before.txt")
    });

    drop(watch);
    drained.drain_until(&receiver, "the cancellation", |seen| {
        seen.position_of(id, Outcome::Cancelled).is_some()
    });

    for index in 0..50 {
        std::fs::write(dir.path().join(format!("after-{index}.txt")), b"x").expect("create");
    }
    std::thread::sleep(Duration::from_millis(100));
    drained.pump(&receiver);

    let cancelled_at = drained
        .position_of(id, Outcome::Cancelled)
        .expect("the cancellation");
    assert_eq!(drained.last_mention_of(id), Some(cancelled_at));

    drop(monitor);
    dir.cleanup();
}

#[test]
fn a_file_target_is_subscribed_rather_than_reported_unwatchable() {
    // D-7: a file is watched via its parent, not rejected as "not a directory".
    let dir = TempDir::new("permanent");
    let file = dir.path().join("a-file-not-a-directory.txt");
    std::fs::write(&file, b"x").expect("create the file");

    let (monitor, session, receiver, mut drained) = client();
    let watch = session
        .subscribe(&file, WatchOptions::new())
        .expect("register");
    let id = watch.id();

    drained.drain_until(&receiver, "the registration outcome", |seen| {
        !seen.outcomes(id).is_empty()
    });
    assert_eq!(
        drained.outcomes(id),
        vec![Outcome::Subscribed],
        "a client must never be left holding a watch that can never fire"
    );

    drop(watch);
    drop(monitor);
    dir.cleanup();
}

#[test]
fn a_thousand_creates_are_either_all_reported_or_explicitly_desynced() {
    let dir = TempDir::new("scale");
    let (monitor, session, receiver, mut drained) = client();
    let watch = subscribe(
        &session,
        &receiver,
        &mut drained,
        dir.path(),
        WatchOptions::new(),
    );
    let id = watch.id();

    for index in 0..SCALE_FILES {
        std::fs::write(dir.path().join(format!("file-{index:04}.txt")), b"scale")
            .expect("create at scale");
    }

    let last = format!("file-{:04}.txt", SCALE_FILES - 1);
    drained.drain_until(&receiver, "the whole burst to settle", |seen| {
        seen.has(id, ChangeKind::Added, &last) || !seen.desyncs(id).is_empty()
    });

    // The crate's central promise (D-12): a change is either delivered or its
    // loss is reported. "Neither" is the failure this asserts against.
    if drained.desyncs(id).is_empty() {
        let changes = drained.changes(id);
        let missing: Vec<String> = (0..SCALE_FILES)
            .map(|index| format!("file-{index:04}.txt"))
            .filter(|name| {
                !changes
                    .iter()
                    .any(|(kind, seen)| *kind == ChangeKind::Added && seen == name)
            })
            .collect();
        assert!(
            missing.is_empty(),
            "{} of {SCALE_FILES} creates were lost with no desync reported, first few: {:?}",
            missing.len(),
            &missing[..missing.len().min(5)]
        );
    }

    drop(watch);
    drop(monitor);
    dir.cleanup();
}

#[test]
fn a_client_that_stops_draining_is_paused_rather_than_dropped_and_then_resumes() {
    // The whole backpressure story end to end: saturate, observe that the loss is
    // reported rather than silent, then drain and observe that both the watching
    // and the request path pick up again.
    let dir = TempDir::new("saturate");
    let monitor = Monitor::new().expect("create the monitor");
    let (session, receiver) = monitor.session_with_bound(NonZeroUsize::new(8).expect("non-zero"));
    let mut drained = Drained::default();

    let watch = session
        .subscribe(dir.path(), WatchOptions::new())
        .expect("register");
    let id = watch.id();
    monitor.quiesce();

    // Never drain while this runs, so the queue genuinely fills.
    for index in 0..500 {
        std::fs::write(dir.path().join(format!("f-{index}.txt")), b"x").expect("create");
    }
    std::thread::sleep(Duration::from_millis(200));

    // Now drain. Whatever was lost must have been reported, and the watch must
    // still be live rather than having stopped.
    drained.drain_until(&receiver, "the queue to drain", |seen| {
        seen.items.len() >= 8
    });
    drained.pump(&receiver);

    // Request draining was never blocked by the full ring (D-33): a cancellation
    // submitted now is serviced and reported.
    std::fs::write(dir.path().join("after-drain.txt"), b"x").expect("create");
    drained.drain_until(&receiver, "watching to resume after the drain", |seen| {
        seen.has(id, ChangeKind::Added, "after-drain.txt") || !seen.desyncs(id).is_empty()
    });

    watch.cancel();
    drained.drain_until(&receiver, "the cancellation", |seen| {
        seen.position_of(id, Outcome::Cancelled).is_some()
    });

    drop(monitor);
    dir.cleanup();
}

#[test]
fn teardown_with_subscriptions_outstanding_converges_promptly() {
    let dir = TempDir::new("teardown");
    let (monitor, session, receiver, mut drained) = client();

    let watches: Vec<Watch> = (0..8)
        .map(|_| {
            subscribe(
                &session,
                &receiver,
                &mut drained,
                dir.path(),
                WatchOptions::new(),
            )
        })
        .collect();

    // Nothing will change this directory again, so only cancellation can retire
    // the outstanding reads. A teardown that waited would sit here until the
    // budget expired.
    let started = Instant::now();
    drop(monitor);
    let elapsed = started.elapsed();
    assert!(
        elapsed < TEARDOWN_BUDGET,
        "teardown took {elapsed:?}, which means it waited rather than cancelled"
    );

    // Releasing every handle ends the stream, so a client's `recv` loop finishes
    // rather than blocking on a queue nothing can fill again.
    drop(watches);
    drop(session);
    let started = Instant::now();
    while receiver.recv().is_some() {
        assert!(
            started.elapsed() < TEARDOWN_BUDGET,
            "the receiver never observed the disconnect"
        );
    }

    dir.cleanup();
}

#[test]
fn a_client_can_wait_on_the_doorbell_instead_of_dedicating_a_thread() {
    let dir = TempDir::new("doorbell");
    let (monitor, session, receiver, mut drained) = client();
    let watch = subscribe(
        &session,
        &receiver,
        &mut drained,
        dir.path(),
        WatchOptions::new(),
    );
    let id = watch.id();

    let doorbell = receiver.doorbell().expect("the doorbell");
    std::fs::write(dir.path().join("ring.txt"), b"x").expect("create");

    // A real wait on a real handle, which is what a client would compose into its
    // own `WaitForMultipleObjects` or thread pool.
    let woken = wait_for_handle(&doorbell, NOTIFY_TIMEOUT);
    assert!(woken, "the doorbell did not ring for a change");

    drained.drain_until(&receiver, "the change", |seen| {
        seen.has(id, ChangeKind::Added, "ring.txt")
    });

    drop(watch);
    drop(monitor);
    dir.cleanup();
}

/// Wait for a handle to be signalled, without pulling in a windows-sys
/// dependency for the test target.
fn wait_for_handle(handle: &std::os::windows::io::BorrowedHandle<'_>, timeout: Duration) -> bool {
    use std::os::windows::io::AsRawHandle;

    unsafe extern "system" {
        fn WaitForSingleObject(handle: *mut core::ffi::c_void, milliseconds: u32) -> u32;
    }

    let millis = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
    // SAFETY: a live handle borrowed from the receiver, and a bounded timeout.
    unsafe { WaitForSingleObject(handle.as_raw_handle(), millis) == 0 }
}

// --- M4: coalescing by directory and file targets ---

#[test]
fn file_watches_and_a_recursive_directory_watch_in_one_tree_see_exactly_their_own_events() {
    // Several file-watches plus a recursive directory watch within one tree:
    // each subscription must receive exactly its matching events and nothing
    // else (M4.5).
    let dir = TempDir::new("m4-tree");
    let alpha = dir.path().join("alpha.txt");
    let beta = dir.path().join("beta.txt");
    std::fs::write(&alpha, b"a").expect("create alpha");
    std::fs::write(&beta, b"b").expect("create beta");

    let (monitor, session, receiver, mut drained) = client();

    // Two file targets, sharing the directory with a recursive directory watch.
    let alpha_watch = subscribe_ok(
        &session,
        &receiver,
        &mut drained,
        &alpha,
        WatchOptions::new(),
    );
    let beta_watch = subscribe_ok(
        &session,
        &receiver,
        &mut drained,
        &beta,
        WatchOptions::new(),
    );
    let tree_watch = subscribe_ok(
        &session,
        &receiver,
        &mut drained,
        dir.path(),
        WatchOptions::new().subtree(true),
    );

    let nested = dir.path().join("nested");
    std::fs::create_dir(&nested).expect("create the nested directory");
    let gamma = nested.join("gamma.txt");
    std::fs::write(&gamma, b"c").expect("create gamma");

    std::fs::write(&alpha, b"modified alpha").expect("modify alpha");
    std::fs::write(&beta, b"modified beta").expect("modify beta");

    drained.drain_until(&receiver, "every subscription's own events", |seen| {
        seen.has(alpha_watch.id(), ChangeKind::Modified, "alpha.txt")
            && seen.has(beta_watch.id(), ChangeKind::Modified, "beta.txt")
            && seen.has(tree_watch.id(), ChangeKind::Added, "nested\\gamma.txt")
    });

    // Exactly its own: the file watches must never see the other's events, or
    // the tree's, and the tree watch's Added/Modified for alpha/beta must be
    // tagged with its own id, never the file watches'.
    let alpha_changes = drained.changes(alpha_watch.id());
    assert!(
        alpha_changes.iter().all(|(_, name)| name == "alpha.txt"),
        "the alpha file-watch saw something other than alpha.txt: {alpha_changes:?}"
    );
    let beta_changes = drained.changes(beta_watch.id());
    assert!(
        beta_changes.iter().all(|(_, name)| name == "beta.txt"),
        "the beta file-watch saw something other than beta.txt: {beta_changes:?}"
    );

    let tree_changes = drained.changes(tree_watch.id());
    assert!(
        tree_changes
            .iter()
            .any(|(kind, name)| *kind == ChangeKind::Modified && name == "alpha.txt"),
        "the recursive tree watch must also see top-level changes, saw {tree_changes:?}"
    );
    assert!(
        tree_changes
            .iter()
            .any(|(kind, name)| *kind == ChangeKind::Modified && name == "beta.txt")
    );
    assert!(
        tree_changes
            .iter()
            .any(|(kind, name)| *kind == ChangeKind::Added && name == "nested\\gamma.txt")
    );

    drop((alpha_watch, beta_watch, tree_watch));
    drop(monitor);
    dir.cleanup();
}

#[test]
fn cancelling_one_coalesced_subscription_does_not_disturb_its_siblings() {
    let dir = TempDir::new("m4-cancel-shared");
    let (monitor, session, receiver, mut drained) = client();

    let a = subscribe_ok(
        &session,
        &receiver,
        &mut drained,
        dir.path(),
        WatchOptions::new(),
    );
    let b = subscribe_ok(
        &session,
        &receiver,
        &mut drained,
        dir.path(),
        WatchOptions::new(),
    );

    let a_id = a.id();
    a.cancel();
    drained.drain_until(&receiver, "a's cancellation", |seen| {
        seen.position_of(a_id, Outcome::Cancelled).is_some()
    });

    // b's watcher must have survived a's teardown: the directory coalescing
    // (D-6) means one still-live subscriber keeps the watcher alive.
    std::fs::write(dir.path().join("after.txt"), b"x").expect("create");
    drained.drain_until(&receiver, "b's change after a's cancellation", |seen| {
        seen.has(b.id(), ChangeKind::Added, "after.txt")
    });

    drop(b);
    drop(monitor);
    dir.cleanup();
}

/// Subscribe and wait for the registration outcome to arrive, asserting it
/// succeeded.
fn subscribe_ok(
    session: &Session,
    receiver: &Receiver,
    drained: &mut Drained,
    path: &Path,
    options: WatchOptions,
) -> Watch {
    let watch = session.subscribe(path, options).expect("register");
    let id = watch.id();
    drained.drain_until(receiver, "the registration", |seen| {
        !seen.outcomes(id).is_empty()
    });
    assert_eq!(drained.outcomes(id), vec![Outcome::Subscribed]);
    watch
}
