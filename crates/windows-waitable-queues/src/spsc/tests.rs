// Copyright (c) Mike Grier.

//! Tests for the SPSC bounded ring.
//!
//! Every one runs in memory in microseconds. The cross-thread cases use a
//! joined thread rather than a sleep, so they are deterministic: the assertion
//! runs after the peer has finished, not after a guess about how long it takes.

use super::{Producer, bounded};
use crate::PushError;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

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

#[test]
fn a_pushed_item_comes_back_out() {
    let (tx, rx) = bounded::<u32>(4).expect("a power-of-two capacity");
    tx.push(42).expect("a fresh queue has room");
    assert_eq!(rx.pop(), Some(42));
}

#[test]
fn an_empty_queue_pops_nothing() {
    let (_tx, rx) = bounded::<u32>(4).expect("a power-of-two capacity");
    assert_eq!(rx.pop(), None);
    assert!(rx.is_empty());
    assert_eq!(rx.len(), 0);
}

#[test]
fn items_come_out_in_the_order_they_went_in() {
    let (tx, rx) = bounded::<u32>(8).expect("a power-of-two capacity");
    for value in 0..8 {
        tx.push(value).expect("room for eight");
    }
    let drained: Vec<u32> = std::iter::from_fn(|| rx.pop()).collect();
    assert_eq!(drained, (0..8).collect::<Vec<_>>());
}

#[test]
fn a_full_queue_refuses_and_hands_the_item_back() {
    let (tx, rx) = bounded::<u32>(2).expect("a power-of-two capacity");
    tx.push(1).expect("room");
    tx.push(2).expect("room");
    assert!(tx.is_full());

    match tx.push(3) {
        Err(PushError::Full(returned)) => assert_eq!(
            returned, 3,
            "the refused item must come back, or a caller cannot retry it"
        ),
        other => panic!("expected Full, got {other:?}"),
    }

    // And the refusal did not disturb what was already there.
    assert_eq!(rx.pop(), Some(1));
    assert_eq!(rx.pop(), Some(2));
}

#[test]
fn a_capacity_of_one_holds_exactly_one() {
    let (tx, rx) = bounded::<u32>(1).expect("one is a power of two");
    tx.push(1).expect("room for one");
    assert!(matches!(tx.push(2), Err(PushError::Full(2))));
    assert_eq!(rx.pop(), Some(1));
    tx.push(3).expect("the slot was freed");
    assert_eq!(rx.pop(), Some(3));
}

#[test]
fn the_ring_wraps_many_times_without_losing_order() {
    // Far more operations than slots, so every slot is reused repeatedly and a
    // mistake in the masking or in the free-slot arithmetic shows up as a
    // wrong value rather than as a crash.
    let (tx, rx) = bounded::<usize>(4).expect("a power-of-two capacity");
    for round in 0..1000 {
        tx.push(round).expect("the previous item was taken");
        assert_eq!(rx.pop(), Some(round));
    }
    assert!(rx.is_empty());
}

#[test]
fn a_partly_full_ring_wraps_correctly() {
    // Keeps two items resident while cycling, so head and tail are never equal
    // and never a whole lap apart -- the case a simple "empty when equal" test
    // never reaches.
    let (tx, rx) = bounded::<usize>(4).expect("a power-of-two capacity");
    tx.push(0).expect("room");
    tx.push(1).expect("room");
    for round in 2..500 {
        tx.push(round)
            .expect("room, because one is taken each round");
        assert_eq!(rx.pop(), Some(round - 2));
        assert_eq!(rx.len(), 2);
    }
}

#[test]
fn len_tracks_pushes_and_pops() {
    let (tx, rx) = bounded::<u32>(4).expect("a power-of-two capacity");
    assert_eq!(tx.len(), 0);
    tx.push(1).expect("room");
    assert_eq!(tx.len(), 1);
    assert_eq!(rx.len(), 1, "both handles report the same queue");
    tx.push(2).expect("room");
    assert_eq!(tx.len(), 2);
    rx.pop().expect("an item");
    assert_eq!(rx.len(), 1);
    rx.pop().expect("an item");
    assert!(rx.is_empty());
}

#[test]
fn dropping_the_queue_drops_the_items_it_still_holds() {
    let drops = Arc::new(AtomicUsize::new(0));
    {
        let (tx, _rx) = bounded::<DropCounter>(8).expect("a power-of-two capacity");
        for _ in 0..5 {
            tx.push(DropCounter(Arc::clone(&drops))).expect("room");
        }
        assert_eq!(drops.load(Ordering::Relaxed), 0, "nothing dropped yet");
    }
    assert_eq!(
        drops.load(Ordering::Relaxed),
        5,
        "every undrained item must be dropped, not leaked"
    );
}

