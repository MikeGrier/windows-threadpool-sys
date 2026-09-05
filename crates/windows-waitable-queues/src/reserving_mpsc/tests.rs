// Copyright (c) Mike Grier.

//! Tests for the reserving MPSC bounded array queue.
//!
//! The shape's queueing behaviour is `slotwise_mpsc`'s and is covered there; what is
//! tested here is the part that is different -- the reservation, the packed
//! claim word, and the ways the two interact with everything else.
//!
//! The load-bearing property is stated once and asserted from several angles:
//! **a granted reservation is always redeemable.** A test that only checked
//! "reserve then send works on an idle queue" would assert nothing, because the
//! failure mode is a reservation granted while a racing producer takes the last
//! slot -- so the interesting cases all put the queue under pressure first.

use super::{
    BOUNDS_MAX, Consumer, MAX_RESERVED, POSITION_MASK, Producer, Reservation, advance, bounded,
    bounded_with, claim_word, position_of, reserved_of,
};
use crate::race_hooks;
use crate::{Disposal, Options};
// The trait is imported anonymously because this module also names the concrete
// `Consumer` type, and only its `drain` method is wanted here. That the two can
// coexist is the point made in `traits`: the trait is named for the role and the
// handle is named for the role, and a caller who wants only the methods says so.
use crate::Consumer as _;
use crate::{Bounded, PushError, RecvError, Reserving};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

/// Counts its own drops, so a test can prove an item was destroyed rather than
/// leaked. `Arc<AtomicUsize>` rather than a `static`, so tests that run
/// concurrently in one process cannot see each other's counts.
#[derive(Debug)]
struct DropCounter(Arc<AtomicUsize>);

