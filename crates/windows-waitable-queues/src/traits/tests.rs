// Copyright (c) Mike Grier.

//! Tests for the capability traits.
//!
//! # What these are actually for
//!
//! Two things, and an earlier version of this header dismissed the second.
//!
//! The first is the claim [D-3](../../DESIGN-NOTES.md#d-3) makes: that the
//! traits are a real abstraction over more than one implementation, so a caller
//! can be written against them without knowing which shape it has. The evidence
//! is a set of generic functions with no knowledge of any shape, exercised
//! against all of them. If a trait were shaped around one -- the failure D-3
//! exists to prevent -- these would not compile against the others, which is a
//! stronger check than any assertion in the bodies.
//!
//! # The delegating impls are checked here too, and that is not redundant
//!
//! This header used to say testing them "would only assert that a delegating
//! trait impl delegates", and left them alone on that reasoning. A mutation run
//! falsified it: **79 of the 128 surviving mutants in the three shapes were in
//! trait impls**, because every test called the inherent method, which shadows
//! the trait one. `<Producer as Bounded>::len` could return `0` unconditionally
//! and the whole suite stayed green.
//!
//! The impls are hand-written forwarders, so they are a second statement of
//! each shape's contract -- and a second statement is exactly the thing this
//! repository does not leave unchecked. They are also the *only* surface a
//! generic consumer touches, which is the surface D-2 says the crate is for.
//!
//! So the generic helpers below assert against **known queue state** rather
//! than against the inherent methods. Comparing the two views would prove they
//! agree while leaving both free to be wrong together; asserting a queue filled
//! to four reports a length of four fails a forwarder that returns a constant,
//! and fails an inherent method that does, and does not care which one broke.

use crate::{
    Bounded, Consumer, Observable, Options, Producer, PushError, Waitable, reserving_mpsc,
    slotwise_mpsc, spsc,
};

/// Fills a queue through nothing but the [`Producer`] and [`Bounded`] traits,
/// and reports what the refusal said.
///
/// Deliberately generic over two unrelated types with two unrelated internal
/// protocols. Both `where` bounds are load-bearing: this is a caller that needs
/// to push *and* to know the bound, and D-2's whole argument is that it should
/// be able to ask for exactly those two things.
fn fill_to_capacity<P>(producer: &P) -> PushError<u32>
where
    P: Producer<Item = u32> + Bounded,
{
    assert!(producer.is_empty(), "a fresh queue holds nothing");
    assert_eq!(
        producer.remaining(),
        producer.capacity(),
        "and all of its room is available"
    );

    for value in 0..producer.capacity() {
        let value = u32::try_from(value).expect("the test capacities are small");
        producer.push(value).expect("there is room");
    }

    assert_eq!(producer.len(), producer.capacity());
    assert_eq!(
        producer.remaining(),
        0,
        "a full queue has no room, which is what the default method must compute"
    );
    producer
        .push(u32::MAX)
        .expect_err("a full queue must refuse")
}

/// Drains a queue through nothing but the [`Consumer`] trait, using the
/// provided `drain` method rather than a hand-written `while let` loop.
fn drain_all<C>(consumer: &C) -> Vec<u32>
where
    C: Consumer<Item = u32>,
{
    let drained: Vec<u32> = consumer.drain().collect();
    assert!(
        consumer.drain().next().is_none(),
        "draining must leave the queue empty"
    );
    drained
}

/// Parks-or-proceeds through nothing but the [`Waitable`] trait.
///
/// This is the arming protocol as a *consumer* would write it, which is the
/// case D-2 names: a drainer needs `Consumer` and `Waitable` and nothing else,
/// and must not be coupled to reservation or loss reporting to get them.
fn arm_and_report<C>(consumer: &C) -> bool
where
    C: Consumer<Item = u32> + Waitable,
{
    consumer.doorbell().expect("the doorbell must be creatable");
    consumer
        .doorbell_owned()
        .expect("the duplicate must be creatable");
    consumer.arm().expect("arming must succeed")
}

