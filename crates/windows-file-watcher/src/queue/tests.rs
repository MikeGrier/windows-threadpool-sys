// Copyright (c) 2026 Mike Grier
//! Unit tests for the crate-owned notification queue.
//!
//! The queue is the boundary that keeps client behaviour off the cadence path,
//! so what is under test is the enqueue/receive contract itself: enqueueing never
//! blocks or fails, receiving terminates on teardown rather than hanging, and
//! ordering within a subscription is preserved.

use std::num::NonZeroUsize;
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle};
use std::sync::Arc;
use std::time::Duration;

use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
use windows_sys::Win32::System::Threading::WaitForSingleObject;
use windows_threadpool_sys::wait::{ThreadpoolWait, WaitableHandle};

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
        Notification::Desync { .. }
        | Notification::Completion { .. }
        | Notification::Suspended { .. }
        | Notification::Resumed { .. }
        | Notification::Established { .. }
        | Notification::RetryQuestion { .. } => Vec::new(),
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
    let (_sender, receiver) = channel();
    assert_eq!(receiver.capacity(), super::DEFAULT_BOUND.get());
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

// --- the standing slot (D-27/D-28) ---

#[test]
fn a_standing_slot_can_send_repeatedly_without_ever_failing() {
    let (sender, receiver) = bounded(2);
    let slot = sender.reserve_standing().expect("a slot");
    let watch = WatchId::from_raw(1);
    for _ in 0..5 {
        slot.send(Notification::RetryQuestion {
            watch,
            operation: crate::retry::FaultOperation::Open,
        });
        assert!(receiver.try_recv().is_some(), "each send is delivered");
    }
}

#[test]
fn a_standing_send_does_not_overflow_capacity_while_the_queue_is_otherwise_full() {
    // The exact scenario a double-counted reservation would corrupt: capacity 2,
    // one unit permanently carved out for the standing slot and one best-effort
    // item already queued, leaves no free capacity. The standing send must still
    // succeed without `free()` underflowing (it would have, before the queued
    // entry started standing in for the reservation instead of sitting on top of
    // it).
    let (sender, receiver) = bounded(2);
    let slot = sender.reserve_standing().expect("a slot");
    let watch = WatchId::from_raw(1);
    deliver(&sender, batch(watch, &["a.txt"]));
    assert_eq!(
        sender.send(batch(watch, &["b.txt"])),
        Delivery::Latched,
        "the queue has no free capacity left"
    );

    slot.send(Notification::RetryQuestion {
        watch,
        operation: crate::retry::FaultOperation::Open,
    });

    assert_eq!(receiver.len(), 2, "the standing send still landed");
}

#[test]
fn dropping_a_standing_slot_while_its_message_is_still_queued_releases_capacity_once() {
    // If the slot released its reservation on drop *and* the queued entry
    // released it again once drained, the queue's accounting would go negative
    // (or wrap, in a release build). Exactly one of the two must release it.
    let (sender, receiver) = bounded(1);
    let slot = sender.reserve_standing().expect("a slot");
    let watch = WatchId::from_raw(1);
    slot.send(Notification::RetryQuestion {
        watch,
        operation: crate::retry::FaultOperation::Open,
    });

    // Dropped while its last-sent message is still sitting in the queue,
    // undrained.
    drop(slot);

    assert!(
        receiver.try_recv().is_some(),
        "the queued message still arrives"
    );
    // The slot's own reservation was already spent on the message just drained,
    // so nothing further can ever be sent through it, and this capacity is
    // gone for good.
    assert!(
        sender.reserve().is_none(),
        "the standing slot's capacity is not returned to the pool"
    );
}

#[test]
fn dropping_an_unused_standing_slot_returns_its_capacity() {
    let (sender, _receiver) = bounded(1);
    let slot = sender.reserve_standing().expect("a slot");
    assert!(
        sender.reserve().is_none(),
        "the standing slot's carve-out is the only capacity"
    );
    drop(slot);
    assert!(
        sender.reserve().is_some(),
        "an unused standing slot returns its capacity on drop"
    );
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

    // The background thread only delivers "b.txt" and hands `sender` back
    // once that has happened; the overflow send that must observe a full
    // queue runs here, after delivery is confirmed and before anything can
    // drain it. This was previously racy: if the overflow send ran on the
    // background thread immediately after its own `deliver`, this thread's
    // `recv` for "b.txt" could occasionally win the race and drain it first
    // (an OS scheduling detail, not anything the crate promises), making the
    // overflow observe room instead of a full queue and fail to latch.
    let (delivered_tx, delivered_rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        deliver(&sender, batch(watch, &["b.txt"]));
        delivered_tx.send(sender).expect("signal delivery");
    });

    let sender = delivered_rx
        .recv()
        .expect("the background thread delivered");
    assert_eq!(sender.send(batch(watch, &["lost.txt"])), Delivery::Latched);

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

