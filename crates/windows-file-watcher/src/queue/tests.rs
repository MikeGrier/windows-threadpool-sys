// Copyright (c) 2026 Mike Grier
//! Unit tests for the crate-owned notification queue.
//!
//! The queue is the boundary that keeps client behaviour off the cadence path,
//! so what is under test is the enqueue/receive contract itself: enqueueing never
//! blocks or fails, receiving terminates on teardown rather than hanging, and
//! ordering within a subscription is preserved.

use std::sync::Arc;
use std::time::Duration;

use super::{Notification, WatchId, channel};
use crate::notify::{Change, ChangeKind, DesyncCause, RelativeName};

/// A change with a given name, for building recognisable batches.
fn change(name: &str) -> Change {
    Change {
        kind: ChangeKind::Added,
        name: RelativeName::from_units(name.encode_utf16().collect::<Vec<u16>>()),
    }
}

fn batch(watch: WatchId, names: &[&str]) -> Notification {
    Notification::Batch {
        watch,
        changes: names.iter().map(|name| change(name)).collect(),
    }
}

fn names(notification: &Notification) -> Vec<String> {
    match notification {
        Notification::Batch { changes, .. } => changes
            .iter()
            .map(|c| c.name.to_os_string().to_string_lossy().into_owned())
            .collect(),
        Notification::Desync { .. } => Vec::new(),
    }
}

// --- basic delivery ---

#[test]
fn an_empty_queue_yields_nothing() {
    let (_sender, receiver) = channel();
    assert!(receiver.try_recv().is_none());
    assert!(receiver.is_empty());
    assert_eq!(receiver.len(), 0);
    assert!(!receiver.is_disconnected());
}

#[test]
fn a_sent_notification_is_received() {
    let (sender, receiver) = channel();
    sender.send(batch(WatchId::from_raw(1), &["a.txt"]));
    let received = receiver.try_recv().expect("a notification");
    assert_eq!(names(&received), vec!["a.txt"]);
    assert_eq!(received.watch(), WatchId::from_raw(1));
}

#[test]
fn notifications_are_received_in_send_order() {
    // Ordering within a subscription is part of the contract (D-12): a client
    // that sees a Desync must know exactly which changes preceded it.
    let (sender, receiver) = channel();
    let watch = WatchId::from_raw(7);
    for index in 0..16 {
        sender.send(batch(watch, &[&format!("file-{index}.txt")]));
    }
    for index in 0..16 {
        let received = receiver.recv().expect("a notification");
        assert_eq!(names(&received), vec![format!("file-{index}.txt")]);
    }
}

#[test]
fn changes_and_desyncs_share_one_ordered_stream() {
    // They ride the same queue precisely so their relative order is defined.
    let (sender, receiver) = channel();
    let watch = WatchId::from_raw(2);
    sender.send(batch(watch, &["before.txt"]));
    sender.send(Notification::Desync {
        watch,
        cause: DesyncCause::Overflow,
    });
    sender.send(batch(watch, &["after.txt"]));

    assert_eq!(names(&receiver.recv().expect("first")), vec!["before.txt"]);
    assert!(matches!(
        receiver.recv().expect("second"),
        Notification::Desync {
            cause: DesyncCause::Overflow,
            ..
        }
    ));
    assert_eq!(names(&receiver.recv().expect("third")), vec!["after.txt"]);
}

#[test]
fn every_notification_carries_its_subscription() {
    let (sender, receiver) = channel();
    sender.send(batch(WatchId::from_raw(10), &["a.txt"]));
    sender.send(Notification::Desync {
        watch: WatchId::from_raw(20),
        cause: DesyncCause::Coarse,
    });
    assert_eq!(receiver.recv().expect("a").watch(), WatchId::from_raw(10));
    assert_eq!(receiver.recv().expect("b").watch(), WatchId::from_raw(20));
}