#[test]
fn both_shapes_satisfy_the_producer_and_bounded_traits() {
    let (spsc_tx, spsc_rx) = spsc::bounded::<u32>(4).expect("4 is valid for both shapes");
    let (mpsc_tx, mpsc_rx) = slotwise_mpsc::bounded::<u32>(4).expect("4 is valid for both shapes");

    assert!(
        matches!(fill_to_capacity(&spsc_tx), PushError::Full(u32::MAX)),
        "the generic filler must work against the ring"
    );
    assert!(
        matches!(fill_to_capacity(&mpsc_tx), PushError::Full(u32::MAX)),
        "and against the sequence-protocol queue, unchanged"
    );

    assert_eq!(drain_all(&spsc_rx), vec![0, 1, 2, 3]);
    assert_eq!(drain_all(&mpsc_rx), vec![0, 1, 2, 3]);
}

#[test]
fn both_shapes_report_disconnection_through_the_traits() {
    let (spsc_tx, spsc_rx) = spsc::bounded::<u32>(4).expect("4 is valid for both shapes");
    let (mpsc_tx, mpsc_rx) = slotwise_mpsc::bounded::<u32>(4).expect("4 is valid for both shapes");

    fn producer_sees_it<P: Producer<Item = u32>>(producer: &P) -> bool {
        producer.is_disconnected()
    }
    fn consumer_sees_it<C: Consumer<Item = u32>>(consumer: &C) -> bool {
        consumer.is_disconnected()
    }

    assert!(!producer_sees_it(&spsc_tx));
    assert!(!producer_sees_it(&mpsc_tx));
    assert!(!consumer_sees_it(&spsc_rx));
    assert!(!consumer_sees_it(&mpsc_rx));

    // **The consumer's own view, after the producers go.** Asserting only the
    // `false` before disconnection left a forwarder returning a constant
    // `false` alive on both shapes -- the direction that matters, because a
    // consumer that never learns the stream ended waits forever.
    let (res_tx, res_rx) = reserving_mpsc::bounded::<u32>(4).expect("4 is valid");
    assert!(!consumer_sees_it(&res_rx));

    drop(spsc_tx);
    drop(mpsc_tx);
    drop(res_tx);
    assert!(
        consumer_sees_it(&spsc_rx),
        "the consumer must see the producer go"
    );
    assert!(consumer_sees_it(&mpsc_rx));
    assert!(consumer_sees_it(&res_rx));

    // And the converse direction, on fresh queues so the drops above do not
    // decide the answer. All three shapes, because each producer's
    // `is_disconnected` is its own forwarder.
    let (spsc_tx, spsc_rx) = spsc::bounded::<u32>(4).expect("4 is valid");
    let (mpsc_tx, mpsc_rx) = slotwise_mpsc::bounded::<u32>(4).expect("4 is valid");
    let (res_tx, res_rx) = reserving_mpsc::bounded::<u32>(4).expect("4 is valid");
    assert!(!producer_sees_it(&res_tx), "still connected");

    drop(spsc_rx);
    drop(mpsc_rx);
    drop(res_rx);
    assert!(producer_sees_it(&spsc_tx));
    assert!(
        producer_sees_it(&mpsc_tx),
        "one consumer gone is every consumer gone, for both shapes"
    );
    assert!(producer_sees_it(&res_tx), "and for the reserving shape");
}

#[test]
fn both_reserving_shapes_report_outstanding_claims_through_the_trait() {
    // `spsc` implements `Reserving` too, with a borrowing reservation where
    // `reserving_mpsc` hands out an owned one -- which is the two-implementor
    // evidence D-3 asks for, and was untested until a mutation run said so.
    fn claim_then_release<P>(producer: &P) -> (usize, usize, usize)
    where
        P: crate::Reserving<Item = u32>,
    {
        let before = producer.outstanding_reservations();
        let reservation = producer.reserve().expect("a fresh queue has room");
        let held = producer.outstanding_reservations();
        drop(reservation);
        (before, held, producer.outstanding_reservations())
    }

    let (spsc_tx, _rx) = spsc::bounded::<u32>(4).expect("4 is valid");
    assert_eq!(
        claim_then_release(&spsc_tx),
        (0, 1, 0),
        "the borrowing reservation is counted while it is held"
    );

    let (res_tx, _rx) = reserving_mpsc::bounded::<u32>(4).expect("4 is valid");
    assert_eq!(
        claim_then_release(&res_tx),
        (0, 1, 0),
        "and so is the owned one, through the same trait"
    );
}