impl Drop for DropCounter {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

/// Fills every slot a best-effort producer is allowed to take.
///
/// Returns how many went in, which is the capacity less whatever is reserved.
fn fill<T: Clone>(producer: &Producer<T>, item: T) -> usize {
    let mut pushed = 0;
    while producer.push(item.clone()).is_ok() {
        pushed += 1;
    }
    pushed
}

// ---------------------------------------------------------------------------
// The packed claim word.
//
// Tested directly as well as through the queue: the packing is arithmetic, and
// arithmetic is worth checking at its edges rather than only through the six
// layers of queue that happen to use it.
// ---------------------------------------------------------------------------

#[test]
fn the_claim_word_round_trips_both_halves() {
    // Written against the split's own constants rather than against `u32`'s
    // extremes, because the position is no longer 32 bits by definition: it is
    // carried in a `u64` and bounded by `POSITION_MASK`. Spelling the edges as
    // `u32::MAX` would have quietly stopped testing the edges the moment the
    // apportionment changed, while still passing.
    let max_reserved = MAX_RESERVED as u32;
    for &reserved in &[0_u32, 1, 2, 1000, max_reserved - 1, max_reserved] {
        for &position in &[0_u64, 1, 2, 1000, POSITION_MASK - 1, POSITION_MASK] {
            let word = claim_word(reserved, position);
            assert_eq!(
                (reserved_of(word), position_of(word)),
                (reserved, position),
                "packing must be lossless in both halves, including at their extremes"
            );
        }
    }
}

#[test]
fn the_two_halves_do_not_bleed_into_each_other() {
    // The mistake packing invites: a position that wraps must not carry into
    // the reservation count, and a count must not appear as a position.
    let word = claim_word(0, POSITION_MASK);
    assert_eq!(
        reserved_of(word),
        0,
        "a maximal position leaves the count at zero"
    );

    let word = claim_word(MAX_RESERVED as u32, 0);
    assert_eq!(
        position_of(word),
        0,
        "a maximal count leaves the position at zero"
    );

    // And an increment of the position at its maximum wraps within its own half
    // rather than incrementing the count, which is what the queue relies on
    // every time a position laps.
    let wrapped = claim_word(7, advance(POSITION_MASK));
    assert_eq!((reserved_of(wrapped), position_of(wrapped)), (7, 0));
}

// The relationship between the split and the ceiling is deliberately NOT tested
// here. It is a fact about constants, so it lives as a `const` assertion beside
// `BOUNDS` in the parent module, where changing the split without changing the
// ceiling fails to compile. A test would have been the weaker instrument: it can
// only report after the fact, and only on a build somebody chose to run.

#[test]
fn a_capacity_above_this_shapes_ceiling_is_refused_even_though_others_accept_it() {
    // The bound is a property of the shape, which is exactly what D-12 argued
    // and what this shape is the second instance of. `slotwise_mpsc` takes this capacity
    // happily; the packing means this one cannot.
    let error = bounded::<u8>(BOUNDS_MAX * 2).expect_err("beyond the packed position's range");
    assert_eq!(error.max_valid(), BOUNDS_MAX);
    assert_eq!(
        error.previous_valid(),
        Some(BOUNDS_MAX),
        "and the correction offered is this shape's own ceiling"
    );
}

// ---------------------------------------------------------------------------
// The reservation guarantee.
// ---------------------------------------------------------------------------

#[test]
fn a_reserved_slot_is_delivered_into_a_queue_that_is_otherwise_full() {
    // The whole contract in one test: reserve, let the best-effort path fill
    // everything it is allowed to, and redeem anyway.
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    assert!(!tx.is_full(), "an empty queue is not full");
    let slot = tx.reserve().expect("a fresh queue has room");
    assert!(
        !tx.is_full(),
        "nor is one holding a single reservation against four slots"
    );

    let pushed = fill(&tx, 1);
    assert_eq!(pushed, 3, "the reservation withheld exactly one slot");
    // Both directions, and this shape's is the interesting one: `is_full`
    // counts held reservations as occupied, so the queue is full at three
    // items rather than four. An `is_full` stuck at either constant would
    // report that wrongly, and only the positive case was ever asserted.
    assert!(tx.is_full(), "and now nothing more may be pushed");

    slot.send(99).expect("the room was already ours");

    let drained: Vec<u32> = rx.drain().collect();
    assert_eq!(
        drained,
        vec![1, 1, 1, 99],
        "the reserved item lands where it was redeemed, not where it was claimed"
    );
}

#[test]
fn a_reservation_withholds_a_slot_from_the_best_effort_path() {
    let (tx, _rx) = bounded::<u32>(8).expect("8 is a valid capacity");
    assert_eq!(
        fill(&tx, 0),
        8,
        "with nothing reserved, every slot is available"
    );

    let (tx, _rx) = bounded::<u32>(8).expect("8 is a valid capacity");
    let reservations: Vec<_> = (0..3).map(|_| tx.reserve().expect("room")).collect();
    assert_eq!(tx.outstanding_reservations(), 3);
    assert_eq!(
        fill(&tx, 0),
        5,
        "three reserved leaves five for the best-effort path"
    );
    drop(reservations);
}

#[test]
fn dropping_a_reservation_returns_the_slot() {
    let (tx, _rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    let slot = tx.reserve().expect("room");
    assert_eq!(tx.outstanding_reservations(), 1);

    drop(slot);
    assert_eq!(tx.outstanding_reservations(), 0);
    assert_eq!(
        fill(&tx, 0),
        4,
        "a released reservation is capacity given back, not capacity lost"
    );
}

#[test]
fn the_consumer_can_see_that_something_was_promised_even_with_nothing_queued() {
    // The consumer's own `outstanding_reservations`, which is a *second*
    // accessor rather than a view of the producer's -- and one no test reached,
    // so a mutation run found it could return a constant. The distinction it
    // exists to draw is in the name: a drained queue with a reservation
    // outstanding is not an idle one, and a consumer deciding whether to park
    // has to be able to tell the two apart.
    let (tx, rx) = bounded::<u32>(8).expect("8 is a valid capacity");

    assert_eq!(
        rx.outstanding_reservations(),
        0,
        "a fresh queue has promised nothing"
    );

    // Two, not one: a count asserted only at one is satisfied by a method that
    // always answers one.
    let first = tx.reserve().expect("room");
    let second = tx.reserve().expect("room");
    assert_eq!(rx.outstanding_reservations(), 2);
    assert_eq!(
        rx.outstanding_reservations(),
        tx.outstanding_reservations(),
        "the two handles read the same claim word, so they cannot disagree"
    );

    // The state the method is for: nothing to pop, and yet not idle.
    assert!(rx.is_empty(), "nothing has been sent");
    assert_eq!(
        rx.outstanding_reservations(),
        2,
        "an empty queue with two slots promised is waiting, not finished"
    );

    first.send(7).expect("the room was ours");
    assert_eq!(
        rx.outstanding_reservations(),
        1,
        "one redeemed, one still out"
    );
    assert_eq!(rx.pop(), Some(7));
    assert_eq!(
        rx.outstanding_reservations(),
        1,
        "and taking the item does not release the *other* promise"
    );

    drop(second);
    assert_eq!(
        rx.outstanding_reservations(),
        0,
        "a dropped reservation is a promise withdrawn"
    );
}

#[test]
fn a_redeemed_reservation_does_not_also_release_its_slot() {
    // The double-release bug this shape's `send` avoids by consuming `self` and
    // suppressing the drop. If both ran, the count would underflow and the
    // queue would over-admit for ever afterwards.
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    for round in 0..10 {
        let slot = tx.reserve().expect("room");
        assert_eq!(tx.outstanding_reservations(), 1);
        slot.send(round).expect("the room was ours");
        assert_eq!(
            tx.outstanding_reservations(),
            0,
            "redeeming releases the claim exactly once"
        );
        assert_eq!(rx.pop(), Some(round));
    }
    assert_eq!(
        fill(&tx, 0),
        4,
        "and the capacity is intact after ten cycles"
    );
}

#[test]
fn reserving_fails_when_every_slot_is_spoken_for() {
    let (tx, _rx) = bounded::<u32>(2).expect("2 is a valid capacity");
    let _first = tx.reserve().expect("room");
    let _second = tx.reserve().expect("room");
    assert!(
        tx.reserve().is_none(),
        "reservations are drawn from the same capacity as everything else"
    );

    let (tx, _rx) = bounded::<u32>(2).expect("2 is a valid capacity");
    fill(&tx, 0);
    assert!(
        tx.reserve().is_none(),
        "and a full queue has nothing left to promise"
    );
}

#[test]
fn a_full_queue_refuses_a_best_effort_push_and_hands_the_item_back() {
    let (tx, rx) = bounded::<u32>(2).expect("2 is a valid capacity");
    tx.push(1).expect("room");
    tx.push(2).expect("room");

    match tx.push(3) {
        Err(PushError::Full(returned)) => assert_eq!(returned, 3),
        other => panic!("expected Full, got {other:?}"),
    }
    assert_eq!(rx.pop(), Some(1));
    assert_eq!(rx.pop(), Some(2));
}

#[test]
fn a_push_refused_for_a_reservation_is_still_reported_as_full() {
    // A best-effort caller cannot tell "no slots" from "the only slot is
    // reserved", and should not have to: both mean "no room for you", both are
    // backpressure, and both clear when the queue drains.
    let (tx, _rx) = bounded::<u32>(2).expect("2 is a valid capacity");
    let _slot = tx.reserve().expect("room");
    tx.push(1).expect("one slot is unreserved");

    assert!(
        matches!(tx.push(2), Err(PushError::Full(2))),
        "the reserved slot is not available to the best-effort path"
    );
    assert!(!tx.is_empty(), "yet the queue is demonstrably not empty");
}

// ---------------------------------------------------------------------------
// The guarantee under contention, which is the reason the claim word is packed.
// ---------------------------------------------------------------------------

/// How many producer threads the concurrent tests use.
///
/// Fixed rather than derived from the machine's core count, so a failure
/// reproduces on the machine that reported it.
const PRODUCERS: usize = 4;

#[test]
fn every_granted_reservation_is_redeemable_under_contention() {
    // **The test the packed claim word exists to pass.** With the count in its
    // own atomic, a pushing producer and a reserving one can each read before
    // the other's write, and the queue grants a slot that does not exist. That
    // shows up here as a `send` finding no room -- which, because the invariant
    // it violates is checked by a debug assertion in `send`, aborts the test
    // rather than quietly corrupting the ring.
    //
    // A small capacity and many threads, because the race needs the queue to be
    // near-full continuously for the two paths to collide at the boundary.
    const ROUNDS: usize = 2_000;
    let (tx, rx) = bounded::<usize>(4).expect("4 is a valid capacity");

    let threads: Vec<_> = (0..PRODUCERS)
        .map(|producer| {
            let handle = tx.clone();
            thread::spawn(move || {
                let mut granted = 0_usize;
                for round in 0..ROUNDS {
                    // Alternate the two paths so both are contending for the
                    // same last slot rather than taking turns.
                    if round % 2 == 0 {
                        let _ = handle.push(producer);
                    } else if let Some(slot) = handle.reserve() {
                        granted += 1;
                        slot.send(producer).expect("a granted slot is guaranteed");
                    }
                }
                granted
            })
        })
        .collect();
    drop(tx);

    // Drain continuously, so the queue keeps returning to the near-full
    // boundary instead of simply staying full.
    let mut received = 0_usize;
    while let Ok(item) = rx.recv() {
        assert!(item < PRODUCERS, "items must not be torn or invented");
        received += 1;
    }

    let granted: usize = threads
        .into_iter()
        .map(|thread| thread.join().expect("no producer may panic"))
        .sum();

    assert!(
        granted > 0,
        "the run must actually have exercised reservations"
    );
    assert!(
        received >= granted,
        "every reservation that was granted must have been delivered: \
         {granted} granted, only {received} items arrived in total"
    );
}

#[test]
fn a_reservation_holds_capacity_against_every_other_producer() {
    // Not just against the thread that took it. Reserve on one thread, fill
    // from others, and redeem: the slot must have survived their contention.
    let (tx, rx) = bounded::<u32>(8).expect("8 is a valid capacity");
    let slot = tx.reserve().expect("room");

    let threads: Vec<_> = (0..PRODUCERS)
        .map(|_| {
            let handle = tx.clone();
            thread::spawn(move || while handle.push(0).is_ok() {})
        })
        .collect();
    for thread in threads {
        thread.join().expect("no producer may panic");
    }

    assert_eq!(tx.len(), 7, "seven taken, one withheld");
    slot.send(99).expect("the withheld slot is still ours");
    assert_eq!(rx.len(), 8);
}

#[test]
fn a_reservation_can_be_redeemed_from_another_thread() {
    // The shape of the real use case, and the reason this shape's reservation
    // is owned rather than borrowed: claim the slot where the work is
    // submitted, redeem it wherever the completion lands.
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    let slot = tx.reserve().expect("room");
    fill(&tx, 1);

    thread::spawn(move || {
        slot.send(99).expect("the room was claimed before the move");
    })
    .join()
    .expect("the redeeming thread must not panic");

    let drained: Vec<u32> = rx.drain().collect();
    assert_eq!(drained.last(), Some(&99));
}

#[test]
fn a_reservation_is_send_but_not_sync() {
    fn assert_send<T: Send>() {}
    assert_send::<Reservation<u32>>();
    assert_send::<Producer<u32>>();
    assert_send::<Consumer<u32>>();

    // `!Sync` is asserted by the absence of any test that shares one across
    // threads: the compiler refuses to write it.
}

// ---------------------------------------------------------------------------
// Disconnection, which a reservation participates in.
// ---------------------------------------------------------------------------

#[test]
fn an_outstanding_reservation_keeps_the_stream_open() {
    // **A reservation is a promise of a message still to come.** If dropping
    // the last producer ended the stream while one was outstanding, the
    // consumer would be told the queue was finished and then handed an item --
    // losing exactly the message the reservation existed to protect.
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    let slot = tx.reserve().expect("room");
    drop(tx);

    assert!(
        !rx.is_disconnected(),
        "a promise outstanding is a producer outstanding"
    );

    slot.send(7).expect("the consumer is alive");
    assert!(
        rx.is_disconnected(),
        "and redeeming the last one does end the stream"
    );
    assert_eq!(rx.pop(), Some(7), "with the promised item still owed");
}

#[test]
fn dropping_an_outstanding_reservation_also_ends_the_stream() {
    // The other half: a promise abandoned is still a promise resolved, so the
    // consumer must not be left waiting on it for ever.
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    let slot = tx.reserve().expect("room");
    drop(tx);
    assert!(!rx.is_disconnected());

    drop(slot);
    assert!(
        rx.is_disconnected(),
        "an abandoned promise resolves the stream"
    );
}

#[test]
fn a_blocked_consumer_is_woken_by_the_last_reservation_being_redeemed() {
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    let slot = tx.reserve().expect("room");
    drop(tx);

    let sender = thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        slot.send(7).expect("the consumer is alive");
    });