#[test]
fn several_subscriptions_can_share_one_queue() {
    // A session aggregates subscriptions onto one receiver, so the tag is what
    // lets a client demultiplex.
    let (sender, receiver) = channel();
    for id in 0..8_u64 {
        sender.send(batch(WatchId::from_raw(id), &["shared.txt"]));
    }
    let mut seen: Vec<u64> = Vec::new();
    while let Some(item) = receiver.try_recv() {
        seen.push(item.watch().get());
    }
    assert_eq!(seen, (0..8).collect::<Vec<u64>>());
}

#[test]
fn the_queue_length_tracks_what_is_pending() {
    let (sender, receiver) = channel();
    assert_eq!(receiver.len(), 0);
    sender.send(batch(WatchId::from_raw(1), &["a.txt"]));
    sender.send(batch(WatchId::from_raw(1), &["b.txt"]));
    assert_eq!(receiver.len(), 2);
    assert!(!receiver.is_empty());
    let _ = receiver.try_recv();
    assert_eq!(receiver.len(), 1);
    let _ = receiver.try_recv();
    assert!(receiver.is_empty());
}

// --- multi-producer ---

#[test]
fn many_senders_can_enqueue_concurrently() {
    // Completions for different watchers run on different pool threads, so the
    // sender must be usable from several at once (D-11).
    let (sender, receiver) = channel();
    const SENDERS: u64 = 8;
    const EACH: usize = 64;

    let handles: Vec<_> = (0..SENDERS)
        .map(|id| {
            let sender = sender.clone();
            std::thread::spawn(move || {
                for index in 0..EACH {
                    sender.send(batch(WatchId::from_raw(id), &[&format!("f-{index}")]));
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().expect("sender thread");
    }
    drop(sender);

    let mut per_watch = std::collections::HashMap::<u64, usize>::new();
    while let Some(item) = receiver.recv() {
        *per_watch.entry(item.watch().get()).or_default() += 1;
    }
    assert_eq!(per_watch.len(), SENDERS as usize);
    for id in 0..SENDERS {
        assert_eq!(per_watch[&id], EACH, "watch {id} lost notifications");
    }
}

#[test]
fn order_is_preserved_per_sender_under_concurrency() {
    // No cross-subscription ordering is promised, but a single producer's
    // sequence must not be reordered.
    let (sender, receiver) = channel();
    const EACH: usize = 128;
    let second = sender.clone();
    let a = std::thread::spawn(move || {
        for index in 0..EACH {
            sender.send(batch(WatchId::from_raw(1), &[&format!("a-{index}")]));
        }
    });
    let b = std::thread::spawn(move || {
        for index in 0..EACH {
            second.send(batch(WatchId::from_raw(2), &[&format!("b-{index}")]));
        }
    });
    a.join().expect("a");
    b.join().expect("b");

    let mut seen_a = Vec::new();
    let mut seen_b = Vec::new();
    while let Some(item) = receiver.recv() {
        let name = names(&item).remove(0);
        if item.watch() == WatchId::from_raw(1) {
            seen_a.push(name);
        } else {
            seen_b.push(name);
        }
    }
    let expected_a: Vec<String> = (0..EACH).map(|i| format!("a-{i}")).collect();
    let expected_b: Vec<String> = (0..EACH).map(|i| format!("b-{i}")).collect();
    assert_eq!(seen_a, expected_a);
    assert_eq!(seen_b, expected_b);
}

// --- blocking and teardown ---

#[test]
fn recv_blocks_until_something_arrives() {
    let (sender, receiver) = channel();
    let handle = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        sender.send(batch(WatchId::from_raw(1), &["late.txt"]));
    });
    let received = receiver.recv().expect("the late notification");
    assert_eq!(names(&received), vec!["late.txt"]);
    handle.join().expect("sender thread");
}

#[test]
fn recv_returns_none_once_every_sender_is_gone() {
    // A client loop must terminate on teardown rather than hang on a queue that
    // can never be filled again.
    let (sender, receiver) = channel();
    drop(sender);
    assert!(receiver.recv().is_none());
    assert!(receiver.is_disconnected());
}

#[test]
fn a_blocked_receiver_is_woken_by_the_last_sender_dropping() {
    let (sender, receiver) = channel();
    let handle = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        drop(sender);
    });
    assert!(
        receiver.recv().is_none(),
        "dropping the last sender must wake a blocked receiver"
    );
    handle.join().expect("dropper thread");
}

