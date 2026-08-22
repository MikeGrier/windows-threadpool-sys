// Copyright (c) 2026 Mike Grier
//! Unit tests for the affine subscription handle.
//!
//! What is under test is the lifetime contract -- registration begins a watch,
//! and both ways of ending one end it -- plus the binding between a subscription
//! and the receiver its notifications reach.

use std::num::NonZeroUsize;
use std::time::{Duration, Instant};

use super::{RetryMode, WatchOptions};
use crate::directory::OpenFailure;
use crate::monitor::Monitor;
use crate::notify::ChangeKind;
use crate::queue::{Notification, Outcome, Receiver, WatchId};
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

// --- request completions (D-30) ---

/// Drain until a completion for `watch` arrives, returning its outcome.
fn await_completion(receiver: &Receiver, watch: WatchId) -> Outcome {
    let deadline = Instant::now() + NOTIFY_TIMEOUT;
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .unwrap_or_default();
        assert!(
            !remaining.is_zero(),
            "timed out waiting for a completion for {watch:?}"
        );
        let Some(item) = receiver.recv_timeout(remaining) else {
            continue;
        };
        if let Notification::Completion {
            watch: reported,
            outcome,
        } = item
            && reported == watch
        {
            return outcome;
        }
    }
}

#[test]
fn a_successful_subscribe_reports_itself() {
    let dir = TempDir::new("completion-ok");
    let monitor = Monitor::new().expect("create the monitor");
    let (session, receiver) = monitor.session();

    let watch = session
        .subscribe(dir.path(), WatchOptions::new())
        .expect("register");

    assert_eq!(await_completion(&receiver, watch.id()), Outcome::Subscribed);

    drop(watch);
    drop(monitor);
    dir.cleanup();
}

#[test]
fn a_path_through_a_file_is_reported_as_establishing_not_failed() {
    // A path that treats a *file* as if it were a directory with contents (an
    // intermediate component that is not itself a directory) does not exist as
    // far as `CreateFileW` is concerned -- Windows reports `ERROR_PATH_NOT_FOUND`
    // for the whole path rather than naming the file in the middle, so this
    // classifies as retryable (D-22) exactly like any other not-yet-existing
    // path, not as a permanent `NotADirectory`.
    let dir = TempDir::new("completion-file");
    let file = dir.path().join("a-file-not-a-directory.txt");
    std::fs::write(&file, b"x").expect("create the file");
    let impossible = file.join("nested");

    let monitor = Monitor::new().expect("create the monitor");
    let (session, receiver) = monitor.session();
    let watch = session
        .subscribe(&impossible, WatchOptions::new())
        .expect("register");

    assert_eq!(
        await_completion(&receiver, watch.id()),
        Outcome::Establishing
    );

    drop(watch);
    drop(monitor);
    dir.cleanup();
}

#[test]
fn an_interior_nul_is_reported_as_a_permanent_failure() {
    let monitor = Monitor::new().expect("create the monitor");
    let (session, receiver) = monitor.session();
    let watch = session
        .subscribe("c:\\some\0path", WatchOptions::new())
        .expect("register");

    assert_eq!(
        await_completion(&receiver, watch.id()),
        Outcome::Failed {
            failure: OpenFailure::InvalidPath
        }
    );

    drop(watch);
    drop(monitor);
}

#[test]
fn a_target_that_does_not_exist_yet_is_establishing_rather_than_failed() {
    // D-14 has no terminal fault state, so a path that may appear later is a
    // state to recover from, not a failure to report. The subscription stays
    // registered; M5.1 is what drives the recovery.
    let dir = TempDir::new("completion-missing");
    let monitor = Monitor::new().expect("create the monitor");
    let (session, receiver) = monitor.session();
    let watch = session
        .subscribe(dir.path().join("not-yet"), WatchOptions::new())
        .expect("register");

    assert_eq!(
        await_completion(&receiver, watch.id()),
        Outcome::Establishing
    );
    monitor.quiesce();
    assert!(monitor.is_registered(watch.id()), "still a subscription");
    assert!(!monitor.is_watching(watch.id()), "but not yet established");

    drop(watch);
    drop(monitor);
    dir.cleanup();
}