    assert_eq!(
        rx.recv().expect("the reservation is redeemed"),
        7,
        "a parked consumer must be woken by a reserved delivery like any other"
    );
    assert!(matches!(rx.recv(), Err(RecvError::Disconnected)));
    sender.join().expect("the sender must not panic");
}

#[test]
fn a_blocked_consumer_is_woken_by_the_last_reservation_being_dropped() {
    // Caught as a hang if the drop path forgets to release the producer count
    // or to ring the doorbell.
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    let slot = tx.reserve().expect("room");
    drop(tx);

    let abandoner = thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        drop(slot);
    });

    assert!(
        matches!(rx.recv(), Err(RecvError::Disconnected)),
        "abandoning the last promise must wake a parked consumer"
    );
    abandoner
        .join()
        .expect("the abandoning thread must not panic");
}

#[test]
fn redeeming_into_a_departed_consumer_hands_the_item_back() {
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    let slot = tx.reserve().expect("room");
    drop(rx);

    assert!(slot.is_disconnected());
    let error = slot.send(7).expect_err("nobody is left to take it");
    assert_eq!(
        error.into_inner(),
        7,
        "an item important enough to reserve for must not be dropped silently"
    );
}

#[test]
fn an_abandoned_reservation_leaves_the_queue_usable() {
    // A reservation that fails to be redeemed must not poison the capacity.
    let (tx, rx) = bounded::<u32>(2).expect("2 is a valid capacity");
    for _ in 0..50 {
        let slot = tx.reserve().expect("room");
        drop(slot);
    }
    assert_eq!(tx.outstanding_reservations(), 0);
    tx.push(1).expect("room");
    tx.push(2).expect("room");
    assert!(tx.push(3).is_err());
    assert_eq!(rx.pop(), Some(1));
}