#[test]
fn queued_items_are_drained_before_disconnection_is_reported() {
    // Teardown must not discard what was already enqueued.
    let (sender, receiver) = channel();
    sender.send(batch(WatchId::from_raw(1), &["a.txt"]));
    sender.send(batch(WatchId::from_raw(1), &["b.txt"]));
    drop(sender);

    assert_eq!(names(&receiver.recv().expect("a")), vec!["a.txt"]);
    assert_eq!(names(&receiver.recv().expect("b")), vec!["b.txt"]);
    assert!(receiver.recv().is_none());
}

#[test]
fn disconnection_needs_every_sender_gone_not_just_one() {
    let (sender, receiver) = channel();
    let clone = sender.clone();
    drop(sender);
    assert!(
        !receiver.is_disconnected(),
        "one surviving sender keeps the queue open"
    );
    drop(clone);
    assert!(receiver.is_disconnected());
}

#[test]
fn recv_timeout_gives_up_and_can_be_retried() {
    let (sender, receiver) = channel();
    let start = std::time::Instant::now();
    assert!(receiver.recv_timeout(Duration::from_millis(50)).is_none());
    assert!(
        start.elapsed() >= Duration::from_millis(40),
        "recv_timeout returned early"
    );
    sender.send(batch(WatchId::from_raw(1), &["eventual.txt"]));
    let received = receiver
        .recv_timeout(Duration::from_millis(500))
        .expect("the notification");
    assert_eq!(names(&received), vec!["eventual.txt"]);
}

#[test]
fn recv_timeout_returns_immediately_when_something_is_queued() {
    let (sender, receiver) = channel();
    sender.send(batch(WatchId::from_raw(1), &["ready.txt"]));
    let start = std::time::Instant::now();
    assert!(receiver.recv_timeout(Duration::from_secs(30)).is_some());
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "a queued item must not wait for the timeout"
    );
}

#[test]
fn a_sender_survives_the_receiver_being_dropped() {
    // The crate's cadence path must never fault because a client went away, so
    // sending into an abandoned queue is a no-op rather than an error or panic.
    let (sender, receiver) = channel();
    drop(receiver);
    sender.send(batch(WatchId::from_raw(1), &["orphan.txt"]));
    sender.send(batch(WatchId::from_raw(1), &["orphan2.txt"]));
}

#[test]
fn senders_can_be_cloned_across_threads() {
    let (sender, receiver) = channel();
    let shared = Arc::new(sender);
    let handles: Vec<_> = (0..4)
        .map(|id| {
            let sender = Arc::clone(&shared);
            std::thread::spawn(move || sender.send(batch(WatchId::from_raw(id), &["x"])))
        })
        .collect();
    for handle in handles {
        handle.join().expect("thread");
    }
    assert_eq!(receiver.len(), 4);
}

// --- the identifier ---

#[test]
fn a_watch_id_round_trips_its_raw_value() {
    for raw in [0_u64, 1, 42, u64::MAX] {
        assert_eq!(WatchId::from_raw(raw).get(), raw);
    }
}

#[test]
fn watch_ids_compare_and_hash_by_value() {
    use std::collections::HashSet;
    let mut set = HashSet::new();
    assert!(set.insert(WatchId::from_raw(1)));
    assert!(!set.insert(WatchId::from_raw(1)), "equal ids are one entry");
    assert!(set.insert(WatchId::from_raw(2)));
    assert_ne!(WatchId::from_raw(1), WatchId::from_raw(2));
}