#[test]
fn both_shapes_satisfy_the_waitable_trait() {
    let (spsc_tx, spsc_rx) = spsc::bounded::<u32>(4).expect("4 is valid for both shapes");
    let (mpsc_tx, mpsc_rx) = slotwise_mpsc::bounded::<u32>(4).expect("4 is valid for both shapes");

    assert!(arm_and_report(&spsc_rx), "an empty ring is safe to wait on");
    assert!(
        arm_and_report(&mpsc_rx),
        "and so is an empty sequence-protocol queue"
    );

    spsc_tx.push(1).expect("there is room");
    mpsc_tx.push(1).expect("there is room");
    assert!(
        !arm_and_report(&spsc_rx),
        "and neither blesses a wait over an item"
    );
    assert!(!arm_and_report(&mpsc_rx));
}

#[test]
fn the_multi_producer_shape_is_usable_through_the_producer_trait_from_a_clone() {
    // The trait was written against handles that are not `Clone` and handles
    // that are, and `push` taking `&self` is what lets it span both. Had the
    // first shape shipped `push(&mut self)` -- which single-producer soundness
    // would have permitted -- this could not compile.
    let (tx, rx) = slotwise_mpsc::bounded::<u32>(4).expect("4 is a valid capacity");
    let second = tx.clone();

    fn push_one<P: Producer<Item = u32>>(producer: &P, value: u32) {
        producer.push(value).expect("there is room");
    }

    push_one(&tx, 1);
    push_one(&second, 2);
    assert_eq!(drain_all(&rx), vec![1, 2]);
}

#[test]
fn drain_stops_at_the_current_end_rather_than_at_the_end_of_the_stream() {
    // `drain` is the "take everything available" step of the arming protocol,
    // not a way to consume a queue to its end. A caller that read it as the
    // latter would drop items pushed afterwards, so the distinction is asserted
    // rather than left to the documentation.
    let (tx, rx) = slotwise_mpsc::bounded::<u32>(4).expect("4 is a valid capacity");
    tx.push(1).expect("there is room");

    assert_eq!(drain_all(&rx), vec![1]);
    tx.push(2).expect("there is room");
    assert_eq!(
        drain_all(&rx),
        vec![2],
        "the queue was momentarily empty, not finished"
    );
}

/// Every `Bounded` reading, against a queue whose contents are known.
///
/// Deliberately not compared against the inherent methods: see the header. A
/// filled queue of four *is* four items long, whichever implementation is
/// asked, so this fails a forwarder that returns a constant without needing to
/// know that a forwarder exists.
fn bounded_readings_match_known_state<P>(producer: &P, capacity: usize)
where
    P: crate::Producer<Item = u32> + Bounded,
{
    assert_eq!(producer.capacity(), capacity, "capacity as constructed");
    assert_eq!(producer.len(), 0, "a fresh queue is empty");
    assert!(producer.is_empty());
    assert_eq!(producer.remaining(), capacity);

    producer.push(1).expect("there is room");
    assert_eq!(producer.len(), 1, "one push is one item");
    assert!(!producer.is_empty(), "and one item is not empty");
    assert_eq!(producer.remaining(), capacity - 1);
    assert_eq!(
        producer.capacity(),
        capacity,
        "capacity does not move when the contents do"
    );

    for value in 1..capacity {
        producer
            .push(u32::try_from(value).expect("small"))
            .expect("there is room");
    }
    assert_eq!(producer.len(), capacity, "filled to the brim");
    assert_eq!(producer.remaining(), 0);
    assert!(!producer.is_empty());
}

