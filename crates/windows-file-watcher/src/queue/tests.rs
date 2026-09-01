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

use super::{Delivery, Notification, Receiver, WatchId, channel};
use crate::directory::{FailureCode, FaultDetail, OpenFailure};
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

/// An arbitrary detail for tests that only need `RetryQuestion` to carry one,
/// not to assert on its contents.
fn test_detail() -> FaultDetail {
    FaultDetail {
        failure: OpenFailure::Retryable,
        code: FailureCode::Win32(0),
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
        | Notification::RetryQuestion { .. }
        | Notification::VolumeChanged { .. } => Vec::new(),
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
        let received = next(&receiver, "a notification");
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

    assert_eq!(names(&next(&receiver, "first")), vec!["before.txt"]);
    assert!(matches!(
        next(&receiver, "second"),
        Notification::Desync {
            cause: DesyncCause::Overflow,
            ..
        }
    ));
    assert_eq!(names(&next(&receiver, "third")), vec!["after.txt"]);
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
    assert_eq!(next(&receiver, "a").watch(), WatchId::from_raw(10));
    assert_eq!(next(&receiver, "b").watch(), WatchId::from_raw(20));
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

    // Bounded, for the reason `assert_stream_ended` exists: an unbounded drain
    // loop ends only when the last sender's drop wakes the receiver, so a
    // broken wake turns this into an infinite loop rather than a failure. It
    // was the last such loop in this file, and the one that kept the
    // `Drop for Sender` mutants costing a full mutation deadline apiece.
    let mut per_watch = std::collections::HashMap::<u64, usize>::new();
    while let Some(item) = receiver.recv_timeout(Duration::from_secs(5)) {
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
    while let Some(item) = receiver.recv_timeout(Duration::from_secs(5)) {
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
    let received = next(&receiver, "the late notification");
    assert_eq!(names(&received), vec!["late.txt"]);
    handle.join().expect("sender thread");
}

#[test]
fn recv_returns_none_once_every_sender_is_gone() {
    // A client loop must terminate on teardown rather than hang on a queue that
    // can never be filled again.
    let (sender, receiver) = channel();
    drop(sender);
    assert_stream_ended(&receiver, "every sender is gone");
    assert!(receiver.is_disconnected());
}

#[test]
fn a_blocked_receiver_is_woken_by_the_last_sender_dropping() {
    // The blocking `recv` runs on a worker and the *assertion* waits with a
    // deadline, rather than the test itself blocking in `recv`.
    //
    // The distinction is not stylistic. This test does detect a broken wake --
    // but by hanging, because an unwoken `recv` never returns and there is no
    // deadline on the main thread to notice. A hang is the worst shape a
    // failure can take: it wedges the whole suite, reports nothing about what
    // broke, and under `cargo mutants` is filed as `timeout` rather than
    // `caught`, so the mutant reads as undetected *and* costs the full deadline.
    // Four mutants in `Drop for Sender` did exactly that, at 120s each.
    //
    // Bounded, the same defect fails in about a second and says which rule it
    // broke. The waiting thread is left blocked when that happens, which is
    // fine: libtest exits the process at the end of the run without joining it.
    let (sender, receiver) = channel();
    let (tx, rx) = std::sync::mpsc::channel();

    let waiter = std::thread::spawn(move || {
        let outcome = receiver.recv();
        // Ignored deliberately: on the failure path the main thread has already
        // given up and dropped its end, and this send is how we find out.
        let _ = tx.send(outcome.is_none());
    });

    let dropper = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        drop(sender);
    });

    // Generous against a loaded machine, and still two orders of magnitude
    // below the mutation deadline it replaces. This is a liveness bound, not a
    // performance assertion.
    match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(disconnected) => assert!(
            disconnected,
            "a woken receiver must observe disconnection, not a notification"
        ),
        Err(_) => panic!(
            "dropping the last sender did not wake a blocked receiver within 5s \
             -- the wake in `Drop for Sender` is the rule this asserts"
        ),
    }

    dropper.join().expect("dropper thread");
    drop(waiter);
}