// ---------------------------------------------------------------------------
// Queue behaviour, kept honest against the shape it is a variant of.
// ---------------------------------------------------------------------------

#[test]
fn items_come_out_in_the_order_they_went_in() {
    let (tx, rx) = bounded::<u32>(8).expect("a power-of-two capacity");
    for value in 0..8 {
        tx.push(value).expect("room for eight");
    }
    let drained: Vec<u32> = rx.drain().collect();
    assert_eq!(drained, (0..8).collect::<Vec<_>>());
}

#[test]
fn the_ring_wraps_many_times_without_losing_order() {
    // The test that indicts the position arithmetic, and it matters more here
    // than in `slotwise_mpsc`: this shape decides a slot is free from the consumer's
    // position rather than from the slot's own sequence, so an error in the
    // wrapping subtraction is a use-after-free rather than a wrong answer.
    let (tx, rx) = bounded::<usize>(4).expect("a power-of-two capacity");
    for round in 0..2000 {
        tx.push(round).expect("the previous item was taken");
        assert_eq!(rx.pop(), Some(round));
    }
    assert!(rx.is_empty());
}

#[test]
fn a_partly_full_ring_wraps_correctly_with_a_reservation_held_throughout() {
    // Keeps a reservation outstanding across hundreds of laps, so the count
    // must survive every position wrap in the packed word.
    let (tx, rx) = bounded::<usize>(4).expect("a power-of-two capacity");
    let slot = tx.reserve().expect("room");

    for round in 0..500 {
        tx.push(round).expect("three slots remain unreserved");
        assert_eq!(rx.pop(), Some(round));
        assert_eq!(tx.outstanding_reservations(), 1, "round {round}");
    }

    slot.send(99).expect("still ours after five hundred laps");
    assert_eq!(rx.pop(), Some(99));
}

#[test]
fn zero_sized_items_round_trip() {
    let (tx, rx) = bounded::<()>(2).expect("a power-of-two capacity");
    let slot = tx.reserve().expect("room");
    tx.push(()).expect("room");
    assert!(matches!(tx.push(()), Err(PushError::Full(()))));
    slot.send(()).expect("the room was ours");
    assert_eq!(rx.pop(), Some(()));
    assert_eq!(rx.pop(), Some(()));
    assert_eq!(rx.pop(), None);
}

#[test]
fn dropping_the_queue_drops_the_items_it_still_holds() {
    let drops = Arc::new(AtomicUsize::new(0));
    {
        let (tx, _rx) = bounded::<DropCounter>(8).expect("a power-of-two capacity");
        let slot = tx.reserve().expect("room");
        for _ in 0..5 {
            tx.push(DropCounter(Arc::clone(&drops))).expect("room");
        }
        slot.send(DropCounter(Arc::clone(&drops)))
            .expect("the room was ours");
        assert_eq!(drops.load(Ordering::Relaxed), 0, "nothing dropped yet");
    }
    assert_eq!(
        drops.load(Ordering::Relaxed),
        6,
        "every undrained item must be dropped, not leaked -- including the reserved one"
    );
}

#[test]
fn dropping_the_queue_after_a_wrap_drops_only_what_is_resident() {
    let drops = Arc::new(AtomicUsize::new(0));
    {
        let (tx, rx) = bounded::<DropCounter>(4).expect("a power-of-two capacity");
        for _ in 0..6 {
            tx.push(DropCounter(Arc::clone(&drops))).expect("room");
            rx.pop().expect("an item");
        }
        assert_eq!(drops.load(Ordering::Relaxed), 6);
        for _ in 0..3 {
            tx.push(DropCounter(Arc::clone(&drops))).expect("room");
        }
    }
    assert_eq!(
        drops.load(Ordering::Relaxed),
        9,
        "the three still resident must also be dropped"
    );
}

#[test]
fn many_producers_deliver_every_item_exactly_once() {
    const PER_PRODUCER: usize = 500;
    let (tx, rx) = bounded::<(usize, usize)>(16).expect("a valid capacity");

    let threads: Vec<_> = (0..PRODUCERS)
        .map(|producer| {
            let handle = tx.clone();
            thread::spawn(move || {
                for sequence in 0..PER_PRODUCER {
                    let mut item = (producer, sequence);
                    while let Err(PushError::Full(returned)) = handle.push(item) {
                        item = returned;
                        std::hint::spin_loop();
                    }
                }
            })
        })
        .collect();
    drop(tx);

    let mut per_producer = [0_usize; PRODUCERS];
    while let Ok((producer, sequence)) = rx.recv() {
        assert_eq!(
            sequence, per_producer[producer],
            "a producer's own items must arrive in that producer's order"
        );
        per_producer[producer] += 1;
    }
    for thread in threads {
        thread.join().expect("no producer may panic");
    }
    assert!(per_producer.iter().all(|count| *count == PER_PRODUCER));
}

// ---------------------------------------------------------------------------
// The doorbell, which behaves as it does everywhere else.
// ---------------------------------------------------------------------------

#[test]
fn polling_never_creates_a_kernel_object() {
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    let slot = tx.reserve().expect("room");
    slot.send(1).expect("the room was ours");
    while rx.pop().is_some() {}
    drop(tx);
    while rx.pop().is_some() {}

    assert!(
        !rx.shared.doorbell.is_armed(),
        "a poll-only consumer must allocate no kernel object, reservations included"
    );
}