/// Every `Observable` reading, against known state.
///
/// The three counters answer different questions and are asserted separately
/// on purpose: an implementation that returned the same number for all of them
/// would satisfy any test that only checked one had moved.
fn observable_readings_match_known_state<P>(producer: &P, capacity: usize)
where
    P: crate::Producer<Item = u32> + Observable,
{
    assert_eq!(producer.refused(), 0, "nothing has been refused yet");
    assert_eq!(
        producer.doorbell_rings(),
        0,
        "a doorbell nobody asked for has never rung"
    );

    for value in 0..capacity {
        producer
            .push(u32::try_from(value).expect("small"))
            .expect("there is room");
    }
    assert_eq!(
        producer.refused(),
        0,
        "filling a queue exactly refuses nothing"
    );

    producer.push(u32::MAX).expect_err("the queue is full");
    assert_eq!(producer.refused(), 1, "and one refusal is counted");
    producer.push(u32::MAX).expect_err("still full");
    assert_eq!(producer.refused(), 2, "each refusal counts separately");
}

/// The same `Bounded` readings, from the **consumer** handle.
///
/// A separate helper because both handles implement the trait separately, and
/// each impl is its own forwarder: exercising only the producer's left every
/// consumer-side reading unverified, which is exactly what the first pass at
/// this file did and what a second mutation run caught.
fn consumer_bounded_readings_match_known_state<C, P>(consumer: &C, producer: &P, capacity: usize)
where
    C: Consumer<Item = u32> + Bounded,
    P: crate::Producer<Item = u32>,
{
    assert_eq!(consumer.capacity(), capacity, "capacity as constructed");
    assert_eq!(consumer.len(), 0, "a fresh queue is empty");
    assert!(consumer.is_empty());
    assert_eq!(consumer.remaining(), capacity);

    producer.push(1).expect("there is room");
    producer.push(2).expect("there is room");
    assert_eq!(consumer.len(), 2, "the consumer sees what was pushed");
    assert!(!consumer.is_empty());
    assert_eq!(consumer.remaining(), capacity - 2);

    assert_eq!(consumer.pop(), Some(1));
    assert_eq!(consumer.len(), 1, "and sees the depth fall as it drains");
    assert_eq!(consumer.pop(), Some(2));
    assert!(consumer.is_empty(), "drained back to empty");
    assert_eq!(consumer.remaining(), capacity);
}

/// The `Observable` counters from the **consumer** handle, which reports the
/// same shared state the producer does.
fn consumer_observable_readings_match_known_state<C, P>(consumer: &C, producer: &P, capacity: usize)
where
    C: Consumer<Item = u32> + Observable,
    P: crate::Producer<Item = u32>,
{
    assert_eq!(consumer.refused(), 0, "nothing refused yet");
    assert_eq!(
        consumer.doorbell_rings(),
        0,
        "and the doorbell has not rung"
    );

    for value in 0..capacity {
        producer
            .push(u32::try_from(value).expect("small"))
            .expect("there is room");
    }
    producer.push(u32::MAX).expect_err("full");
    assert_eq!(
        consumer.refused(),
        1,
        "a refusal is shared state, visible from either end"
    );
}

/// `high_water` from either end, tracked and untracked.
fn high_water_readings_match_known_state<O: Observable>(subject: &O, expected: Option<usize>) {
    assert_eq!(subject.high_water(), expected);
}

#[test]
fn every_shape_reports_its_bounds_through_the_bounded_trait() {
    // All three, including `reserving_mpsc`, which this file did not mention at
    // all and which carried the largest share of the surviving mutants.
    let (spsc_tx, _spsc_rx) = spsc::bounded::<u32>(4).expect("4 is valid");
    let (slot_tx, _slot_rx) = slotwise_mpsc::bounded::<u32>(4).expect("4 is valid");
    let (res_tx, _res_rx) = reserving_mpsc::bounded::<u32>(4).expect("4 is valid");

    bounded_readings_match_known_state(&spsc_tx, 4);
    bounded_readings_match_known_state(&slot_tx, 4);
    bounded_readings_match_known_state(&res_tx, 4);
}

#[test]
fn every_shape_counts_refusals_through_the_observable_trait() {
    let (spsc_tx, _spsc_rx) = spsc::bounded::<u32>(4).expect("4 is valid");
    let (slot_tx, _slot_rx) = slotwise_mpsc::bounded::<u32>(4).expect("4 is valid");
    let (res_tx, _res_rx) = reserving_mpsc::bounded::<u32>(4).expect("4 is valid");

    observable_readings_match_known_state(&spsc_tx, 4);
    observable_readings_match_known_state(&slot_tx, 4);
    observable_readings_match_known_state(&res_tx, 4);
}