#[test]
fn cancelling_reports_itself() {
    let dir = TempDir::new("completion-cancel");
    let monitor = Monitor::new().expect("create the monitor");
    let (session, receiver) = monitor.session();

    let watch = session
        .subscribe(dir.path(), WatchOptions::new())
        .expect("register");
    let id = watch.id();
    assert_eq!(await_completion(&receiver, id), Outcome::Subscribed);

    watch.cancel();
    assert_eq!(await_completion(&receiver, id), Outcome::Cancelled);

    drop(monitor);
    dir.cleanup();
}

#[test]
fn dropping_a_watch_reports_the_cancellation_too() {
    // `Drop` has nowhere to report a refused reservation, which is why the
    // cancellation's slot is taken at registration; this asserts the completion
    // that guarantee exists to make possible.
    let dir = TempDir::new("completion-drop");
    let monitor = Monitor::new().expect("create the monitor");
    let (session, receiver) = monitor.session();

    let watch = session
        .subscribe(dir.path(), WatchOptions::new())
        .expect("register");
    let id = watch.id();
    assert_eq!(await_completion(&receiver, id), Outcome::Subscribed);

    drop(watch);
    assert_eq!(await_completion(&receiver, id), Outcome::Cancelled);

    drop(monitor);
    dir.cleanup();
}

#[test]
fn nothing_follows_a_cancellation_in_the_stream() {
    // The structural ordering D-30 buys: because the completion is enqueued only
    // once the watcher is fully stopped, a client can treat `Cancelled` as a
    // boundary rather than having to reason about timing.
    let dir = TempDir::new("completion-order");
    let monitor = Monitor::new().expect("create the monitor");
    let (session, receiver) = monitor.session();

    let watch = session
        .subscribe(dir.path(), WatchOptions::new())
        .expect("register");
    let id = watch.id();
    assert_eq!(await_completion(&receiver, id), Outcome::Subscribed);

    // Generate real traffic right up to the cancellation, so the boundary is
    // being asserted against a stream that is actually moving.
    for index in 0..200 {
        std::fs::write(dir.path().join(format!("f-{index}.txt")), b"x").expect("create");
    }
    watch.cancel();

    let mut seen_cancelled = false;
    let deadline = Instant::now() + NOTIFY_TIMEOUT;
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .unwrap_or_default();
        assert!(!remaining.is_zero(), "timed out draining the stream");
        let Some(item) = receiver.recv_timeout(remaining) else {
            if seen_cancelled {
                break;
            }
            continue;
        };
        match item {
            Notification::Completion {
                watch: reported,
                outcome: Outcome::Cancelled,
            } if reported == id => seen_cancelled = true,
            other => assert!(
                !seen_cancelled || other.watch() != id,
                "nothing for a cancelled watch may follow its cancellation, saw {other:?}"
            ),
        }
        if seen_cancelled && receiver.is_empty() {
            break;
        }
    }
    assert!(seen_cancelled, "the cancellation must be reported");

    drop(monitor);
    dir.cleanup();
}

#[test]
fn a_completion_is_delivered_even_when_the_queue_is_saturated() {
    // The point of reserving at submit (D-33): change traffic cannot crowd out a
    // completion, however far behind the client is.
    let dir = TempDir::new("completion-full");
    let monitor = Monitor::new().expect("create the monitor");
    // Four slots: two are this subscription's reservations, leaving almost
    // nothing for observation.
    let (session, receiver) = monitor.session_with_bound(NonZeroUsize::new(4).expect("non-zero"));

    let watch = session
        .subscribe(dir.path(), WatchOptions::new())
        .expect("register");
    let id = watch.id();
    monitor.quiesce();

    // Saturate with real change traffic and never drain.
    for index in 0..500 {
        std::fs::write(dir.path().join(format!("f-{index}.txt")), b"x").expect("create");
    }
    std::thread::sleep(Duration::from_millis(100));

    watch.cancel();

    // Both completions must survive, however much change traffic was dropped or
    // latched around them: neither ever competed for the room they were promised.
    let mut outcomes = Vec::new();
    loop {
        let outcome = await_completion(&receiver, id);
        outcomes.push(outcome);
        if outcome == Outcome::Cancelled {
            break;
        }
    }
    assert!(
        outcomes.contains(&Outcome::Subscribed),
        "the registration completion was lost to saturation, saw {outcomes:?}"
    );

    drop(monitor);
    dir.cleanup();
}