#[test]
fn a_reserved_delivery_lights_the_doorbell() {
    // A reserved send is a delivery like any other, so it must ring. If it did
    // not, a consumer parked on the doorbell would sleep through precisely the
    // message that was important enough to reserve a slot for.
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    rx.doorbell().expect("the doorbell must be creatable");
    let slot = tx.reserve().expect("room");
    assert!(rx.arm().expect("arming must succeed"), "nothing yet");

    slot.send(1).expect("the room was ours");
    assert!(
        !rx.arm().expect("arming must succeed"),
        "a reserved delivery must be visible to the arming protocol"
    );
}

#[test]
fn the_real_arm_finds_an_item_that_lands_inside_its_window() {
    // The same deterministic indictment of the reversed order used by the other
    // shapes, driven through this one's `arm`.
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    rx.doorbell().expect("the doorbell must be creatable");

    let safe_to_wait = race_hooks::ARM.with(
        move || {
            tx.push(1).expect("there is room");
        },
        || rx.arm().expect("arming must succeed"),
    );

    assert!(
        !safe_to_wait,
        "an item landing between the clear and the check must be found, not waited past"
    );
}

#[test]
fn the_real_arm_still_blesses_a_wait_when_its_window_stays_empty() {
    let (_tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    rx.doorbell().expect("the doorbell must be creatable");
    let safe_to_wait = race_hooks::ARM.with(|| {}, || rx.arm().expect("arming must succeed"));
    assert!(safe_to_wait, "nothing arrived, so waiting is right");
}

// ---------------------------------------------------------------------------
// The claim word going stale under the room test, which is the one window where
// "full" can be computed from two readings that never coexisted.
// ---------------------------------------------------------------------------

/// Drains the queue after filling it, so `head` overtakes a claim position read
/// before any of it happened.
///
/// Returned as a closure that fires **once**: the hook sits inside the claim
/// loop, so a closure that acted on every call would move the queue forward
/// again on each retry and the loop could never catch up with it.
fn advance_past(tx: Producer<u32>, rx: Rc<Consumer<u32>>, items: u32) -> impl FnMut() {
    let mut fired = false;
    move || {
        if fired {
            return;
        }
        fired = true;
        for i in 0..items {
            tx.push(i).expect("the queue starts empty, so this fits");
        }
        for _ in 0..items {
            rx.pop().expect("what was just pushed is takeable");
        }
    }
}

#[test]
fn a_push_whose_claim_goes_stale_retries_instead_of_reporting_full() {
    // The defect this guards. `push` reads the claim word, and the room test
    // then reads `head`. If the queue fills and drains in between, `head`
    // passes the position that word carried and `position.wrapping_sub(head)`
    // wraps to near `u32::MAX` -- so an EMPTY queue reports `Full`, and records
    // a refusal for it. The compare-and-swap that would have caught the
    // staleness is never reached, because the room test returns first.
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    let rx = Rc::new(rx);
    let racing = advance_past(tx.clone(), Rc::clone(&rx), 4);

    let outcome = race_hooks::CLAIM.with(racing, || tx.push(99));

    assert!(
        outcome.is_ok(),
        "the queue is empty when the claim is made, so the push must land: {outcome:?}"
    );
    assert_eq!(
        rx.pop(),
        Some(99),
        "the item the retry claimed must actually be in the queue"
    );
    assert_eq!(
        tx.refused(),
        0,
        "a retried claim is not backpressure and must not be counted as one"
    );
}

#[test]
fn a_reservation_whose_claim_goes_stale_retries_instead_of_failing() {
    // `reserve` shares the room test, so it shares the window: the same stale
    // pair made an empty queue refuse a reservation.
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    let rx = Rc::new(rx);
    let racing = advance_past(tx.clone(), Rc::clone(&rx), 4);

    let reservation = race_hooks::CLAIM.with(racing, || tx.reserve());

    let reservation = reservation.expect("the queue is empty when the claim is made");
    reservation.send(7).expect("the consumer is still here");
    assert_eq!(rx.pop(), Some(7));
}

#[test]
fn a_genuinely_full_queue_still_reports_full_through_the_window() {
    // The other direction, so the retry cannot pass by never refusing. Nothing
    // races here, so the claim word the room test rejected is still current and
    // the refusal is authoritative.
    let (tx, rx) = bounded::<u32>(2).expect("2 is a valid capacity");
    tx.push(1).expect("there is room");
    tx.push(2).expect("there is room");

    let outcome = race_hooks::CLAIM.with(|| {}, || tx.push(3));

    assert!(
        matches!(outcome, Err(PushError::Full(3))),
        "a full queue must still refuse, and hand the item back: {outcome:?}"
    );
    assert_eq!(tx.refused(), 1, "a real refusal is still counted");
    assert_eq!(rx.pop(), Some(1));
}
// ---------------------------------------------------------------------------
// Through the traits, which is where this shape and `slotwise_mpsc` visibly differ.
// ---------------------------------------------------------------------------

#[test]
fn the_shape_is_usable_through_the_reserving_trait() {
    fn reserve_and_send<P>(producer: &P, item: P::Item) -> bool
    where
        P: Reserving + Bounded,
        P::Item: Copy,
        for<'a> P::Reservation<'a>: ReservationLike<P::Item>,
    {
        let before = producer.outstanding_reservations();
        let Some(slot) = producer.reserve() else {
            return false;
        };
        assert_eq!(producer.outstanding_reservations(), before + 1);
        slot.deliver(item).is_ok()
    }

    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    assert!(reserve_and_send(&tx, 7));
    assert_eq!(rx.pop(), Some(7));
}

/// The one operation the [`Reserving`] trait deliberately does not name.
///
/// Redeeming consumes the reservation and hands back a shape-specific error, so
/// putting it on the trait would have meant an associated error type carried for
/// the sake of one method. The trait names how a claim is *obtained*, which is
/// the part a generic caller needs; a caller generic over redeeming as well can
/// say so itself, as this does.
trait ReservationLike<T> {
    type Error;
    fn deliver(self, item: T) -> Result<(), Self::Error>;
}

impl<T> ReservationLike<T> for Reservation<T> {
    type Error = crate::Disconnected<T>;

    fn deliver(self, item: T) -> Result<(), Self::Error> {
        self.send(item)
    }
}

impl<T> ReservationLike<T> for crate::spsc::Reservation<'_, T> {
    type Error = crate::Disconnected<T>;

    fn deliver(self, item: T) -> Result<(), Self::Error> {
        self.send(item)
    }
}