#[test]
fn high_water_distinguishes_untracked_from_a_tracked_zero() {
    // `None` and `Some(0)` are different answers -- nobody counted, against
    // counted and never grew -- and a forwarder returning either constant
    // would satisfy a test that only looked at one configuration.
    fn peak<P: Observable>(producer: &P) -> Option<usize> {
        producer.high_water()
    }

    let (untracked, _rx) = spsc::bounded::<u32>(4).expect("4 is valid");
    assert_eq!(peak(&untracked), None, "tracking is off by default");

    let (tracked, _rx) =
        spsc::bounded_with::<u32>(4, Options::new().tracking_high_water()).expect("4 is valid");
    assert_eq!(peak(&tracked), Some(0), "counted, and never grown");

    tracked.push(1).expect("there is room");
    tracked.push(2).expect("there is room");
    assert_eq!(peak(&tracked), Some(2), "the peak follows the depth up");
}

#[test]
fn the_reserving_shape_is_usable_through_the_reserving_trait() {
    // `Reserving` has one implementor here, so this cannot show the trait spans
    // shapes the way the others do. What it does show is that the trait is
    // usable without naming the concrete type -- and it covers the forwarders,
    // which is where the mutants survived.
    //
    // **Only claiming and releasing are exercised, because that is all the
    // trait offers.** `Reservation<'a>` is declared with no bound, so a caller
    // generic over `Reserving` can obtain a reservation and drop it and nothing
    // else: `commit` is inherent to each shape's own type and is unreachable
    // from here. That is a gap in the trait rather than in this test, and it is
    // raised as such rather than papered over by reaching for the concrete
    // type, which would stop testing the trait at all.
    let (tx, rx) = reserving_mpsc::bounded::<u32>(4).expect("4 is valid");

    fn claim_then_release<P>(producer: &P) -> (usize, usize)
    where
        P: crate::Reserving<Item = u32>,
    {
        let before = producer.outstanding_reservations();
        let reservation = producer.reserve().expect("a fresh queue has room");
        let held = producer.outstanding_reservations();
        drop(reservation);
        (before, held)
    }

    let (before, held) = claim_then_release(&tx);
    assert_eq!(before, 0, "a fresh queue has nothing outstanding");
    assert_eq!(held, 1, "an open reservation is outstanding");
    assert_eq!(
        tx.outstanding_reservations(),
        0,
        "and dropping it returns the slot"
    );

    // The released slot is genuinely usable again, so `outstanding_reservations`
    // reporting zero is not merely a constant that happens to read right.
    tx.push(7).expect("the released slot is available");
    assert_eq!(rx.pop(), Some(7));
}

#[test]
fn every_shape_reports_its_bounds_from_the_consumer_end_too() {
    let (spsc_tx, spsc_rx) = spsc::bounded::<u32>(4).expect("4 is valid");
    let (slot_tx, slot_rx) = slotwise_mpsc::bounded::<u32>(4).expect("4 is valid");
    let (res_tx, res_rx) = reserving_mpsc::bounded::<u32>(4).expect("4 is valid");

    consumer_bounded_readings_match_known_state(&spsc_rx, &spsc_tx, 4);
    consumer_bounded_readings_match_known_state(&slot_rx, &slot_tx, 4);
    consumer_bounded_readings_match_known_state(&res_rx, &res_tx, 4);
}

#[test]
fn every_shape_counts_refusals_from_the_consumer_end_too() {
    let (spsc_tx, spsc_rx) = spsc::bounded::<u32>(4).expect("4 is valid");
    let (slot_tx, slot_rx) = slotwise_mpsc::bounded::<u32>(4).expect("4 is valid");
    let (res_tx, res_rx) = reserving_mpsc::bounded::<u32>(4).expect("4 is valid");

    consumer_observable_readings_match_known_state(&spsc_rx, &spsc_tx, 4);
    consumer_observable_readings_match_known_state(&slot_rx, &slot_tx, 4);
    consumer_observable_readings_match_known_state(&res_rx, &res_tx, 4);
}

