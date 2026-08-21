// Copyright (c) 2026 Mike Grier
//! Unit tests for the crate-owned notification queue.
//!
//! The queue is the boundary that keeps client behaviour off the cadence path,
//! so what is under test is the enqueue/receive contract itself: enqueueing never
//! blocks or fails, receiving terminates on teardown rather than hanging, and
//! ordering within a subscription is preserved.

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Duration;

use super::{Delivery, Notification, WatchId, channel};
use crate::notify::{Change, ChangeKind, DesyncCause, RelativeName};

/// Send, asserting the queue had room.
///
/// Every test outside the saturation section sends into a roomy queue and would
/// be meaningless if the notification had been latched instead, so the outcome is
/// asserted rather than discarded.
fn deliver(sender: &super::Sender, notification: Notification) {
    assert_eq!(
        sender.send(notification),
        Delivery::Queued,
        "the queue was expected to have room"
    );
}

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
    deliver(&sender, batch(WatchId::from_raw(1), &["a.txt"]));
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
        deliver(&sender, batch(watch, &[&format!("file-{index}.txt")]));
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
    deliver(&sender, batch(watch, &["before.txt"]));
    deliver(
        &sender,
        Notification::Desync {
            watch,
            cause: DesyncCause::Overflow,
        },
    );
    deliver(&sender, batch(watch, &["after.txt"]));

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
    deliver(&sender, batch(WatchId::from_raw(10), &["a.txt"]));
    deliver(
        &sender,
        Notification::Desync {
            watch: WatchId::from_raw(20),
            cause: DesyncCause::Coarse,
        },
    );
    assert_eq!(receiver.recv().expect("a").watch(), WatchId::from_raw(10));
    assert_eq!(receiver.recv().expect("b").watch(), WatchId::from_raw(20));
}