#[test]
fn both_reserving_shapes_satisfy_the_trait() {
    // The D-3 check, run for the `Reserving` trait: two implementations that do
    // not resemble each other internally, one handing out a borrowed
    // reservation and one an owned one, reached through the same generic code.
    fn claim_one<P: Reserving>(producer: &P) -> Option<P::Reservation<'_>> {
        producer.reserve()
    }

    let (spsc_tx, _spsc_rx) = crate::spsc::bounded::<u32>(4).expect("4 is valid for both");
    let (mpsc_tx, _mpsc_rx) = bounded::<u32>(4).expect("4 is valid for both");

    assert!(claim_one(&spsc_tx).is_some());
    assert!(claim_one(&mpsc_tx).is_some());
    assert_eq!(
        spsc_tx.outstanding_reservations(),
        0,
        "the claim was dropped"
    );
    assert_eq!(mpsc_tx.outstanding_reservations(), 0);
}

// ---------------------------------------------------------------------------
// Teardown: what becomes of items nobody drained.
//
// The policy itself is covered in `crate::disposal`'s suite. What this shape
// adds is the interaction with reservations, which is where teardown matters
// most: a reservation exists because its message must not be lost, so a
// message redeemed into a queue that is then abandoned would be lost after
// all -- just later, and more quietly.
// ---------------------------------------------------------------------------

/// Records that it was destroyed, so a test can tell "handed to the owner" from
/// "destructor run by whichever thread dropped last".
#[derive(Debug)]
struct Tracked {
    id: u32,
    destroyed: Arc<AtomicUsize>,
}