#[test]
fn every_shape_reports_high_water_from_either_end() {
    // Both handles and all three shapes, tracked and untracked. `None` and
    // `Some(n)` are different answers, so a forwarder returning either constant
    // has to fail one of these configurations.
    let (spsc_tx, spsc_rx) = spsc::bounded::<u32>(4).expect("4 is valid");
    let (slot_tx, slot_rx) = slotwise_mpsc::bounded::<u32>(4).expect("4 is valid");
    let (res_tx, res_rx) = reserving_mpsc::bounded::<u32>(4).expect("4 is valid");

    high_water_readings_match_known_state(&spsc_tx, None);
    high_water_readings_match_known_state(&spsc_rx, None);
    high_water_readings_match_known_state(&slot_tx, None);
    high_water_readings_match_known_state(&slot_rx, None);
    high_water_readings_match_known_state(&res_tx, None);
    high_water_readings_match_known_state(&res_rx, None);

    // And with tracking on, the peak is visible from both ends and follows the
    // depth up rather than sitting at a constant.
    let options = || Options::new().tracking_high_water();
    let (tx, rx) = spsc::bounded_with::<u32>(4, options()).expect("4 is valid");
    let (stx, srx) = slotwise_mpsc::bounded_with::<u32>(4, options()).expect("4 is valid");
    let (rtx, rrx) = reserving_mpsc::bounded_with::<u32>(4, options()).expect("4 is valid");

    high_water_readings_match_known_state(&tx, Some(0));
    high_water_readings_match_known_state(&rx, Some(0));
    high_water_readings_match_known_state(&stx, Some(0));
    high_water_readings_match_known_state(&srx, Some(0));
    high_water_readings_match_known_state(&rtx, Some(0));
    high_water_readings_match_known_state(&rrx, Some(0));

    for value in 0..3 {
        tx.push(value).expect("there is room");
    }
    high_water_readings_match_known_state(&tx, Some(3));
    high_water_readings_match_known_state(&rx, Some(3));

    for value in 0..3 {
        stx.push(value).expect("there is room");
        rtx.push(value).expect("there is room");
    }
    high_water_readings_match_known_state(&stx, Some(3));
    high_water_readings_match_known_state(&srx, Some(3));
    high_water_readings_match_known_state(&rtx, Some(3));
    high_water_readings_match_known_state(&rrx, Some(3));
}

#[test]
fn every_shape_counts_a_doorbell_ring_that_actually_happened() {
    // `doorbell_rings` counts real `SetEvent` calls, so it stays zero until a
    // consumer has armed and a producer has pushed against that armed state.
    // Asserting only the zero -- which the refusal tests above do -- leaves a
    // forwarder returning a constant zero alive.
    fn rings<O: Observable>(subject: &O) -> u64 {
        subject.doorbell_rings()
    }

    fn ring_once<P, C>(producer: &P, consumer: &C)
    where
        P: crate::Producer<Item = u32>,
        C: Consumer<Item = u32> + Waitable + Observable,
    {
        assert_eq!(rings(consumer), 0, "nothing has rung yet");
        assert!(
            consumer.arm().expect("arming must succeed"),
            "empty, so safe"
        );
        producer.push(1).expect("there is room");
        assert!(
            rings(consumer) >= 1,
            "a push against an armed doorbell must ring it"
        );
    }

    let (spsc_tx, spsc_rx) = spsc::bounded::<u32>(4).expect("4 is valid");
    let (slot_tx, slot_rx) = slotwise_mpsc::bounded::<u32>(4).expect("4 is valid");
    let (res_tx, res_rx) = reserving_mpsc::bounded::<u32>(4).expect("4 is valid");

    ring_once(&spsc_tx, &spsc_rx);
    ring_once(&slot_tx, &slot_rx);
    ring_once(&res_tx, &res_rx);

    // Visible from the producer end as well, which is its own forwarder.
    assert!(rings(&spsc_tx) >= 1);
    assert!(rings(&slot_tx) >= 1);
    assert!(rings(&res_tx) >= 1);
}