#[test]
fn dropping_the_queue_after_a_wrap_drops_only_what_is_resident() {
    // The interesting case for the drop loop: head and tail are both far from
    // zero and the live range straddles the end of the slot array, so a drop
    // that iterated `0..len` instead of `head..tail` would destroy the wrong
    // slots -- and would drop uninitialized memory.
    let drops = Arc::new(AtomicUsize::new(0));
    {
        let (tx, rx) = bounded::<DropCounter>(4).expect("a power-of-two capacity");
        for _ in 0..6 {
            tx.push(DropCounter(Arc::clone(&drops))).expect("room");
            rx.pop().expect("an item");
        }
        assert_eq!(
            drops.load(Ordering::Relaxed),
            6,
            "the six taken were dropped"
        );

        // Now leave three resident, starting from a wrapped position.
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
fn a_consumer_that_is_gone_turns_a_push_into_a_disconnect() {
    let (tx, rx) = bounded::<u32>(4).expect("a power-of-two capacity");
    assert!(!tx.is_disconnected());
    drop(rx);
    assert!(tx.is_disconnected());

    match tx.push(1) {
        Err(PushError::Disconnected(returned)) => assert_eq!(returned, 1),
        other => panic!("expected Disconnected, got {other:?}"),
    }
}

#[test]
fn a_full_queue_whose_consumer_is_gone_reports_disconnected_not_full() {
    // The distinction is the whole point of having two variants: Full invites a
    // retry, and retrying this one would spin for ever.
    let (tx, rx) = bounded::<u32>(2).expect("a power-of-two capacity");
    tx.push(1).expect("room");
    tx.push(2).expect("room");
    drop(rx);

    match tx.push(3) {
        Err(PushError::Disconnected(_)) => {}
        Err(PushError::Full(_)) => {
            panic!("a full queue with no consumer will never drain, so Full would invite a spin")
        }
        Ok(()) => panic!("the queue was full"),
    }
}

#[test]
fn a_producer_that_is_gone_leaves_the_queued_items_takeable() {
    // Disconnection must not discard what was already pushed, which is why the
    // documented order is drain first and check afterwards.
    let (tx, rx) = bounded::<u32>(4).expect("a power-of-two capacity");
    tx.push(1).expect("room");
    tx.push(2).expect("room");
    drop(tx);

    assert!(rx.is_disconnected());
    assert_eq!(rx.pop(), Some(1), "a dropped producer does not discard");
    assert_eq!(rx.pop(), Some(2));
    assert_eq!(rx.pop(), None);
}

#[test]
fn a_zero_capacity_is_refused_because_it_could_never_accept_anything() {
    let error = bounded::<u32>(0).expect_err("zero is not a usable capacity");
    assert_eq!(error.requested(), 0);
    assert_eq!(error.next_valid(), Some(1));
}

#[test]
fn a_non_power_of_two_capacity_is_refused_with_both_neighbours() {
    let error = bounded::<u32>(100).expect_err("100 is not a power of two");
    assert_eq!(error.requested(), 100);
    assert_eq!(
        (error.previous_valid(), error.next_valid()),
        (Some(64), Some(128)),
        "the error should make the correction obvious without arithmetic"
    );
}

#[test]
fn every_power_of_two_capacity_up_to_a_reasonable_bound_is_accepted() {
    for shift in 0..16 {
        let capacity = 1_usize << shift;
        let (tx, rx) = bounded::<usize>(capacity).expect("a power of two");
        assert_eq!(tx.capacity(), capacity);
        assert_eq!(rx.capacity(), capacity, "both handles agree");
        tx.push(shift).expect("a fresh queue has room");
        assert_eq!(rx.pop(), Some(shift));
    }
}

#[test]
fn a_capacity_above_half_the_address_space_is_refused() {
    // Not because the allocation would fail first, but because the position
    // arithmetic would become ambiguous across wraparound. Checked explicitly
    // so the reason survives even though no machine could allocate it.
    let error = bounded::<u8>(1_usize << (usize::BITS - 1)).expect_err("too large");
    assert!(
        error.next_valid().is_none(),
        "there is nothing larger to suggest"
    );
}

#[test]
fn zero_sized_items_round_trip() {
    // A ZST exercises the slot arithmetic with no bytes to copy, so a mistake
    // cannot hide behind a memcpy that happens to do the right thing.
    let (tx, rx) = bounded::<()>(2).expect("a power-of-two capacity");
    tx.push(()).expect("room");
    tx.push(()).expect("room");
    assert!(matches!(tx.push(()), Err(PushError::Full(()))));
    assert_eq!(rx.pop(), Some(()));
    assert_eq!(rx.pop(), Some(()));
    assert_eq!(rx.pop(), None);
}

#[test]
fn items_cross_a_thread_boundary_in_order_and_intact() {
    // The real test of the memory ordering. Each item carries a value derived
    // from its index, so a torn or stale read is a wrong value rather than a
    // silent pass. Boxed, so each item is a heap pointer the consumer must
    // observe fully initialized -- a missing release would surface as a
    // corrupt pointer rather than as a wrong integer.
    const COUNT: usize = 20_000;
    let (tx, rx) = bounded::<Box<usize>>(64).expect("a power-of-two capacity");

    let producer = std::thread::spawn(move || {
        for value in 0..COUNT {
            // Spin rather than sleep: the consumer is draining concurrently,
            // so a full queue clears in nanoseconds.
            let mut item = Box::new(value);
            loop {
                match tx.push(item) {
                    Ok(()) => break,
                    Err(PushError::Full(returned)) => {
                        item = returned;
                        std::hint::spin_loop();
                    }
                    Err(PushError::Disconnected(_)) => panic!("the consumer is alive"),
                }
            }
        }
    });

    let mut received = 0_usize;
    while received < COUNT {
        if let Some(item) = rx.pop() {
            assert_eq!(*item, received, "items must arrive in order and intact");
            received += 1;
        } else {
            std::hint::spin_loop();
        }
    }

    producer.join().expect("the producer thread");
    assert_eq!(rx.pop(), None);
}

#[test]
fn a_producer_can_be_moved_to_another_thread() {
    // `Send` is the property that makes the split useful, and it is worth
    // pinning: a handle that could not move would force construction on the
    // thread that ends up owning it.
    fn assert_send<T: Send>() {}
    assert_send::<Producer<u32>>();
    assert_send::<super::Consumer<u32>>();

    let (tx, rx) = bounded::<u32>(4).expect("a power-of-two capacity");
    std::thread::spawn(move || {
        tx.push(7).expect("room");
    })
    .join()
    .expect("the pushing thread");
    assert_eq!(rx.pop(), Some(7));
}