impl Drop for Tracked {
    fn drop(&mut self) {
        self.destroyed.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn undrained_items_reach_the_disposal_sink_instead_of_being_destroyed() {
    let destroyed = Arc::new(AtomicUsize::new(0));
    let (undelivered, reaper) = std::sync::mpsc::channel();

    {
        let (tx, _rx) = bounded_with::<Tracked>(
            8,
            Options::new().disposal(Disposal::new(move |item| {
                let _ = undelivered.send(item);
            })),
        )
        .expect("8 is a valid capacity");

        for id in 0..5 {
            tx.push(Tracked {
                id,
                destroyed: Arc::clone(&destroyed),
            })
            .expect("room");
        }
    }

    assert_eq!(
        reaper.iter().map(|item| item.id).collect::<Vec<_>>(),
        vec![0, 1, 2, 3, 4]
    );
    assert_eq!(destroyed.load(Ordering::Relaxed), 5);
}

#[test]
fn a_reserved_message_abandoned_at_teardown_is_still_accounted_for() {
    // **The case this shape exists to make safe.** A reservation is taken
    // precisely because the message must not be lost. Redeeming it into a queue
    // that is then torn down undrained would lose it after all, so the sink has
    // to see it like any other survivor.
    let (undelivered, reaper) = std::sync::mpsc::channel();
    {
        let (tx, _rx) = bounded_with::<u32>(
            4,
            Options::new().disposal(Disposal::new(move |item| {
                let _ = undelivered.send(item);
            })),
        )
        .expect("4 is a valid capacity");

        let slot = tx.reserve().expect("room");
        tx.push(1).expect("room");
        slot.send(99).expect("the room was ours");
    }

    assert_eq!(
        reaper.iter().collect::<Vec<_>>(),
        vec![1, 99],
        "a redeemed reservation is an ordinary queued item, and is accounted for as one"
    );
}

#[test]
fn an_unredeemed_reservation_hands_nothing_to_the_sink() {
    // A reservation holds *capacity*, not an item. There is nothing to dispose
    // of, and reporting a phantom would be worse than reporting nothing --
    // the sink is the owner's accounting, and it must not lie in either
    // direction.
    let (undelivered, reaper) = std::sync::mpsc::channel();
    {
        let (tx, _rx) = bounded_with::<u32>(
            4,
            Options::new().disposal(Disposal::new(move |item| {
                let _ = undelivered.send(item);
            })),
        )
        .expect("4 is a valid capacity");

        let _slot = tx.reserve().expect("room");
        tx.push(1).expect("room");
    }

    assert_eq!(
        reaper.iter().collect::<Vec<_>>(),
        vec![1],
        "the abandoned reservation was capacity, not a message"
    );
}

#[test]
fn a_queue_torn_down_by_a_reservation_still_reaches_the_sink() {
    // A reservation counts as a producer, so it can be the last handle
    // standing -- and then its drop is what tears the queue down.
    let (undelivered, reaper) = std::sync::mpsc::channel();
    let (tx, rx) = bounded_with::<u32>(
        4,
        Options::new().disposal(Disposal::new(move |item| {
            let _ = undelivered.send(item);
        })),
    )
    .expect("4 is a valid capacity");

    tx.push(1).expect("room");
    let slot = tx.reserve().expect("room");
    drop(tx);
    drop(rx);
    drop(slot);

    assert_eq!(
        reaper.iter().collect::<Vec<_>>(),
        vec![1],
        "whichever handle releases last must still account for the survivors"
    );
}

#[test]
fn the_sink_sees_survivors_after_the_ring_has_wrapped() {
    let (undelivered, reaper) = std::sync::mpsc::channel();
    {
        let (tx, rx) = bounded_with::<u32>(
            4,
            Options::new().disposal(Disposal::new(move |item| {
                let _ = undelivered.send(item);
            })),
        )
        .expect("4 is a valid capacity");

        for round in 0..6 {
            tx.push(round).expect("room");
            rx.pop().expect("an item");
        }
        for round in 100..103 {
            tx.push(round).expect("room");
        }
    }
    assert_eq!(reaper.iter().collect::<Vec<_>>(), vec![100, 101, 102]);
}

#[test]
fn without_a_sink_undrained_items_are_destroyed_in_place() {
    let destroyed = Arc::new(AtomicUsize::new(0));
    {
        let (tx, _rx) = bounded::<Tracked>(4).expect("4 is a valid capacity");
        for id in 0..3 {
            tx.push(Tracked {
                id,
                destroyed: Arc::clone(&destroyed),
            })
            .expect("room");
        }
    }
    assert_eq!(destroyed.load(Ordering::Relaxed), 3);
}

// ---------------------------------------------------------------------------
// Observability.
// ---------------------------------------------------------------------------

#[test]
fn refusals_are_counted_but_disconnections_are_not() {
    let (tx, rx) = bounded::<u32>(2).expect("2 is a valid capacity");
    tx.push(1).expect("room");
    tx.push(2).expect("room");
    assert!(tx.push(3).is_err());
    assert_eq!(tx.refused(), 1);
    assert_eq!(rx.refused(), 1, "both handles report the same queue");

    drop(rx);
    assert!(matches!(tx.push(4), Err(PushError::Disconnected(4))));
    assert_eq!(
        tx.refused(),
        1,
        "the end of the stream is not backpressure and must not be counted as it"
    );
}

#[test]
fn a_push_refused_because_a_slot_is_reserved_counts_as_a_refusal() {
    // It is backpressure like any other from the caller's side: the queue had
    // no room for *this* push, and the reason is the queue's business rather
    // than the refused producer's.
    let (tx, _rx) = bounded::<u32>(2).expect("2 is a valid capacity");
    let _slot = tx.reserve().expect("room");
    tx.push(1).expect("one slot is unreserved");

    assert!(tx.push(2).is_err());
    assert_eq!(tx.refused(), 1);
}

#[test]
fn high_water_is_untracked_by_default() {
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    tx.push(1).expect("room");
    assert_eq!(tx.high_water(), None);
    assert_eq!(rx.high_water(), None);
}

#[test]
fn high_water_records_the_peak_when_asked_for() {
    let (tx, rx) = bounded_with::<u32>(8, Options::new().tracking_high_water())
        .expect("8 is a valid capacity");
    assert_eq!(tx.high_water(), Some(0));

    for value in 0..5 {
        tx.push(value).expect("room");
    }
    assert_eq!(tx.high_water(), Some(5));

    while rx.pop().is_some() {}
    assert_eq!(rx.high_water(), Some(5));
}

#[test]
fn an_unredeemed_reservation_does_not_count_towards_the_peak() {
    // A reservation holds capacity, not an item. Counting it as depth would
    // report a backlog that does not exist, and the whole point of the mark is
    // to size a queue from evidence.
    let (tx, _rx) = bounded_with::<u32>(8, Options::new().tracking_high_water())
        .expect("8 is a valid capacity");

    let _slot = tx.reserve().expect("room");
    tx.push(1).expect("room");

    assert_eq!(
        tx.high_water(),
        Some(1),
        "one item is one item, whatever else is promised"
    );
}

#[test]
fn the_ring_count_reports_syscalls_rather_than_signal_attempts() {
    let (tx, rx) = bounded::<u32>(8).expect("8 is a valid capacity");
    rx.doorbell().expect("the doorbell must be creatable");

    for value in 0..4 {
        tx.push(value).expect("room");
    }

    assert_eq!(
        rx.doorbell_rings(),
        1,
        "the first push lit it; the other three had nothing to do"
    );
}

#[test]
fn a_reserved_delivery_rings_like_any_other() {
    let (tx, rx) = bounded::<u32>(8).expect("8 is a valid capacity");
    rx.doorbell().expect("the doorbell must be creatable");

    let slot = tx.reserve().expect("room");
    slot.send(1).expect("the room was ours");

    assert_eq!(
        rx.doorbell_rings(),
        1,
        "the message a reservation exists to protect must wake a parked consumer"
    );
}

#[test]
fn the_debug_renderings_name_the_type_and_its_state() {
    // See the same test in the other shapes.
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    tx.push(1).expect("room");

    let producer = format!("{tx:?}");
    assert!(
        producer.contains("reserving_mpsc::Producer"),
        "got {producer}"
    );
    assert!(producer.contains('4'), "the capacity must show: {producer}");

    let consumer = format!("{rx:?}");
    assert!(
        consumer.contains("reserving_mpsc::Consumer"),
        "got {consumer}"
    );

    let reservation = tx.reserve().expect("there is room");
    let rendered = format!("{reservation:?}");
    assert!(
        rendered.contains("reserving_mpsc::Reservation"),
        "got {rendered}"
    );
}

// ---------------------------------------------------------------------------
// The gauges: `len` under a skewed pair of loads, and `remaining` against the
// reservations `len` deliberately excludes.
// ---------------------------------------------------------------------------

#[test]
fn remaining_subtracts_outstanding_reservations() {
    // The defect. `Bounded`'s default is `capacity - len`, and `len` excludes
    // reservations by design, so an empty queue of four holding one reservation
    // answered four -- promising room for a fourth item that `push` is
    // guaranteed to refuse, because the reservation is holding the slot.
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    let slot = tx.reserve().expect("an empty queue has room");

    assert_eq!(tx.len(), 0, "a reservation is not an item");
    assert_eq!(
        tx.remaining(),
        3,
        "one of the four slots is spoken for by the reservation"
    );
    assert_eq!(
        rx.remaining(),
        3,
        "both handles describe the same queue and must agree"
    );

    // And the number is honest: exactly three further pushes fit.
    for i in 0..3 {
        tx.push(i).expect("remaining() said there was room");
    }
    assert_eq!(tx.remaining(), 0);
    assert!(tx.is_full(), "no unreserved slot is left");
    assert!(matches!(tx.push(99), Err(PushError::Full(99))));

    slot.send(7).expect("the consumer is still here");
    assert_eq!(rx.len(), 4, "the redeemed reservation is now an item");
}

#[test]
fn remaining_agrees_through_the_bounded_trait() {
    // The override is on the trait impls, not only the inherent methods: a
    // caller generic over `Bounded` is exactly who would be misled by the
    // default, since it cannot reach `outstanding_reservations` to correct it.
    fn room_through_trait<B: Bounded>(handle: &B) -> usize {
        handle.remaining()
    }

    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    let _slot = tx.reserve().expect("an empty queue has room");

    assert_eq!(room_through_trait(&tx), 3);
    assert_eq!(room_through_trait(&rx), 3);
}

#[test]
fn the_gauges_are_clamped_when_head_has_passed_the_sampled_position() {
    // `len` and `remaining` each read the claim word and then `head`, which are
    // two instants rather than one. If the consumer drains past the position
    // the claim held, `head` overtakes it and `wrapping_sub` yields a number
    // near `u32::MAX` -- a four-slot queue reporting four billion items, and
    // four billion slots of room, straight out of a public metric.
    //
    // The skewed pair is written directly rather than raced for. The CLAIM hook
    // opens the window inside `push`, but by the time `push` returns the two
    // values agree again, so a test that called `len()` afterwards would assert
    // nothing -- which is exactly what an earlier version of this test did, and
    // a sabotage run caught it doing.
    let (tx, _rx) = bounded::<u32>(4).expect("4 is a valid capacity");

    tx.shared.claim.0.store(claim_word(0, 1), Ordering::Release);
    tx.shared.head.0.store(2, Ordering::Release);

    assert_eq!(
        tx.len(),
        tx.capacity(),
        "a bounded queue must never report holding more than it can"
    );
    assert_eq!(
        tx.remaining(),
        0,
        "the clamp must resolve towards full, which is the safe direction"
    );
    assert!(tx.is_full());

    // **Restored before the handles drop, and this is not tidiness.** Teardown
    // walks from `head` to the claim position to dispose whatever is still
    // held, so leaving `head` ahead sets that walk a `u32::MAX`-length loop and
    // the test hangs rather than fails. Measured the hard way.
    tx.shared.head.0.store(0, Ordering::Release);
    tx.shared.claim.0.store(claim_word(0, 0), Ordering::Release);
}

#[test]
fn the_gauges_are_exact_when_the_two_loads_agree() {
    // The guard must not have been bought by clamping everything: an ordinary
    // reading still reports the true count and the true room.
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    tx.push(1).expect("room");
    tx.push(2).expect("room");

    assert_eq!(tx.len(), 2);
    assert_eq!(tx.remaining(), 2);
    assert!(!tx.is_full());
    assert_eq!(rx.pop(), Some(1));
    assert_eq!(tx.len(), 1);
    assert_eq!(tx.remaining(), 3);
}

#[test]
fn publish_waits_for_a_head_that_has_freed_the_slot() {
    // The `send`-path data race, asserted as the guarantee that closes it.
    //
    // Freeing a slot is the consumer's `head.store(Release)`, and on that path
    // it is the *only* release it performs -- so a producer may write the slot
    // only once an acquire load of `head` has actually observed that store. A
    // single acquire load does not give that: it may legally return any earlier
    // value in the modification order, and synchronizes only with the release
    // whose value it in fact reads. `Reservation::send` is the exposed path,
    // because it has no room check and a `Reservation` is `Send`, so the thread
    // redeeming one need never have read `head` at all.
    //
    // A stale read cannot be raced for on a coherent machine, so the state one
    // would observe is written instead: `head` far enough behind the claim
    // position that this slot's previous occupant has not been freed. `publish`
    // must then wait rather than write.
    //
    // **This replaces `the_high_water_mark_never_exceeds_the_capacity`**, which
    // drove the same stale state to prove the high-water *clamp* bounded the
    // over-report. Waiting for a fresh `head` removes the over-report at its
    // source -- after the wait, `position - head < capacity`, so the depth is
    // already bounded and the clamp cannot be reached from here. Asserting the
    // fix is worth more than asserting a mitigation that is now unreachable.
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    let slot = tx.reserve().expect("an empty queue has room");

    // `send` claims position 0, so this leaves `position - head == 11` on a
    // four-slot queue: a view in which the slot is not free.
    tx.shared
        .head
        .0
        .store(POSITION_MASK - 10, Ordering::Release);

    // `Arc<AtomicBool>` rather than a `static`, for the reason `DropCounter`
    // gives: tests share a process, so a module-scope flag would be visible to
    // whichever test ran beside this one.
    let sent = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&sent);
    let sender = thread::spawn(move || {
        slot.send(7).expect("the consumer is still here");
        flag.store(true, Ordering::Release);
    });

    // One-sided on purpose: a slow machine leaves it waiting and the assertion
    // still holds. Only a `publish` that wrongly proceeded can fail it, and that
    // one returns immediately.
    thread::sleep(Duration::from_millis(50));
    assert!(
        !sent.load(Ordering::Acquire),
        "the slot was written while `head` still said its previous occupant was live"
    );

    // Free it, and the wait must end. Also restores a `head` the teardown walk
    // can use: it steps from `head` to the claim position, and a head this far
    // behind would set it a four-billion-step loop that hangs rather than fails.
    tx.shared.head.0.store(0, Ordering::Release);
    sender.join().expect("the sending thread must not panic");
    assert!(
        sent.load(Ordering::Acquire),
        "observing the freeing store must end the wait"
    );

    assert_eq!(rx.pop(), Some(7), "the item itself must be unaffected");
}

#[test]
fn the_high_water_mark_still_reaches_a_genuine_peak() {
    // The clamp must not have been bought by flattening the answer: filling the
    // queue must still be reported as having filled it.
    let (tx, rx) = bounded_with::<u32>(4, Options::new().tracking_high_water())
        .expect("4 is a valid capacity");

    for i in 0..4 {
        tx.push(i).expect("room");
    }

    assert_eq!(
        tx.high_water(),
        Some(4),
        "the queue was filled, so the peak is its capacity"
    );
    assert_eq!(rx.pop(), Some(0));
}

#[test]
fn the_high_water_mark_is_untracked_by_default_on_this_shape() {
    // `None` and `Some(0)` are different answers, and the default is the former.
    let (tx, _rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    tx.push(1).expect("room");
    assert_eq!(tx.high_water(), None);
}