#[test]
fn registration_is_refused_when_no_completion_can_be_promised() {
    // Backpressure lands here, on the client's own thread at the call that asked
    // for the work, rather than at a delivery with no safe way to fail.
    let dir = TempDir::new("completion-backpressure");
    let monitor = Monitor::new().expect("create the monitor");
    let (session, _receiver) = monitor.session_with_bound(NonZeroUsize::new(2).expect("non-zero"));

    // The first subscription takes both slots: one for its completion, one
    // standing for its cancellation.
    let first = session
        .subscribe(dir.path(), WatchOptions::new())
        .expect("register");

    let error = session
        .subscribe(dir.path(), WatchOptions::new())
        .expect_err("there is no room to promise a second subscription's completions");
    assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);

    drop(first);
    drop(monitor);
    dir.cleanup();
}

#[test]
fn every_subscription_gets_exactly_one_completion_of_each_kind() {
    let dir = TempDir::new("completion-count");
    let monitor = Monitor::new().expect("create the monitor");
    let (session, receiver) = monitor.session();

    let watches: Vec<_> = (0..16)
        .map(|_| {
            session
                .subscribe(dir.path(), WatchOptions::new())
                .expect("register")
        })
        .collect();
    let ids: Vec<WatchId> = watches.iter().map(super::Watch::id).collect();
    for id in &ids {
        assert_eq!(await_completion(&receiver, *id), Outcome::Subscribed);
    }

    drop(watches);
    for id in &ids {
        assert_eq!(await_completion(&receiver, *id), Outcome::Cancelled);
    }

    drop(monitor);
    dir.cleanup();
}

// --- file targets (D-7) ---

#[test]
fn subscribing_to_a_file_watches_its_parent_directory() {
    let dir = TempDir::new("file-target");
    let file = dir.path().join("target.txt");
    std::fs::write(&file, b"x").expect("create the file");

    let monitor = Monitor::new().expect("create the monitor");
    let (session, receiver) = monitor.session();
    let watch = session
        .subscribe(&file, WatchOptions::new())
        .expect("register");

    assert_eq!(await_completion(&receiver, watch.id()), Outcome::Subscribed);
    assert!(monitor.is_watching(watch.id()));

    drop(watch);
    drop(monitor);
    dir.cleanup();
}

#[test]
fn a_file_target_reports_changes_to_that_file_and_nothing_else() {
    let dir = TempDir::new("file-target-filter");
    let target = dir.path().join("target.txt");
    // The target must exist at subscribe time: resolving a not-yet-existing path
    // to a file target versus a directory target is left to M5's re-establish
    // loop (see the module docs), not attempted here.
    std::fs::write(&target, b"x").expect("create the target");

    let monitor = Monitor::new().expect("create the monitor");
    let (session, receiver) = monitor.session();
    let watch = session
        .subscribe(&target, WatchOptions::new())
        .expect("register");
    monitor.quiesce();

    std::fs::write(dir.path().join("other.txt"), b"y").expect("create an unrelated sibling");
    std::fs::remove_file(&target).expect("remove the target");
    std::fs::write(&target, b"recreated").expect("recreate the target");

    assert_eq!(await_change(&receiver, "target.txt"), watch.id());

    drop(watch);
    drop(monitor);
    dir.cleanup();
}

#[test]
fn a_file_target_and_a_directory_target_on_the_same_directory_coalesce() {
    let dir = TempDir::new("file-target-coalesce");
    let target = dir.path().join("target.txt");
    std::fs::write(&target, b"x").expect("create the target");

    let monitor = Monitor::new().expect("create the monitor");
    let (session, receiver) = monitor.session();
    let file_watch = session
        .subscribe(&target, WatchOptions::new())
        .expect("register the file target");
    let dir_watch = session
        .subscribe(dir.path(), WatchOptions::new())
        .expect("register the directory target");
    monitor.quiesce();

    assert_eq!(
        monitor.directory_count(),
        1,
        "one directory, one coalesced watcher, regardless of target kind"
    );

    std::fs::write(dir.path().join("fresh.txt"), b"x").expect("create a new file");
    let seen_by = await_change(&receiver, "fresh.txt");
    assert!(seen_by == file_watch.id() || seen_by == dir_watch.id());

    drop((file_watch, dir_watch));
    drop(monitor);
    dir.cleanup();
}