#[test]
fn several_subscriptions_can_share_one_queue() {
    // A session aggregates subscriptions onto one receiver, so the tag is what
    // lets a client demultiplex.
    let (sender, receiver) = channel();
    for id in 0..8_u64 {
        deliver(&sender, batch(WatchId::from_raw(id), &["shared.txt"]));
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
    deliver(&sender, batch(WatchId::from_raw(1), &["a.txt"]));
    deliver(&sender, batch(WatchId::from_raw(1), &["b.txt"]));
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
                    deliver(
                        &sender,
                        batch(WatchId::from_raw(id), &[&format!("f-{index}")]),
                    );
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
            deliver(
                &sender,
                batch(WatchId::from_raw(1), &[&format!("a-{index}")]),
            );
        }
    });
    let b = std::thread::spawn(move || {
        for index in 0..EACH {
            deliver(
                &second,
                batch(WatchId::from_raw(2), &[&format!("b-{index}")]),
            );
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
        deliver(&sender, batch(WatchId::from_raw(1), &["late.txt"]));
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
    deliver(&sender, batch(WatchId::from_raw(1), &["a.txt"]));
    deliver(&sender, batch(WatchId::from_raw(1), &["b.txt"]));
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
    deliver(&sender, batch(WatchId::from_raw(1), &["eventual.txt"]));
    let received = receiver
        .recv_timeout(Duration::from_millis(500))
        .expect("the notification");
    assert_eq!(names(&received), vec!["eventual.txt"]);
}

#[test]
fn recv_timeout_returns_immediately_when_something_is_queued() {
    let (sender, receiver) = channel();
    deliver(&sender, batch(WatchId::from_raw(1), &["ready.txt"]));
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
    deliver(&sender, batch(WatchId::from_raw(1), &["orphan.txt"]));
    deliver(&sender, batch(WatchId::from_raw(1), &["orphan2.txt"]));
}

#[test]
fn senders_can_be_cloned_across_threads() {
    let (sender, receiver) = channel();
    let shared = Arc::new(sender);
    let handles: Vec<_> = (0..4)
        .map(|id| {
            let sender = Arc::clone(&shared);
            std::thread::spawn(move || deliver(&sender, batch(WatchId::from_raw(id), &["x"])))
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

// --- the bound, reservations, and the latch (D-33) ---

/// A queue holding exactly `bound` notifications.
fn bounded(bound: usize) -> (super::Sender, super::Receiver) {
    super::channel_with_bound(NonZeroUsize::new(bound).expect("a non-zero bound"))
}

/// Fill a queue to its bound, asserting each send is queued.
fn fill(sender: &super::Sender, watch: WatchId, count: usize) {
    for index in 0..count {
        deliver(sender, batch(watch, &[&format!("fill-{index}")]));
    }
}

#[test]
fn the_default_bound_is_reported() {
    let (sender, receiver) = channel();
    assert_eq!(receiver.capacity(), super::DEFAULT_BOUND.get());
    assert_eq!(sender.capacity(), super::DEFAULT_BOUND.get());
}

#[test]
fn a_chosen_bound_is_honoured() {
    let (sender, receiver) = bounded(4);
    assert_eq!(receiver.capacity(), 4);

    fill(&sender, WatchId::from_raw(1), 4);
    assert_eq!(receiver.len(), 4);

    // The bound is a bound: the fifth does not simply grow the queue.
    assert_eq!(
        sender.send(batch(WatchId::from_raw(1), &["overflow.txt"])),
        Delivery::Latched
    );
    assert_eq!(receiver.len(), 4, "the queue must not exceed its bound");
}

#[test]
fn a_dropped_notification_is_reported_as_a_desync_not_lost_silently() {
    // The crate's central promise (D-12): a change is either delivered or its
    // loss is reported. Never neither.
    let (sender, receiver) = bounded(2);
    let watch = WatchId::from_raw(9);
    fill(&sender, watch, 2);
    assert_eq!(
        sender.send(batch(watch, &["dropped.txt"])),
        Delivery::Latched
    );

    assert_eq!(names(&receiver.recv().expect("first")), vec!["fill-0"]);
    assert_eq!(names(&receiver.recv().expect("second")), vec!["fill-1"]);
    let reported = receiver.recv().expect("the loss report");
    assert!(matches!(
        reported,
        Notification::Desync {
            cause: DesyncCause::QueueFull,
            ..
        }
    ));
    assert_eq!(reported.watch(), watch);
}

#[test]
fn a_latched_loss_is_reported_after_everything_that_preceded_it() {
    // Ordering is the reason the latch is drained at the *next enqueue* rather
    // than surfaced immediately: the queue was full when the loss happened, so
    // everything still queued precedes it.
    let (sender, receiver) = bounded(3);
    let watch = WatchId::from_raw(1);
    deliver(&sender, batch(watch, &["first.txt"]));
    deliver(&sender, batch(watch, &["second.txt"]));
    deliver(&sender, batch(watch, &["third.txt"]));
    assert_eq!(sender.send(batch(watch, &["lost.txt"])), Delivery::Latched);

    // Two slots, so the flushed desync and the new notification both fit and
    // their relative order is what is under test.
    assert_eq!(names(&receiver.recv().expect("first")), vec!["first.txt"]);
    assert_eq!(names(&receiver.recv().expect("second")), vec!["second.txt"]);
    deliver(&sender, batch(watch, &["fourth.txt"]));

    assert_eq!(names(&receiver.recv().expect("third")), vec!["third.txt"]);
    assert!(
        matches!(
            receiver.recv().expect("the desync"),
            Notification::Desync {
                cause: DesyncCause::QueueFull,
                ..
            }
        ),
        "the loss belongs after the changes that preceded it and before the one that followed"
    );
    assert_eq!(names(&receiver.recv().expect("fourth")), vec!["fourth.txt"]);
}

#[test]
fn a_latched_loss_reaches_a_receiver_even_if_nothing_further_is_sent() {
    // The latch is drained at the next enqueue, but there may never be one --
    // so draining to empty must surface it too, or the guarantee would depend on
    // future activity.
    let (sender, receiver) = bounded(1);
    let watch = WatchId::from_raw(3);
    deliver(&sender, batch(watch, &["only.txt"]));
    assert_eq!(sender.send(batch(watch, &["lost.txt"])), Delivery::Latched);

    assert_eq!(
        names(&receiver.recv().expect("the queued one")),
        vec!["only.txt"]
    );
    assert!(matches!(
        receiver.recv().expect("the latched one"),
        Notification::Desync {
            cause: DesyncCause::QueueFull,
            ..
        }
    ));
    assert!(receiver.try_recv().is_none());
}

#[test]
fn a_latched_loss_is_delivered_even_after_every_sender_is_gone() {
    // Otherwise teardown could swallow the one signal that says changes were
    // lost, which is exactly the silent loss the design forbids.
    let (sender, receiver) = bounded(1);
    let watch = WatchId::from_raw(4);
    deliver(&sender, batch(watch, &["only.txt"]));
    assert_eq!(sender.send(batch(watch, &["lost.txt"])), Delivery::Latched);
    drop(sender);

    assert_eq!(
        names(&receiver.recv().expect("the queued one")),
        vec!["only.txt"]
    );
    assert!(matches!(
        receiver.recv().expect("the latched one"),
        Notification::Desync {
            cause: DesyncCause::QueueFull,
            ..
        }
    ));
    assert!(receiver.recv().is_none(), "and only then is it finished");
}

#[test]
fn repeated_losses_for_one_subscription_coalesce() {
    // Coalescing loses nothing: a desync is idempotent, and the client's answer
    // to one is its answer to a thousand.
    let (sender, receiver) = bounded(1);
    let watch = WatchId::from_raw(5);
    deliver(&sender, batch(watch, &["only.txt"]));
    for index in 0..100 {
        assert_eq!(
            sender.send(batch(watch, &[&format!("lost-{index}")])),
            Delivery::Latched
        );
    }
    assert_eq!(
        receiver.latched(),
        1,
        "one subscription, one pending desync"
    );
}

#[test]
fn losses_are_latched_per_subscription() {
    let (sender, receiver) = bounded(1);
    deliver(&sender, batch(WatchId::from_raw(0), &["only.txt"]));
    for watch in 1..5_u64 {
        assert_eq!(
            sender.send(batch(WatchId::from_raw(watch), &["lost.txt"])),
            Delivery::Latched
        );
    }
    assert_eq!(receiver.latched(), 4, "each subscription is owed its own");

    let _ = receiver.recv().expect("the queued one");
    let mut reported: Vec<u64> = Vec::new();
    while let Some(item) = receiver.try_recv() {
        reported.push(item.watch().get());
    }
    reported.sort_unstable();
    assert_eq!(reported, vec![1, 2, 3, 4]);
}

#[test]
fn a_reserved_slot_cannot_be_taken_by_the_best_effort_path() {
    // The whole mechanism in one test: reserved capacity is held away from
    // observation, so a control message cannot be crowded out by change traffic.
    let (sender, receiver) = bounded(2);
    let reservation = sender.reserve().expect("a slot");

    // One slot is reserved, so only one remains for best-effort traffic.
    deliver(&sender, batch(WatchId::from_raw(1), &["a.txt"]));
    assert_eq!(
        sender.send(batch(WatchId::from_raw(1), &["b.txt"])),
        Delivery::Latched,
        "observation must not consume the reserved slot"
    );

    reservation.send(Notification::Desync {
        watch: WatchId::from_raw(1),
        cause: DesyncCause::Reestablished,
    });
    assert_eq!(receiver.len(), 2);
}

#[test]
fn a_reservation_is_refused_when_there_is_no_room() {
    // Backpressure lands here, on the caller's own thread at reservation time,
    // rather than at a delivery that has no way to fail safely (D-33).
    let (sender, _receiver) = bounded(2);
    let first = sender.reserve().expect("a slot");
    let second = sender.reserve().expect("a slot");
    assert!(
        sender.reserve().is_none(),
        "the bound applies to reservations"
    );
    drop(first);
    assert!(sender.reserve().is_some(), "releasing one frees it");
    drop(second);
}

#[test]
fn a_freed_slot_reports_the_owed_loss_before_it_carries_new_changes() {
    // The priority rule, and the reason a released reservation does not simply
    // hand its slot to the next batch: while a loss is owed, the first room to
    // appear is spent telling the client about it. Carrying more changes across
    // an unreported hole would be the silent loss the design forbids.
    let (sender, receiver) = bounded(1);
    let watch = WatchId::from_raw(1);
    let reservation = sender.reserve().expect("a slot");
    assert_eq!(
        sender.send(batch(watch, &["blocked.txt"])),
        Delivery::Latched
    );

    drop(reservation);

    // The slot is free again, but it is owed to the loss report, so this batch
    // is latched too -- and having been reported, the latch reopens for it.
    assert_eq!(sender.send(batch(watch, &["next.txt"])), Delivery::Latched);
    assert!(matches!(
        receiver.recv().expect("the flushed desync"),
        Notification::Desync {
            cause: DesyncCause::QueueFull,
            ..
        }
    ));
    // A report covers the losses since the previous one, so `next.txt` gets its
    // own -- synthesised here, because the queue drained before anything else
    // was sent.
    assert!(matches!(
        receiver.recv().expect("the second desync"),
        Notification::Desync {
            cause: DesyncCause::QueueFull,
            ..
        }
    ));
    assert_eq!(receiver.latched(), 0, "nothing further is owed");

    // With nothing owed, the released slot carries traffic normally again.
    deliver(&sender, batch(watch, &["now.txt"]));
    assert_eq!(names(&receiver.recv().expect("now")), vec!["now.txt"]);
}

#[test]
fn a_reserved_send_never_fails_however_full_the_queue_is() {
    // The guarantee D-33 exists for: whatever else happens between reserving and
    // sending, the room is already this reservation's.
    let (sender, receiver) = bounded(4);
    let reservation = sender.reserve().expect("a slot");
    let watch = WatchId::from_raw(1);

    // Saturate everything else, several times over.
    for index in 0..100 {
        let _ = sender.send(batch(watch, &[&format!("noise-{index}")]));
    }
    assert_eq!(receiver.len(), 3, "the reserved slot stayed empty");

    reservation.send(Notification::Desync {
        watch,
        cause: DesyncCause::Reestablished,
    });
    assert_eq!(receiver.len(), 4);

    let mut causes = Vec::new();
    while let Some(item) = receiver.try_recv() {
        if let Notification::Desync { cause, .. } = item {
            causes.push(cause);
        }
    }
    assert!(
        causes.contains(&DesyncCause::Reestablished),
        "the reserved notification must be delivered, saw {causes:?}"
    );
}

#[test]
fn a_reservation_keeps_the_queue_connected() {
    // A reservation can still deliver, so a receiver must not conclude the stream
    // has finished while one is outstanding.
    let (sender, receiver) = bounded(2);
    let reservation = sender.reserve().expect("a slot");
    drop(sender);

    assert!(
        !receiver.is_disconnected(),
        "an outstanding reservation is a pending delivery"
    );
    reservation.send(Notification::Desync {
        watch: WatchId::from_raw(1),
        cause: DesyncCause::Reestablished,
    });
    assert!(receiver.recv().is_some());
    assert!(receiver.is_disconnected());
}

#[test]
fn reservations_can_be_taken_from_several_threads() {
    let (sender, receiver) = bounded(64);
    let handles: Vec<_> = (0..8)
        .map(|id| {
            let sender = sender.clone();
            std::thread::spawn(move || {
                for _ in 0..8 {
                    let reservation = sender.reserve().expect("a slot");
                    reservation.send(Notification::Desync {
                        watch: WatchId::from_raw(id),
                        cause: DesyncCause::Reestablished,
                    });
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().expect("thread");
    }
    assert_eq!(receiver.len(), 64, "every reserved send was delivered");
}

#[test]
fn a_bound_of_one_still_reports_its_own_saturation() {
    // The smallest legal queue is the sharpest test of the guarantee: with a
    // single slot, the desync announcing a loss cannot share the queue with the
    // notification that preceded it, and must still arrive.
    let (sender, receiver) = bounded(1);
    let watch = WatchId::from_raw(1);
    deliver(&sender, batch(watch, &["a.txt"]));
    assert_eq!(sender.send(batch(watch, &["b.txt"])), Delivery::Latched);
    assert_eq!(sender.send(batch(watch, &["c.txt"])), Delivery::Latched);

    assert_eq!(names(&receiver.recv().expect("a")), vec!["a.txt"]);
    assert!(matches!(
        receiver.recv().expect("the desync"),
        Notification::Desync {
            cause: DesyncCause::QueueFull,
            ..
        }
    ));
    assert!(receiver.try_recv().is_none());
}

#[test]
fn a_blocked_receiver_is_woken_by_a_latched_loss() {
    // A latch is an observable event, so a client parked in `recv` must learn of
    // it rather than sleeping through a reported hole in its change stream.
    let (sender, receiver) = bounded(1);
    let watch = WatchId::from_raw(1);
    deliver(&sender, batch(watch, &["a.txt"]));
    assert_eq!(names(&receiver.recv().expect("a")), vec!["a.txt"]);

    let handle = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        deliver(&sender, batch(watch, &["b.txt"]));
        assert_eq!(sender.send(batch(watch, &["lost.txt"])), Delivery::Latched);
    });

    assert_eq!(names(&receiver.recv().expect("b")), vec!["b.txt"]);
    assert!(matches!(
        receiver.recv().expect("the desync"),
        Notification::Desync {
            cause: DesyncCause::QueueFull,
            ..
        }
    ));
    handle.join().expect("sender thread");
}
