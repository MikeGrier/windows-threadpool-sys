// Copyright (c) Mike Grier.

//! Tests for the capability traits.
//!
//! # What these are actually for
//!
//! Not to re-test the shapes -- each shape's own suite does that, through its
//! inherent methods, and repeating it here would only assert that a delegating
//! trait impl delegates. What is tested here is the claim
//! [D-3](../../DESIGN-NOTES.md#d-3) makes: that the traits are a real
//! abstraction over more than one implementation, so that a caller can be
//! written against them without knowing which shape it has.
//!
//! The evidence is a set of generic functions with no knowledge of either
//! shape, exercised against both. If a trait were shaped around one of them --
//! the failure D-3 exists to prevent -- these would not compile against the
//! other, which is a stronger check than any assertion in the bodies.

use crate::{Bounded, Consumer, Producer, PushError, Waitable, slotwise_mpsc, spsc};

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

    drop(spsc_rx);
    drop(mpsc_rx);
    assert!(producer_sees_it(&spsc_tx));
    assert!(
        producer_sees_it(&mpsc_tx),
        "one consumer gone is every consumer gone, for both shapes"
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