// --- the doorbell (D-25) ---

/// Whether a handle is currently signalled, without blocking.
fn is_signalled(handle: BorrowedHandle<'_>) -> bool {
    // SAFETY: a live event handle; a zero timeout polls rather than waits.
    unsafe { WaitForSingleObject(handle.as_raw_handle(), 0) == WAIT_OBJECT_0 }
}

/// Wait for a handle to become signalled, failing rather than hanging.
fn await_signal(handle: BorrowedHandle<'_>) -> bool {
    // SAFETY: as above, with a bounded timeout.
    unsafe { WaitForSingleObject(handle.as_raw_handle(), 30_000) == WAIT_OBJECT_0 }
}

#[test]
fn a_receiver_that_never_asks_allocates_no_doorbell() {
    // The laziness D-25 asks for: a `recv`-only client should not pay for a
    // kernel object it never waits on.
    let (sender, receiver) = channel();
    deliver(&sender, batch(WatchId::from_raw(1), &["a.txt"]));
    let _ = receiver.recv().expect("a notification");
    assert!(
        receiver.shared.doorbell.get().is_none(),
        "the event must not exist until it is asked for"
    );
}

#[test]
fn the_doorbell_is_created_to_match_the_queue_it_reports_on() {
    // Asked for after notifications have already arrived, it must come up
    // signalled -- otherwise the first wait would miss what is already there.
    let (sender, receiver) = channel();
    deliver(&sender, batch(WatchId::from_raw(1), &["a.txt"]));

    let doorbell = receiver.doorbell().expect("create the doorbell");
    assert!(is_signalled(doorbell));
}

#[test]
fn an_idle_queue_has_an_unsignalled_doorbell() {
    let (_sender, receiver) = channel();
    let doorbell = receiver.doorbell().expect("create the doorbell");
    assert!(!is_signalled(doorbell));
}

#[test]
fn sending_signals_the_doorbell_and_draining_resets_it() {
    let (sender, receiver) = channel();
    let doorbell = receiver.doorbell().expect("create the doorbell");
    assert!(!is_signalled(doorbell));

    deliver(&sender, batch(WatchId::from_raw(1), &["a.txt"]));
    assert!(is_signalled(doorbell));

    let _ = receiver.try_recv().expect("a notification");
    assert!(
        !is_signalled(doorbell),
        "an emptied queue must stop claiming there is something to take"
    );
}

#[test]
fn a_partial_drain_leaves_the_doorbell_signalled() {
    // Manual-reset, so a client that drains one item and stops must still be
    // told the rest is there.
    let (sender, receiver) = channel();
    let doorbell = receiver.doorbell().expect("create the doorbell");
    deliver(&sender, batch(WatchId::from_raw(1), &["a.txt"]));
    deliver(&sender, batch(WatchId::from_raw(1), &["b.txt"]));

    let _ = receiver.try_recv().expect("the first");
    assert!(is_signalled(doorbell));
    let _ = receiver.try_recv().expect("the second");
    assert!(!is_signalled(doorbell));
}

#[test]
fn an_owed_loss_signals_the_doorbell_even_with_an_empty_queue() {
    // A latched desync is not in the queue, but it is still something to take --
    // so the doorbell must report it, or a waiting client would never learn its
    // change stream has a hole in it.
    let (sender, receiver) = bounded(1);
    let watch = WatchId::from_raw(1);
    let doorbell = receiver.doorbell().expect("create the doorbell");

    deliver(&sender, batch(watch, &["a.txt"]));
    assert_eq!(sender.send(batch(watch, &["lost.txt"])), Delivery::Latched);
    let _ = receiver.try_recv().expect("the queued one");

    assert!(receiver.is_empty());
    assert!(
        is_signalled(doorbell),
        "the queue is empty but a loss is still owed"
    );
    let _ = receiver.try_recv().expect("the latched one");
    assert!(!is_signalled(doorbell));
}

#[test]
fn disconnection_signals_the_doorbell() {
    // Otherwise a client waiting on the handle would wait forever for a
    // notification nothing can send.
    let (sender, receiver) = channel();
    let doorbell = receiver.doorbell().expect("create the doorbell");
    assert!(!is_signalled(doorbell));

    drop(sender);
    assert!(is_signalled(doorbell));
    assert!(receiver.recv().is_none());
    assert!(
        is_signalled(doorbell),
        "the end of the stream is permanent, so it stays signalled"
    );
}

#[test]
fn the_doorbell_is_created_once_and_reused() {
    let (_sender, receiver) = channel();
    let first = receiver.doorbell().expect("create").as_raw_handle();
    let second = receiver.doorbell().expect("reuse").as_raw_handle();
    assert_eq!(first, second, "the doorbell is created once, not per call");
}

