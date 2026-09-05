// Copyright (c) Mike Grier.

//! Tests for the experimental permit-claiming MPSC.
//!
//! These are not merely "does it enqueue". The shape exists to make a
//! particular hazard impossible, so the tests that matter are the ones about
//! *admission*: that the queue never admits more claimants than it has slots,
//! that a reservation holds room back without occupying a position, and that an
//! overdrawn permit count is always restored.

use crate::error::TryRecvError;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use super::*;
use crate::error::PushError;

/// A payload that reports its own destruction, so leaks and double-drops in
/// teardown are observable rather than assumed.
#[derive(Debug)]
struct Tracked(Arc<AtomicUsize>);

impl Drop for Tracked {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn a_capacity_below_the_minimum_is_refused() {
    assert!(bounded::<u32>(1).is_err());
    assert!(bounded::<u32>(0).is_err());
}

#[test]
fn a_capacity_that_is_not_a_power_of_two_is_refused() {
    assert!(bounded::<u32>(3).is_err());
    assert!(bounded::<u32>(100).is_err());
}

#[test]
fn a_capacity_above_the_maximum_is_refused() {
    assert!(bounded::<u32>(BOUNDS_MAX.wrapping_mul(2)).is_err());
}

// Deliberately no test that `BOUNDS_MAX` is itself an acceptable capacity. That
// is a fact about constants, and the module's `const _: () = { ... }` block
// already asserts both halves of it -- a const assertion fails the build rather
// than a run somebody chose to make, so a test here would be the weaker
// statement of a property already guaranteed.

#[test]
fn an_item_pushed_is_the_item_popped() {
    let (tx, rx) = bounded::<u32>(4).expect("a valid capacity");
    tx.push(7).expect("room");
    assert_eq!(rx.pop(), Ok(7));
}

#[test]
fn popping_an_empty_queue_reports_nothing() {
    let (_tx, rx) = bounded::<u32>(4).expect("a valid capacity");
    assert_eq!(rx.pop(), Err(TryRecvError::Empty));
}

#[test]
fn items_come_back_in_the_order_they_went_in() {
    let (tx, rx) = bounded::<u32>(8).expect("a valid capacity");
    for value in 0..8 {
        tx.push(value).expect("room");
    }
    for value in 0..8 {
        assert_eq!(rx.pop(), Ok(value));
    }
    assert_eq!(rx.pop(), Err(TryRecvError::Empty));
}

#[test]
fn the_queue_holds_exactly_its_capacity_and_then_refuses() {
    let (tx, rx) = bounded::<u32>(4).expect("a valid capacity");
    for value in 0..4 {
        tx.push(value).expect("room");
    }
    assert_eq!(rx.len(), 4);
    match tx.push(99) {
        Err(PushError::Full(item)) => assert_eq!(item, 99),
        other => panic!("expected Full, got {:?}", other.is_ok()),
    }
}

#[test]
fn a_refusal_hands_the_item_back_and_is_counted() {
    let (tx, _rx) = bounded::<u32>(2).expect("a valid capacity");
    tx.push(1).expect("room");
    tx.push(2).expect("room");
    assert_eq!(tx.refused(), 0);
    assert!(tx.push(3).is_err());
    assert_eq!(tx.refused(), 1);
}

#[test]
fn a_refusal_leaves_the_permit_count_intact() {
    // The optimistic decrement overdraws and must undo. If it did not, a single
    // refusal would permanently cost the queue a slot -- so the queue must
    // still accept exactly `capacity` items after many refusals.
    let (tx, rx) = bounded::<u32>(4).expect("a valid capacity");
    for value in 0..4 {
        tx.push(value).expect("room");
    }
    for _ in 0..1_000 {
        assert!(tx.push(99).is_err());
    }
    for value in 0..4 {
        assert_eq!(rx.pop(), Ok(value));
    }
    // Every slot came back.
    for value in 0..4 {
        tx.push(value).expect("room after draining");
    }
    assert_eq!(rx.len(), 4);
}

#[test]
fn a_concurrent_refusal_storm_leaves_the_permit_count_intact() {
    // The overdraft is bounded by the number of concurrent claimants, so the
    // count can go transiently negative. What must not happen is that it fails
    // to return to exactly the capacity.
    let (tx, rx) = bounded::<u32>(4).expect("a valid capacity");
    for value in 0..4 {
        tx.push(value).expect("room");
    }
    thread::scope(|scope| {
        for _ in 0..8 {
            let tx = tx.clone();
            scope.spawn(move || {
                for _ in 0..2_000 {
                    assert!(tx.push(99).is_err());
                }
            });
        }
    });
    for value in 0..4 {
        assert_eq!(rx.pop(), Ok(value));
    }
    for value in 0..4 {
        tx.push(value).expect("room after draining");
    }
    assert!(tx.push(99).is_err(), "capacity must not have grown");
}

#[test]
fn the_ring_is_reused_across_many_laps() {
    // Far more pushes than slots, so every slot serves many positions. This is
    // ring wraparound, not position wraparound.
    let (tx, rx) = bounded::<u32>(4).expect("a valid capacity");
    for value in 0..10_000 {
        tx.push(value).expect("room");
        assert_eq!(rx.pop(), Ok(value));
    }
    assert_eq!(rx.pop(), Err(TryRecvError::Empty));
}

#[test]
fn a_reservation_holds_room_back_from_other_producers() {
    let (tx, _rx) = bounded::<u32>(4).expect("a valid capacity");
    let _reservation = tx.reserve().expect("room");
    // Three slots remain for best-effort pushes.
    for value in 0..3 {
        tx.push(value).expect("room");
    }
    assert!(
        tx.push(99).is_err(),
        "the reserved slot must not be available to a push"
    );
}

#[test]
fn a_reservation_delivers_even_when_the_queue_is_otherwise_full() {
    let (tx, rx) = bounded::<u32>(4).expect("a valid capacity");
    let reservation = tx.reserve().expect("room");
    for value in 0..3 {
        tx.push(value).expect("room");
    }
    assert!(tx.push(99).is_err());
    // Reserved is guaranteed: this cannot fail.
    reservation.send(42).expect("the consumer is still here");
    assert_eq!(rx.len(), 4);
    for value in 0..3 {
        assert_eq!(rx.pop(), Ok(value));
    }
    assert_eq!(rx.pop(), Ok(42));
}

#[test]
fn a_reservation_dropped_unredeemed_gives_the_room_back() {
    let (tx, _rx) = bounded::<u32>(4).expect("a valid capacity");
    {
        let _reservation = tx.reserve().expect("room");
        for value in 0..3 {
            tx.push(value).expect("room");
        }
        assert!(tx.push(99).is_err());
    }
    tx.push(99)
        .expect("the dropped reservation released its slot");
}

#[test]
fn an_outstanding_reservation_does_not_block_the_consumer() {
    // The semantic that distinguishes this from taking a ticket at reserve
    // time: a reservation withholds capacity but occupies no position, so items
    // pushed after it are delivered without waiting for it.
    let (tx, rx) = bounded::<u32>(8).expect("a valid capacity");
    let reservation = tx.reserve().expect("room");
    tx.push(1).expect("room");
    tx.push(2).expect("room");
    assert_eq!(rx.pop(), Ok(1));
    assert_eq!(rx.pop(), Ok(2));
    assert_eq!(rx.pop(), Err(TryRecvError::Empty));
    reservation.send(3).expect("the consumer is still here");
    assert_eq!(rx.pop(), Ok(3));
}

#[test]
fn every_reservation_the_capacity_allows_can_be_taken_at_once() {
    let (tx, rx) = bounded::<u32>(4).expect("a valid capacity");
    let reservations: Vec<_> = (0..4).map(|_| tx.reserve().expect("room")).collect();
    assert!(tx.push(99).is_err(), "every slot is spoken for");
    for (value, reservation) in reservations.into_iter().enumerate() {
        reservation
            .send(value as u32)
            .expect("the consumer is still here");
    }
    for value in 0..4 {
        assert_eq!(rx.pop(), Ok(value));
    }
}

#[test]
fn a_push_to_a_departed_consumer_is_reported_as_disconnection() {
    let (tx, rx) = bounded::<u32>(4).expect("a valid capacity");
    drop(rx);
    match tx.push(1) {
        Err(PushError::Disconnected(item)) => assert_eq!(item, 1),
        _ => panic!("expected Disconnected"),
    }
}

#[test]
fn a_full_queue_whose_consumer_is_gone_reports_disconnection_not_fullness() {
    // Telling a caller to retry a queue that will never drain is telling it to
    // spin forever, so disconnection wins over fullness.
    let (tx, rx) = bounded::<u32>(2).expect("a valid capacity");
    tx.push(1).expect("room");
    tx.push(2).expect("room");
    drop(rx);
    match tx.push(3) {
        Err(PushError::Disconnected(_)) => {}
        _ => panic!("expected Disconnected on a full, consumerless queue"),
    }
}

#[test]
fn undrained_items_are_dropped_exactly_once_at_teardown() {
    let drops = Arc::new(AtomicUsize::new(0));
    {
        let (tx, rx) = bounded::<Tracked>(4).expect("a valid capacity");
        for _ in 0..3 {
            tx.push(Tracked(Arc::clone(&drops))).expect("room");
        }
        // Take one, so teardown must drop exactly the two that remain.
        drop(rx.pop().expect("an item"));
        assert_eq!(drops.load(Ordering::Relaxed), 1);
    }
    assert_eq!(
        drops.load(Ordering::Relaxed),
        3,
        "every item must be dropped exactly once"
    );
}

#[test]
fn teardown_after_a_lap_drops_only_the_live_items() {
    // Slots hold stale bit patterns from earlier laps; teardown must not drop
    // those a second time.
    let drops = Arc::new(AtomicUsize::new(0));
    {
        let (tx, rx) = bounded::<Tracked>(4).expect("a valid capacity");
        for _ in 0..12 {
            tx.push(Tracked(Arc::clone(&drops))).expect("room");
            drop(rx.pop().expect("an item"));
        }
        assert_eq!(drops.load(Ordering::Relaxed), 12);
        // Two live items remain at teardown.
        tx.push(Tracked(Arc::clone(&drops))).expect("room");
        tx.push(Tracked(Arc::clone(&drops))).expect("room");
    }
    assert_eq!(drops.load(Ordering::Relaxed), 14);
}

#[test]
fn many_producers_deliver_every_item_exactly_once() {
    const PRODUCERS: u32 = 8;
    const EACH: u32 = 2_000;

    let (tx, rx) = bounded::<u32>(64).expect("a valid capacity");
    let mut seen = vec![0_u32; (PRODUCERS * EACH) as usize];

    thread::scope(|scope| {
        for producer in 0..PRODUCERS {
            let tx = tx.clone();
            scope.spawn(move || {
                for index in 0..EACH {
                    let value = producer * EACH + index;
                    // Bounded queue: retry rather than lose the item.
                    while tx.push(value).is_err() {
                        std::hint::spin_loop();
                    }
                }
            });
        }
        let mut taken = 0;
        while taken < PRODUCERS * EACH {
            if let Ok(value) = rx.pop() {
                seen[value as usize] += 1;
                taken += 1;
            } else {
                std::hint::spin_loop();
            }
        }
    });

    assert!(
        seen.iter().all(|&count| count == 1),
        "every item must arrive exactly once"
    );
}

#[test]
fn the_queue_never_admits_more_claimants_than_it_has_slots() {
    // The property the shape exists for, stated as an observable: at no moment
    // may the number of items held exceed the capacity. A permit system that
    // over-admitted would show up here as a length beyond the bound.
    const CAPACITY: usize = 16;
    let (tx, rx) = bounded::<u32>(CAPACITY).expect("a valid capacity");

    thread::scope(|scope| {
        for _ in 0..8 {
            let tx = tx.clone();
            scope.spawn(move || {
                for value in 0..4_000 {
                    let _ = tx.push(value);
                }
            });
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(400);
        while std::time::Instant::now() < deadline {
            assert!(
                rx.len() <= CAPACITY,
                "the queue reported holding more than its capacity"
            );
            let _ = rx.pop();
        }
    });
    while rx.pop().is_ok() {}
}

#[test]
fn a_producer_and_a_reservation_contend_for_the_same_room_without_overdrawing() {
    // `reserve` and `push` are two claimants on one resource. If they could
    // both be admitted to the last slot, the queue would owe a slot that does
    // not exist -- which is the hazard D-17's packing exists to prevent, and
    // which this shape prevents with a single shared permit count instead.
    for _ in 0..200 {
        let (tx, rx) = bounded::<u32>(2).expect("a valid capacity");
        tx.push(0).expect("room");
        // One slot left, two claimants racing for it.
        let reserver = tx.clone();
        let pusher = tx.clone();
        let (reserved, pushed) = thread::scope(|scope| {
            let a = scope.spawn(move || reserver.reserve().ok());
            let b = scope.spawn(move || pusher.push(1).is_ok());
            (a.join().expect("no panic"), b.join().expect("no panic"))
        });
        let claims = usize::from(reserved.is_some()) + usize::from(pushed);
        assert!(claims <= 1, "both claimants took the same single slot");
        if let Some(reservation) = reserved {
            reservation.send(2).expect("the consumer is still here");
        }
        drop(rx);
    }
}

#[test]
fn redeeming_against_a_departed_consumer_hands_the_item_back() {
    // The whole point of a reservation is that the message it stands for is
    // not lost. This used to publish into a ring nobody would ever read: the
    // item was destroyed at teardown and the caller was never told, which is a
    // silent loss of exactly the message the reservation guaranteed.
    //
    // `reserving_mpsc::Reservation::send` has always answered `Disconnected`
    // here, and this module claims only the *admission* protocol differs, so
    // the divergence was undisclosed as well as wrong. Raised in the PR #56
    // review.
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    let reservation = tx.reserve().expect("an empty queue has room");
    drop(rx);

    let returned = reservation
        .send(7)
        .expect_err("a departed consumer must not swallow the item");
    assert_eq!(returned.0, 7, "the item itself must come back, not a copy");
}

#[test]
fn a_refused_redemption_gives_the_room_back() {
    // The complement, so the refusal cannot be bought by leaking the slot the
    // reservation was holding: the permit must return to the pool exactly as
    // it does when a reservation is simply dropped.
    let (tx, rx) = bounded::<u32>(2).expect("2 is a valid capacity");
    let reservation = tx.reserve().expect("an empty queue has room");
    drop(rx);
    let _ = reservation.send(7);

    // Both slots are free again, so both may be reserved.
    assert!(tx.reserve().is_ok(), "the refused slot must be reusable");
    assert!(tx.reserve().is_ok(), "and the queue's other slot with it");
}

// ---------------------------------------------------------------------------
// The consumer's own accessors, and the disconnection question `pop` answers.
//
// A mutation run found every one of these uncovered: `capacity`, `is_empty`,
// `refused` and `is_disconnected` could each be replaced by a constant with the
// suite still passing. This shape is experimental, but a caller who enables the
// feature gets the same surface as the shipping shapes and is entitled to the
// same evidence that it works.
// ---------------------------------------------------------------------------

#[test]
fn the_consumer_reports_the_capacity_it_was_built_with() {
    let (_tx, rx) = bounded::<u32>(8).expect("8 is a valid capacity");
    assert_eq!(rx.capacity(), 8);

    let (_tx, rx) = bounded::<u32>(2).expect("2 is a valid capacity");
    assert_eq!(
        rx.capacity(),
        2,
        "a second capacity, so a constant cannot satisfy both"
    );
}

#[test]
fn the_consumer_sees_the_queue_fill_and_empty() {
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    assert!(rx.is_empty(), "a fresh queue holds nothing");
    assert_eq!(rx.len(), 0);

    tx.push(1).expect("an empty queue has room");
    assert!(!rx.is_empty(), "and not once an item is pushed");
    assert_eq!(rx.len(), 1);

    assert_eq!(rx.pop(), Ok(1));
    assert!(rx.is_empty(), "and empty again once it is taken");
    assert_eq!(rx.len(), 0);
}

#[test]
fn the_consumer_counts_refusals() {
    let (tx, rx) = bounded::<u32>(2).expect("2 is a valid capacity");
    assert_eq!(rx.refused(), 0, "nothing has been refused yet");

    tx.push(1).expect("an empty queue has room");
    tx.push(2).expect("one slot remains");
    assert!(tx.push(3).is_err(), "the third does not fit");

    assert_eq!(rx.refused(), 1, "and the refusal is counted");
    assert!(tx.push(4).is_err());
    assert_eq!(rx.refused(), 2, "and counted again, rather than latched");
}

#[test]
fn the_consumer_learns_when_every_producer_is_gone() {
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    let second = tx.clone();
    assert!(!rx.is_disconnected(), "two producers are alive");

    drop(tx);
    assert!(
        !rx.is_disconnected(),
        "and one still is -- disconnection is every producer, not any"
    );

    drop(second);
    assert!(rx.is_disconnected(), "now none are");
}

#[test]
fn an_empty_queue_is_distinguishable_from_a_finished_one() {
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    assert_eq!(rx.pop(), Err(TryRecvError::Empty));

    drop(tx);
    assert_eq!(rx.pop(), Err(TryRecvError::Disconnected));
}

#[test]
fn a_departed_producers_items_are_delivered_before_the_disconnection() {
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    tx.push(1).expect("an empty queue has room");
    drop(tx);

    assert_eq!(rx.pop(), Ok(1), "the item comes first");
    assert_eq!(
        rx.pop(),
        Err(TryRecvError::Disconnected),
        "and only then the end of the stream"
    );
}

#[test]
fn a_dropped_producer_that_was_not_the_last_leaves_the_stream_open() {
    // The `Drop` impl decrements a count and only signals at zero. A mutation
    // run found both the decrement and its `== 1` test uncovered, so this
    // asserts the boundary from both sides rather than only that dropping
    // everything eventually disconnects.
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    let second = tx.clone();
    let third = tx.clone();

    drop(second);
    drop(third);
    assert!(
        !rx.is_disconnected(),
        "two of three gone is not the end of the stream"
    );
    assert_eq!(rx.pop(), Err(TryRecvError::Empty));

    drop(tx);
    assert!(rx.is_disconnected(), "the last one is");
    assert_eq!(rx.pop(), Err(TryRecvError::Disconnected));
}

#[test]
fn a_dropped_reservation_releases_its_hold_on_the_stream() {
    // A reservation counts as a producer, so dropping the last *handle* while a
    // reservation is outstanding must not end the stream -- and dropping the
    // reservation afterwards must.
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    let slot = tx.reserve().expect("an empty queue has room");
    drop(tx);
    assert!(
        !rx.is_disconnected(),
        "a reservation is a promise of a message still to come"
    );

    drop(slot);
    assert!(
        rx.is_disconnected(),
        "and releasing it without sending ends the stream"
    );
}

#[test]
fn only_the_last_producer_to_leave_rings_the_doorbell() {
    // **The count and the signal are separate consequences of the same drop,
    // and only the count is visible through the public surface.** This shape
    // exposes no `doorbell`, `arm` or `recv`, so a test written against
    // `is_disconnected` alone cannot see whether the signal happened -- a
    // mutation run proved it, surviving `==` -> `!=` here while every such test
    // passed. The ring is what a waiting consumer would depend on, so it is
    // asserted directly through the shared state rather than left unobserved.
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    let second = tx.clone();
    // **The handle must be requested first, or the signal is a no-op.** The
    // doorbell does nothing until a handle exists, and this shape exposes no
    // way to ask for one -- so through its public surface the ring is
    // unobservable, which is precisely why the mutation survived. Asking
    // through the shared state makes the signalling testable now, and is the
    // behaviour the shape needs to already be correct if it ever gains the
    // waitable surface its siblings have.
    let _handle = rx
        .shared
        .doorbell
        .handle()
        .expect("the event can be created");
    let before = rx.shared.doorbell.rings();

    drop(second);
    assert_eq!(
        rx.shared.doorbell.rings(),
        before,
        "a producer leaving while another remains has ended nothing, so waking a consumer \
         would be a spurious wakeup"
    );

    drop(tx);
    assert_eq!(
        rx.shared.doorbell.rings(),
        before + 1,
        "the last one to leave ends the stream, and a consumer parked on the doorbell has to \
         be told or it waits forever"
    );
}

#[test]
fn only_the_last_reservation_to_leave_rings_the_doorbell() {
    // A reservation counts as a producer, so the same boundary applies to it,
    // and it has its own `Drop` impl -- which a mutation run also found
    // unobserved.
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    let slot = tx.reserve().expect("an empty queue has room");
    // As above: no handle, no signal.
    let _handle = rx
        .shared
        .doorbell
        .handle()
        .expect("the event can be created");
    let before = rx.shared.doorbell.rings();

    drop(tx);
    assert_eq!(
        rx.shared.doorbell.rings(),
        before,
        "the handle went but the reservation still promises a message"
    );

    drop(slot);
    assert_eq!(
        rx.shared.doorbell.rings(),
        before + 1,
        "and releasing it unsent is what finally ends the stream"
    );
}