#[test]
fn queued_items_are_drained_before_disconnection_is_reported() {
    // Teardown must not discard what was already enqueued.
    let (sender, receiver) = channel();
    deliver(&sender, batch(WatchId::from_raw(1), &["a.txt"]));
    deliver(&sender, batch(WatchId::from_raw(1), &["b.txt"]));
    drop(sender);

    assert_eq!(names(&next(&receiver, "a")), vec!["a.txt"]);
    assert_eq!(names(&next(&receiver, "b")), vec!["b.txt"]);
    assert_stream_ended(&receiver, "the stream should be finished");
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
fn has_room_accounts_for_a_pending_latch() {
    // A freed slot that is already owed to a pending latched desync is not
    // actually available to a new notification: send() always flushes every
    // owed latch first, so has_room() must account for that or it reports a
    // slot as available when the very next send would still be Latched.
    let (sender, receiver) = bounded(1);
    let watch = WatchId::from_raw(1);
    fill(&sender, watch, 1);
    assert_eq!(
        sender.send(batch(watch, &["lost.txt"])),
        Delivery::Latched,
        "the one slot is already full"
    );
    assert!(!sender.has_room(), "the queue is full, no room at all");

    // Draining the queued entry frees a slot, but the latch is still owed.
    let _ = next(&receiver, "the queued entry");
    assert!(
        !sender.has_room(),
        "the freed slot is already earmarked for the pending latch flush"
    );

    // Confirm: the next send flushes the latch, not this notification.
    assert_eq!(
        sender.send(batch(watch, &["new.txt"])),
        Delivery::Latched,
        "the freed slot went to the owed desync flush, not this notification"
    );
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

    assert_eq!(names(&next(&receiver, "first")), vec!["fill-0"]);
    assert_eq!(names(&next(&receiver, "second")), vec!["fill-1"]);
    let reported = next(&receiver, "the loss report");
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
    assert_eq!(names(&next(&receiver, "first")), vec!["first.txt"]);
    assert_eq!(names(&next(&receiver, "second")), vec!["second.txt"]);
    deliver(&sender, batch(watch, &["fourth.txt"]));

    assert_eq!(names(&next(&receiver, "third")), vec!["third.txt"]);
    assert!(
        matches!(
            next(&receiver, "the desync"),
            Notification::Desync {
                cause: DesyncCause::QueueFull,
                ..
            }
        ),
        "the loss belongs after the changes that preceded it and before the one that followed"
    );
    assert_eq!(names(&next(&receiver, "fourth")), vec!["fourth.txt"]);
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

    assert_eq!(names(&next(&receiver, "the queued one")), vec!["only.txt"]);
    assert!(matches!(
        next(&receiver, "the latched one"),
        Notification::Desync {
            cause: DesyncCause::QueueFull,
            ..
        }
    ));
    assert!(receiver.try_recv().is_none());
}

/// The next notification, or a failure that says what was expected.
///
/// Use this instead of `recv().expect(..)` wherever a test *knows* an item is
/// owed. The two differ only when something is broken, and that is exactly when
/// the difference matters: an unbounded `recv` that is never woken hangs the
/// whole suite and reports nothing, while this fails in seconds and names the
/// item it was waiting for.
///
/// Measured, not stylistic: four mutants in `Drop for Sender` break the wake
/// that ends a stream, and under `cargo mutants` each cost the full 120s
/// deadline and was filed as `timeout` -- neither counted as caught nor visible
/// as a gap. The tests had detected them all along, by hanging.
///
/// The bound is a liveness check rather than a performance assertion, so it is
/// set far above anything a loaded machine would need.
#[track_caller]
fn next(receiver: &Receiver, expected: &str) -> Notification {
    receiver
        .recv_timeout(Duration::from_secs(5))
        .unwrap_or_else(|| panic!("expected {expected} within 5s, but nothing arrived"))
}

/// Assert that the stream has ended, without hanging if it has not.
///
/// The companion to [`next`], and a separate function because `recv_timeout`
/// cannot express this: it returns `None` both for "the stream ended" and for
/// "nothing arrived in time", so it cannot tell a correct disconnection from
/// the exact bug this is checking for. The blocking `recv` *can* -- it returns
/// only on a real end -- so the wait has to happen on another thread with the
/// deadline enforced here.
///
/// Borrows rather than consuming, and uses no thread. A first attempt moved the
/// receiver onto a worker so the blocking `recv` could be abandoned on failure;
/// that does work, but it cannot be used where something else already borrows
/// the receiver -- the doorbell test, for one -- and the two assertions below
/// are strictly more informative anyway.
///
/// The pair is what makes this unambiguous. `recv_timeout` alone cannot express
/// "the stream ended", because it answers `None` both for that and for "nothing
/// arrived in time". Pairing it with [`Receiver::is_disconnected`] separates
/// them: a real end satisfies both, a broken wake satisfies neither.
#[track_caller]
fn assert_stream_ended(receiver: &Receiver, what: &str) {
    assert!(
        receiver.recv_timeout(Duration::from_secs(5)).is_none(),
        "{what}: a notification arrived where the stream should have ended, or \
         the receiver was never woken -- either way this is not a finished stream"
    );
    assert!(
        receiver.is_disconnected(),
        "{what}: nothing arrived within 5s but the queue does not report \
         disconnection, so the receiver was never woken rather than the stream \
         having ended"
    );
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

    assert_eq!(names(&next(&receiver, "the queued one")), vec!["only.txt"]);
    assert!(matches!(
        next(&receiver, "the latched one"),
        Notification::Desync {
            cause: DesyncCause::QueueFull,
            ..
        }
    ));
    assert_stream_ended(&receiver, "and only then is it finished");
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

    let _ = next(&receiver, "the queued one");
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
        next(&receiver, "the flushed desync"),
        Notification::Desync {
            cause: DesyncCause::QueueFull,
            ..
        }
    ));
    // A report covers the losses since the previous one, so `next.txt` gets its
    // own -- synthesised here, because the queue drained before anything else
    // was sent.
    assert!(matches!(
        next(&receiver, "the second desync"),
        Notification::Desync {
            cause: DesyncCause::QueueFull,
            ..
        }
    ));
    assert_eq!(receiver.latched(), 0, "nothing further is owed");

    // With nothing owed, the released slot carries traffic normally again.
    deliver(&sender, batch(watch, &["now.txt"]));
    assert_eq!(names(&next(&receiver, "now")), vec!["now.txt"]);
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
    assert!(receiver.recv_timeout(Duration::from_secs(5)).is_some());
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
            detail: test_detail(),
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
        detail: test_detail(),
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
        detail: test_detail(),
    });

    // Dropped while its last-sent message is still sitting in the queue,
    // undrained.
    drop(slot);

    assert!(
        sender.reserve().is_none(),
        "the message still occupies the queue's only slot"
    );
    assert!(
        receiver.try_recv().is_some(),
        "the queued message still arrives"
    );
    // The slot is gone, so nothing will ever reserve this unit again for a
    // future standing send -- draining its last message must return the
    // capacity to the general pool rather than leaking it as permanently
    // `reserved` for an owner that no longer exists.
    assert!(
        sender.reserve().is_some(),
        "capacity is returned to the pool once the slot's last message drains"
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
fn a_second_standing_send_while_the_first_is_still_queued_coalesces_in_place() {
    // PR #20 review response: reachable when an interactive watch is answered
    // before its queued question is drained and the retry fails again. Before
    // the fix, a second send while the first was still undrained set
    // `in_flight` back to `true` and decremented `reserved` a second time --
    // double-spending the slot's single carved-out unit and underflowing it.
    let (sender, receiver) = bounded(1);
    let slot = sender.reserve_standing().expect("a slot");
    let watch = WatchId::from_raw(1);
    slot.send(Notification::RetryQuestion {
        watch,
        operation: crate::retry::FaultOperation::Open,
        detail: test_detail(),
    });
    // The first message is still queued, undrained, when the second arrives.
    slot.send(Notification::RetryQuestion {
        watch,
        operation: crate::retry::FaultOperation::Arm,
        detail: test_detail(),
    });

    assert_eq!(receiver.len(), 1, "the two sends coalesced into one entry");
    let Some(Notification::RetryQuestion { operation, .. }) = receiver.try_recv() else {
        panic!("expected the coalesced RetryQuestion");
    };
    assert_eq!(
        operation,
        crate::retry::FaultOperation::Arm,
        "the coalesced entry carries the later send's content"
    );
    assert!(
        sender.reserve().is_none(),
        "the slot's one carved-out unit is still its own, not double-spent"
    );
}

#[test]
fn draining_a_standing_send_frees_no_extra_capacity_for_a_racing_producer() {
    // PR #20 review response: popping a standing entry used to expose its
    // queue slot before the entry's StandingHold restored the permanent
    // reservation, so `free()` briefly over-reported room by one. `take` now
    // restores the reservation atomically with the pop, so draining a
    // standing send never creates capacity a concurrent producer could
    // overcommit -- immediately after drain, the queue must still show
    // exactly the same "no free capacity" state it did before the send.
    let (sender, receiver) = bounded(1);
    let slot = sender.reserve_standing().expect("a slot");
    let watch = WatchId::from_raw(1);
    assert!(
        sender.reserve().is_none(),
        "before the send: the slot's carve-out is the only capacity"
    );

    slot.send(Notification::RetryQuestion {
        watch,
        operation: crate::retry::FaultOperation::Open,
        detail: test_detail(),
    });
    assert!(
        sender.reserve().is_none(),
        "while queued: the reservation lives in the queue slot instead"
    );

    assert!(receiver.try_recv().is_some(), "the message drains");
    assert!(
        sender.reserve().is_none(),
        "after drain: the reservation is restored, not freed to the general pool"
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

    assert_eq!(names(&next(&receiver, "a")), vec!["a.txt"]);
    assert!(matches!(
        next(&receiver, "the desync"),
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
    assert_eq!(names(&next(&receiver, "a")), vec!["a.txt"]);

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

    assert_eq!(names(&next(&receiver, "b")), vec!["b.txt"]);
    assert!(matches!(
        next(&receiver, "the desync"),
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
    let _ = next(&receiver, "a notification");
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
    assert_stream_ended(&receiver, "disconnection");
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

// --- has_pending: the predicate the doorbell is signalled on (D-41, M14.3) ---

#[test]
fn a_drained_queue_with_a_loss_owed_is_empty_but_still_has_something_to_take() {
    // The `has_room` shape (workspace DESIGN-NOTES): a predicate must hold in
    // the condition its caller actually uses it in. `is_empty` answers "how
    // deep is the queue", which is not the question a drain loop asks.
    let (sender, receiver) = bounded(1);
    let watch = WatchId::from_raw(77);
    deliver(&sender, batch(watch, &["queued.txt"]));
    assert_eq!(sender.send(batch(watch, &["lost.txt"])), Delivery::Latched);

    assert_eq!(
        names(&next(&receiver, "the queued batch")),
        vec!["queued.txt"]
    );

    // Drained, but a loss is still owed.
    assert!(receiver.is_empty(), "nothing is *queued*");
    assert_eq!(receiver.len(), 0);
    assert_eq!(receiver.latched(), 1, "but a loss is owed");
    assert!(
        receiver.has_pending(),
        "so there is still something to take -- what `is_empty` cannot say"
    );

    assert!(matches!(
        next(&receiver, "the synthesised loss report"),
        Notification::Desync {
            cause: DesyncCause::QueueFull,
            ..
        }
    ));
    assert!(!receiver.has_pending(), "now genuinely drained");
}

#[test]
fn has_pending_tracks_the_doorbell_exactly_through_a_latched_loss() {
    // D-41's invariant is that the event is signalled exactly when the receiver
    // has something to take. `has_pending` is that same predicate made public,
    // so the two must agree at every step -- including the drained-with-a-loss-
    // owed state, which is where a client that waits on the doorbell and then
    // tests `is_empty` would spin without ever collecting the report.
    let (sender, receiver) = bounded(1);
    let watch = WatchId::from_raw(78);
    let doorbell = receiver.doorbell().expect("a doorbell");

    assert_eq!(receiver.has_pending(), is_signalled(doorbell));
    assert!(!receiver.has_pending());

    deliver(&sender, batch(watch, &["queued.txt"]));
    assert_eq!(sender.send(batch(watch, &["lost.txt"])), Delivery::Latched);
    assert_eq!(receiver.has_pending(), is_signalled(doorbell));

    next(&receiver, "the queued batch");
    assert_eq!(
        receiver.has_pending(),
        is_signalled(doorbell),
        "still agreed while only a latched loss remains"
    );
    assert!(is_signalled(doorbell), "the doorbell is still ringing");
    assert!(receiver.is_empty(), "yet the queue reports itself empty");

    next(&receiver, "the loss report");
    assert_eq!(receiver.has_pending(), is_signalled(doorbell));
    assert!(!is_signalled(doorbell), "and only now does it stop");
}

#[test]
fn a_disconnected_empty_queue_still_has_something_to_take() {
    // The third arm of "something to take": the end of the stream. A client
    // must learn the stream ended rather than wait for what cannot arrive.
    let (sender, receiver) = channel();
    assert!(!receiver.has_pending());
    drop(sender);

    assert!(receiver.is_empty());
    assert_eq!(receiver.latched(), 0);
    assert!(
        receiver.has_pending(),
        "disconnection is collectable: recv returns None rather than blocking"
    );
    assert_stream_ended(&receiver, "the stream should be finished");
}

// --- the resume edge must agree with has_room (PR #42 review) ---

/// A [`Resume`] that only records how many times it was prodded.
#[derive(Default)]
struct CountingResumer {
    prods: std::sync::atomic::AtomicUsize,
}

impl CountingResumer {
    fn count(&self) -> usize {
        self.prods.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl super::Resume for CountingResumer {
    fn resume(&self) {
        self.prods.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

#[test]
fn a_parked_producer_is_prodded_at_the_slot_has_room_actually_becomes_true() {
    // The wedge: `has_room` is `free() > latched.len()` but the wake edge was
    // `free() == 1`. With a latch owed, the prod fired one slot too early --
    // the re-check still said "no room" -- and the next drain moved `free()`
    // to 2, which was no longer the edge, so no prod ever came again. A bound
    // greater than one is what exposes it; at capacity 1 the two expressions
    // coincide.
    let (sender, receiver) = bounded(4);
    let watch = WatchId::from_raw(101);
    let producer = Arc::new(CountingResumer::default());
    sender.register_resume(&producer);

    fill(&sender, watch, 4);
    assert_eq!(sender.send(batch(watch, &["lost.txt"])), Delivery::Latched);
    assert_eq!(receiver.latched(), 1, "a loss is owed");
    assert!(!sender.has_room(), "saturated");
    assert_eq!(producer.count(), 0, "nothing freed yet");

    // free() == 1, latched == 1: the old edge fired here, but has_room is
    // still false, so a prod now is wasted -- and, worse, it is the *only*
    // one that ever comes.
    next(&receiver, "first");
    assert!(
        !sender.has_room(),
        "the freed slot is owed to the latch flush, not to a new notification"
    );
    assert_eq!(
        producer.count(),
        0,
        "prodding while has_room is still false burns the one wake the \
         producer was going to get"
    );

    // free() == 2, latched == 1: has_room becomes true, so the prod must land
    // here. The old edge skipped it and never fired again.
    next(&receiver, "second");
    assert!(sender.has_room(), "room genuinely exists now");
    assert_eq!(
        producer.count(),
        1,
        "the producer must have been prodded at the transition, or it stays \
         parked forever with room available"
    );
}

#[test]
fn draining_only_latched_reports_still_reaches_the_resume_edge() {
    // Taking a latched report leaves `free()` untouched and shrinks `latched`,
    // so the transition can happen without any queued item being taken. An
    // edge phrased on `free()` alone cannot see this at all.
    let (sender, receiver) = bounded(2);
    let producer = Arc::new(CountingResumer::default());
    sender.register_resume(&producer);

    // Three watches saturate the queue and two more are left owed a report.
    fill(&sender, WatchId::from_raw(1), 2);
    for id in [2, 3] {
        assert_eq!(
            sender.send(batch(WatchId::from_raw(id), &["lost.txt"])),
            Delivery::Latched
        );
    }
    assert_eq!(receiver.latched(), 2);

    next(&receiver, "first queued");
    next(&receiver, "second queued");
    // Queue empty, free() == 2, latched == 2: still no room.
    assert!(!sender.has_room());
    assert_eq!(producer.count(), 0);

    // Taking one synthesised report is what creates the room.
    assert!(matches!(
        next(&receiver, "a synthesised loss report"),
        Notification::Desync {
            cause: DesyncCause::QueueFull,
            ..
        }
    ));
    assert!(sender.has_room());
    assert_eq!(
        producer.count(),
        1,
        "the edge must be reachable by draining latched reports alone"
    );
}

#[test]
fn a_take_that_took_nothing_is_not_a_crossing_and_does_not_prod() {
    // `Receiver::try_recv` is the only caller that can report `took_one ==
    // false`, and the edge has to be a *crossing*: room that was already there
    // is not a transition into having room. Without that guard, any poll of an
    // empty queue that happens to sit on the edge prods again, so a producer
    // parked behind a saturated queue would be woken by pollers rather than by
    // capacity -- and the wake would carry no room with it.
    let (sender, receiver) = bounded(1);
    let watch = WatchId::from_raw(202);
    let producer = Arc::new(CountingResumer::default());
    sender.register_resume(&producer);

    // Empty, and already sitting on the edge (`best_effort_room() == 1`), so
    // the `took_one` guard is the only thing separating this no-op poll from a
    // real crossing.
    assert!(receiver.try_recv().is_none(), "nothing to take");
    assert_eq!(
        producer.count(),
        0,
        "a poll that took nothing crossed no edge"
    );

    // The genuine crossing: saturate, then take the one item.
    fill(&sender, watch, 1);
    assert!(!sender.has_room(), "saturated");
    assert!(receiver.try_recv().is_some(), "the queued item");
    assert_eq!(producer.count(), 1, "taking the item crossed the edge");

    // The queue now sits on that same edge again, but nothing was taken, so the
    // prod must not repeat.
    assert!(receiver.try_recv().is_none(), "drained");
    assert_eq!(
        producer.count(),
        1,
        "an empty poll must not re-fire an edge it did not cross"
    );
}

#[test]
fn draining_a_standing_send_returns_the_carve_out_to_the_slot_not_to_the_pool() {
    // A `cargo mutants` run flagged the reservation accounting, and chasing it
    // showed the carve-out's *reachable* release -- the one `take` performs
    // inline with the pop -- was covered only incidentally, by tests asserting
    // that sends succeed rather than that capacity is conserved.
    //
    // (The copy of that accounting in `StandingHold::drop` is a different
    // matter: it is not reachable in the current design, and no test here can
    // cover it. See the note on that impl.)
    //
    // `unreserved() == capacity - queue.len() - reserved`, so getting this
    // wrong does not merely lose the slot's guarantee: decrementing inflates
    // the best-effort pool, handing out capacity that is supposed to be carved
    // out. The assertion below is therefore about what the *general* path can
    // take, which is what a wrong sign actually corrupts.
    let (sender, receiver) = bounded(2);
    let slot = sender.reserve_standing().expect("a slot");
    let watch = WatchId::from_raw(1);

    // One unit is carved out for the slot, so exactly one is best-effort.
    let held = sender.reserve().expect("one unreserved unit exists");
    assert!(
        sender.reserve().is_none(),
        "the standing slot's carve-out is not available to the best-effort path"
    );
    drop(held);

    // Send through the slot and drain it: the queued entry stands in for the
    // reservation while it is queued, and dropping the hold on drain must give
    // the carve-out back to the slot.
    slot.send(Notification::RetryQuestion {
        watch,
        operation: crate::retry::FaultOperation::Open,
        detail: test_detail(),
    });
    assert!(receiver.try_recv().is_some(), "the standing send arrives");

    // The state must be exactly what it was before the send.
    let held = sender.reserve().expect("the one best-effort unit is back");
    assert!(
        sender.reserve().is_none(),
        "the carve-out must return to the slot, not to the best-effort pool -- \
         a second unit here means the reservation was released twice"
    );
    drop(held);

    // And the slot must still be able to use it.
    slot.send(Notification::RetryQuestion {
        watch,
        operation: crate::retry::FaultOperation::Open,
        detail: test_detail(),
    });
    assert!(
        receiver.try_recv().is_some(),
        "the slot can send again, which is what the carve-out is for"
    );
}

#[test]
fn a_standing_slot_keeps_its_carve_out_across_many_send_drain_cycles() {
    // The accounting must be stable rather than merely correct once: a sign
    // error that happened to cancel out over one cycle would still drift here.
    let (sender, receiver) = bounded(2);
    let slot = sender.reserve_standing().expect("a slot");
    let watch = WatchId::from_raw(1);

    for cycle in 0..8 {
        slot.send(Notification::RetryQuestion {
            watch,
            operation: crate::retry::FaultOperation::Open,
            detail: test_detail(),
        });
        assert!(
            receiver.try_recv().is_some(),
            "cycle {cycle}: the standing send arrives"
        );

        let held = sender.reserve().expect("one best-effort unit, every cycle");
        assert!(
            sender.reserve().is_none(),
            "cycle {cycle}: the carve-out must still be carved out"
        );
        drop(held);
    }
}

#[test]
fn the_opaque_handles_name_themselves_when_formatted() {
    // Both `Debug` impls survived being replaced with a body that writes
    // nothing. They are hand-written and deliberately opaque -- neither type
    // can usefully show its interior, and `finish_non_exhaustive` says so --
    // but "opaque" is not "empty": a `StandingSlot` that formats as nothing at
    // all makes a panic message or a log line name no type, which is the one
    // job these impls have.
    let (sender, _receiver) = bounded(2);
    let slot = sender.reserve_standing().expect("a slot");
    let reservation = sender.reserve().expect("a reservation");

    let rendered = format!("{slot:?}");
    assert!(
        rendered.contains("StandingSlot"),
        "a standing slot must name itself when formatted, got {rendered:?}"
    );

    let rendered = format!("{reservation:?}");
    assert!(
        rendered.contains("Reservation"),
        "a reservation must name itself when formatted, got {rendered:?}"
    );

    let rendered = format!("{sender:?}");
    assert!(
        rendered.contains("Sender"),
        "a sender must name itself when formatted, got {rendered:?}"
    );
}

#[test]
fn a_formatted_receiver_reports_the_state_a_wedge_is_diagnosed_from() {
    // Unlike the opaque handles above, `Receiver`'s `Debug` is the one place
    // the queue's occupancy is visible from outside, and those four numbers are
    // exactly what someone diagnosing a stalled watcher reads. A body that
    // writes nothing, or a `disconnected` flag that reports the opposite of the
    // truth, both survived mutation -- and an inverted flag is worse than no
    // flag, because it misleads at the moment it is consulted.
    let (sender, receiver) = bounded(2);
    let watch = WatchId::from_raw(303);

    fill(&sender, watch, 2);
    assert_eq!(sender.send(batch(watch, &["lost.txt"])), Delivery::Latched);
    assert_eq!(
        format!("{receiver:?}"),
        "Receiver { queued: 2, capacity: 2, latched: 1, disconnected: false, .. }",
        "a live receiver must report its occupancy and that it is still connected"
    );

    // Dropping the last sender is the transition the flag exists to show.
    drop(sender);
    assert_eq!(
        format!("{receiver:?}"),
        "Receiver { queued: 2, capacity: 2, latched: 1, disconnected: true, .. }",
        "once every sender is gone the receiver must say so"
    );
}