#[test]
fn an_owned_doorbell_refers_to_the_same_event() {
    let (sender, receiver) = channel();
    let owned = receiver.doorbell_owned().expect("duplicate the doorbell");
    assert_ne!(
        owned.as_raw_handle(),
        receiver.doorbell().expect("borrow").as_raw_handle(),
        "a duplicate is a distinct handle"
    );

    deliver(&sender, batch(WatchId::from_raw(1), &["a.txt"]));
    assert!(
        is_signalled(owned.as_handle()),
        "but it refers to the same event, so signalling reaches both"
    );

    // Closing the caller's copy leaves the queue's own intact.
    drop(owned);
    assert!(is_signalled(receiver.doorbell().expect("still there")));
}

#[test]
fn a_waiting_client_is_woken_by_a_send() {
    let (sender, receiver) = channel();
    let doorbell = receiver.doorbell_owned().expect("the doorbell");

    let handle = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        deliver(&sender, batch(WatchId::from_raw(1), &["late.txt"]));
    });

    assert!(
        await_signal(doorbell.as_handle()),
        "the doorbell must ring for a notification that arrives while waiting"
    );
    assert_eq!(names(&receiver.try_recv().expect("it")), vec!["late.txt"]);
    handle.join().expect("sender thread");
}

#[test]
fn no_wakeup_is_lost_under_a_concurrent_burst() {
    // The property the invariant exists for. The consumer waits, drains to
    // empty, and waits again -- exactly the loop a `ThreadpoolWait` client runs
    // -- while a producer sends throughout. A single missed edge wedges it, and
    // the bounded wait turns that into a failure rather than a hang.
    const TOTAL: usize = 2_000;
    let (sender, receiver) = bounded(16);
    let doorbell = receiver.doorbell_owned().expect("the doorbell");

    let producer = std::thread::spawn(move || {
        for index in 0..TOTAL {
            // Best-effort, so a full queue latches; either way the consumer must
            // be woken for it.
            let _ = sender.send(batch(WatchId::from_raw(1), &[&format!("f-{index}")]));
        }
        drop(sender);
    });

    let mut seen = 0_usize;
    loop {
        assert!(
            await_signal(doorbell.as_handle()),
            "the doorbell stopped ringing after {seen} notifications"
        );
        while receiver.try_recv().is_some() {
            seen += 1;
        }
        if receiver.is_disconnected() && receiver.is_empty() && receiver.latched() == 0 {
            break;
        }
    }

    producer.join().expect("producer");
    assert!(seen > 0, "nothing was delivered at all");
}

#[test]
fn a_client_can_drain_from_its_own_threadpool_wait() {
    // The integration the doorbell exists to enable (D-25): no dedicated thread,
    // no crate-supplied callback -- the client arms its own pool object on a
    // handle we hand out, and drains on its own cadence.
    let (sender, receiver) = channel();
    let doorbell = receiver.doorbell_owned().expect("the doorbell");

    let receiver = Arc::new(receiver);
    let drained = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    // An atomic rather than an mpsc sender: the callback must be `Fn + Send +
    // Sync`, and `mpsc::Sender` is not `Sync`.
    let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let consumer = Arc::clone(&receiver);
    let sink = Arc::clone(&drained);
    let signal = Arc::clone(&finished);

    // SAFETY: an event is a supported wait target, and this is our own duplicate
    // of the doorbell, transferred exclusively into the wait object.
    let waitable = unsafe { WaitableHandle::assume_waitable(doorbell) };
    let wait = ThreadpoolWait::new(
        waitable,
        move |activation| {
            while let Some(item) = consumer.try_recv() {
                let mut names = names(&item);
                if !names.is_empty() {
                    sink.lock().expect("record").push(names.remove(0));
                }
            }
            if consumer.is_disconnected() {
                signal.store(true, std::sync::atomic::Ordering::SeqCst);
            } else {
                // Manual-reset plus drain-to-empty means re-arming cannot miss an
                // edge: anything sent since the drain has already re-signalled.
                activation.rearm(None);
            }
        },
        None,
    )
    .expect("create the wait");
    wait.arm(None);

    for index in 0..64 {
        deliver(
            &sender,
            batch(WatchId::from_raw(1), &[&format!("f-{index}")]),
        );
    }
    drop(sender);

    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !finished.load(std::sync::atomic::Ordering::SeqCst) {
        assert!(
            std::time::Instant::now() < deadline,
            "the client's own pool callback never drained the queue"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    wait.stop_and_drain();

    let seen = drained.lock().expect("read").clone();
    assert_eq!(seen.len(), 64, "every notification reached the client");
    let expected: Vec<String> = (0..64).map(|index| format!("f-{index}")).collect();
    assert_eq!(seen, expected);
}
