// Copyright (c) Mike Grier.

//! Tests for the SPSC bounded ring.
//!
//! Every one runs in memory in microseconds. The cross-thread cases use a
//! joined thread rather than a sleep, so they are deterministic: the assertion
//! runs after the peer has finished, not after a guess about how long it takes.

use super::{Consumer, MIN_CAPACITY, Producer, bounded, validate_capacity};
use crate::race_hooks;
use crate::{PushError, RecvError, RecvTimeoutError};
use std::os::windows::io::AsRawHandle;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::WaitForSingleObject;

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

// ---------------------------------------------------------------------------
// The doorbell, joined to the queue.
//
// The tests below are about the *pairing* of the two; the doorbell's own
// behaviour as a kernel object is covered in `crate::doorbell`'s suite.
// ---------------------------------------------------------------------------

/// Whether the queue's doorbell is signalled right now, asked of the kernel
/// rather than of the mirror flag.
///
/// Uses a zero timeout, so it is a state query and never blocks. The event is
/// manual-reset, so asking does not consume the answer.
fn doorbell_is_lit<T>(consumer: &Consumer<T>) -> bool {
    let handle = consumer.doorbell().expect("the doorbell must be creatable");
    // SAFETY: a live event handle borrowed for the call; a zero timeout returns
    // immediately and has no other precondition.
    let result = unsafe { WaitForSingleObject(handle.as_raw_handle(), 0) };
    assert!(
        result == WAIT_OBJECT_0 || result == WAIT_TIMEOUT,
        "the wait must resolve to signalled or not, got {result:#x}"
    );
    result == WAIT_OBJECT_0
}

#[test]
fn polling_never_creates_a_kernel_object() {
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");

    // The laziness claim, asserted rather than assumed: a consumer that only
    // ever polls must not be charged for an event it never waits on.
    for value in 0..4 {
        tx.push(value).expect("there is room");
    }
    while rx.pop().is_some() {}
    drop(tx);
    while rx.pop().is_some() {}

    assert!(
        !rx.shared.doorbell.is_armed(),
        "a poll-only consumer must allocate no kernel object"
    );
}

#[test]
fn a_push_lights_the_doorbell() {
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    assert!(
        !doorbell_is_lit(&rx),
        "an empty queue must not claim readiness"
    );
    tx.push(1).expect("there is room");
    assert!(doorbell_is_lit(&rx), "a pushed item must be announced");
}

#[test]
fn the_doorbell_stays_lit_across_repeated_observation() {
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    // Created before the push, which is not incidental: a push that runs while
    // no event exists signals nothing, as
    // `an_item_pushed_before_the_doorbell_existed_is_still_found` asserts. This
    // test is about the level, so it starts from an armed doorbell.
    rx.doorbell().expect("the doorbell must be creatable");
    tx.push(1).expect("there is room");

    // A level, not an edge. An auto-reset event would fail the second pass, and
    // a consumer sharing the wait with other handles would lose the queue.
    for observation in 1..=3 {
        assert!(
            doorbell_is_lit(&rx),
            "observation {observation} must still see the level"
        );
    }
}

#[test]
fn arm_reports_unsafe_to_wait_while_items_remain() {
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    tx.push(1).expect("there is room");

    assert!(
        !rx.arm().expect("arming must succeed"),
        "arming must refuse to bless a wait while an item is sitting there"
    );
}

#[test]
fn arm_reports_safe_to_wait_when_empty() {
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    // Created before the push, and that is the whole test. Without it the push
    // takes `signal`'s "no event yet" path, the doorbell is never lit, and the
    // assertion below that arming CLEARS it holds trivially -- it passed with
    // `clear`'s `ResetEvent` deleted, which is how this was found.
    rx.doorbell().expect("the doorbell must be creatable");
    tx.push(1).expect("there is room");
    assert!(
        doorbell_is_lit(&rx),
        "the doorbell must be lit before a test of clearing it can mean anything"
    );
    assert_eq!(rx.pop(), Some(1));

    assert!(
        rx.arm().expect("arming must succeed"),
        "a drained queue is safe to wait on"
    );
    assert!(
        !doorbell_is_lit(&rx),
        "arming must clear the doorbell, or the next wait returns at once forever"
    );
}

#[test]
fn arm_relights_the_doorbell_for_a_later_push() {
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    // As above: the first push must actually SET the mirror flag, or the claim
    // that `clear` cleared it is a claim about a flag that was never set.
    rx.doorbell().expect("the doorbell must be creatable");
    tx.push(1).expect("there is room");
    assert!(doorbell_is_lit(&rx), "the first push must light it");
    assert_eq!(rx.pop(), Some(1));
    assert!(rx.arm().expect("arming must succeed"));

    // The signal that must never be skipped: the doorbell was cleared, so the
    // producer's mirror flag has to have been cleared with it.
    tx.push(2).expect("there is room");
    assert!(
        doorbell_is_lit(&rx),
        "the first push after a clear must light the doorbell again"
    );
}

#[test]
fn an_item_pushed_before_the_doorbell_existed_is_still_found() {
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");

    // The lazy-creation hole. This push signals nothing, because there is no
    // event yet to signal -- `crate::doorbell`'s suite asserts that directly.
    tx.push(1).expect("there is room");
    assert!(!rx.shared.doorbell.is_armed(), "no event exists yet");

    // Arming creates the event and only then checks emptiness, so the item is
    // found instead of waited on. Had the check come first, this would report
    // "safe to wait" and the consumer would block on an item already queued.
    assert!(
        !rx.arm().expect("arming must succeed"),
        "arming must not bless a wait over an item that predates the doorbell"
    );
}

// ---------------------------------------------------------------------------
// The lost-wakeup guard, verified by sabotage.
//
// `Consumer::arm` clears the doorbell and *then* checks emptiness. The reverse
// reads more naturally and is wrong. These two tests build the identical race
// against each order and assert that one produces a hang and the other does
// not, which is the only evidence that the order in `arm` is load-bearing
// rather than incidental.
//
// The race is driven deterministically on one thread rather than raced for on
// two: an interleaving that must be hit to prove a point is not one to leave to
// the scheduler.
// ---------------------------------------------------------------------------

/// The sabotage: emptiness observed *before* the doorbell is cleared.
///
/// Deliberately wrong, and called by nothing but the test that indicts it.
/// Mirrors [`Consumer::arm`] in every other respect, so the only difference
/// under test is the order of the two statements in the middle.
///
/// `racing` runs in the window the wrong order opens -- between the emptiness
/// check and the clear. Passing it in makes the interleaving deterministic
/// rather than something two threads have to be lucky to produce.
fn arm_reversed_racing<T>(consumer: &Consumer<T>, racing: impl FnOnce()) -> bool {
    consumer
        .shared
        .doorbell
        .handle()
        .expect("the doorbell must be creatable");
    let empty = consumer.is_empty();
    racing();
    consumer.shared.doorbell.clear();
    empty
}

#[test]
fn reversing_the_clear_and_the_check_strands_an_item() {
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    rx.doorbell().expect("the doorbell must be creatable");

    // Step 1 of the protocol: the consumer drains and sees nothing.
    assert_eq!(rx.pop(), None);

    // The producer lands inside the window the wrong order opens: after the
    // reversed check has already read `is_empty` as true, but before its clear
    // erases the signal that push is about to raise. Driving it through
    // `arm_reversed` keeps the sabotage a single definition rather than a
    // paraphrase that could drift from the thing it is meant to indict.
    let empty_before = arm_reversed_racing(&rx, || {
        tx.push(1).expect("there is room");
    });

    assert!(
        empty_before,
        "the reversed check saw an empty queue and would bless a wait"
    );
    assert_eq!(rx.len(), 1, "yet the queue holds an item");
    assert!(
        !doorbell_is_lit(&rx),
        "and the doorbell is dark, so nothing will ever wake a waiter"
    );

    // Proof that this state really is a hang and not merely suspicious: a real
    // wait against it times out. A generous 250 ms, because the assertion is
    // "this never fires", not "this is slow".
    let handle = rx.doorbell().expect("the doorbell must be creatable");
    // SAFETY: a live event handle borrowed for the call.
    let result = unsafe { WaitForSingleObject(handle.as_raw_handle(), 250) };
    assert_eq!(
        result, WAIT_TIMEOUT,
        "a consumer that checked before clearing waits forever on a queue that \
         is not empty -- this is the lost wakeup, reproduced"
    );
}

#[test]
fn clearing_before_the_check_finds_the_item_instead_of_waiting() {
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    rx.doorbell().expect("the doorbell must be creatable");
    assert_eq!(rx.pop(), None);

    // The identical race, against the correct order. `arm` clears first, so the
    // producer's push either lands before the clear and is caught by the check,
    // or lands after it and lights a doorbell nobody is about to reset.
    tx.push(1).expect("there is room");
    let safe_to_wait = rx.arm().expect("arming must succeed");

    assert!(
        !safe_to_wait,
        "the check after the clear must see the item and refuse the wait"
    );
    assert_eq!(rx.len(), 1, "the item is still there to be taken");
    assert_eq!(rx.pop(), Some(1), "and taking it is what happens instead");
}

#[test]
fn a_push_after_arming_lights_a_doorbell_that_stays_lit() {
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    assert!(
        rx.arm().expect("arming must succeed"),
        "empty, so safe to wait"
    );

    // The other half of the window: a push landing after the clear. Nothing
    // resets the doorbell between the signal and the wait, so the wait returns
    // at once rather than blocking.
    tx.push(1).expect("there is room");

    let handle = rx.doorbell().expect("the doorbell must be creatable");
    // SAFETY: a live event handle borrowed for the call.
    let result = unsafe { WaitForSingleObject(handle.as_raw_handle(), 250) };
    assert_eq!(
        result, WAIT_OBJECT_0,
        "a push after arming must wake a waiter immediately"
    );
}

// ---------------------------------------------------------------------------
// Blocking receive.
// ---------------------------------------------------------------------------

#[test]
fn recv_returns_an_item_already_queued() {
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    tx.push(7).expect("there is room");
    assert_eq!(rx.recv().expect("an item is queued"), 7);
}

#[test]
fn recv_blocks_until_a_push_arrives() {
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    let producer = thread::spawn(move || {
        // A short sleep so the consumer is genuinely parked rather than racing
        // to the first `pop`. Correctness does not depend on winning that race
        // -- it depends on the wakeup arriving either way.
        thread::sleep(Duration::from_millis(50));
        tx.push(99).expect("there is room");
    });

    assert_eq!(rx.recv().expect("the producer pushes"), 99);
    producer.join().expect("the producer must not panic");
}

#[test]
fn recv_reports_disconnection_once_drained() {
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    drop(tx);
    assert!(
        matches!(rx.recv(), Err(RecvError::Disconnected)),
        "an empty queue with no producer is finished"
    );
}

#[test]
fn recv_delivers_items_pushed_before_the_producer_dropped() {
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    tx.push(1).expect("there is room");
    tx.push(2).expect("there is room");
    drop(tx);

    // Disconnection must not discard what was already sent. Testing the flag
    // before draining is the mistake this guards.
    assert_eq!(rx.recv().expect("item one is owed"), 1);
    assert_eq!(rx.recv().expect("item two is owed"), 2);
    assert!(
        matches!(rx.recv(), Err(RecvError::Disconnected)),
        "and only then is it finished"
    );
}

#[test]
fn a_blocked_recv_is_released_by_the_producer_dropping() {
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    let producer = thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        drop(tx);
    });

    // Without a signal in the producer's `Drop` this hangs forever: the queue
    // would be correct and the program would still be wedged.
    assert!(
        matches!(rx.recv(), Err(RecvError::Disconnected)),
        "dropping the producer must wake a parked consumer"
    );
    producer.join().expect("the producer must not panic");
}

#[test]
fn recv_timeout_gives_up_on_an_empty_live_queue() {
    let (_tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    let started = Instant::now();
    let result = rx.recv_timeout(Duration::from_millis(60));

    assert!(
        matches!(result, Err(RecvTimeoutError::Timeout)),
        "an empty queue with a live producer times out rather than ending"
    );
    assert!(
        result.is_err_and(|error| error.is_retryable()),
        "and a timeout is worth retrying, unlike the other two variants"
    );
    assert!(
        started.elapsed() >= Duration::from_millis(50),
        "it must actually have waited rather than returned at once"
    );
}

#[test]
fn recv_timeout_returns_an_item_that_arrives_in_time() {
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    let producer = thread::spawn(move || {
        thread::sleep(Duration::from_millis(30));
        tx.push(5).expect("there is room");
    });

    assert_eq!(
        rx.recv_timeout(Duration::from_secs(5))
            .expect("the push lands well inside the deadline"),
        5
    );
    producer.join().expect("the producer must not panic");
}

#[test]
fn recv_timeout_reports_disconnection_rather_than_waiting_out_the_clock() {
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    drop(tx);

    let started = Instant::now();
    let result = rx.recv_timeout(Duration::from_secs(30));

    assert!(
        matches!(result, Err(RecvTimeoutError::Disconnected)),
        "a finished queue is finished, deadline or not"
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "and it must be reported at once rather than after the deadline"
    );
}

#[test]
fn a_blocking_consumer_receives_every_item_in_order() {
    // The whole mechanism under load: a capacity far smaller than the run, so
    // the producer blocks on a full queue and the consumer blocks on an empty
    // one, repeatedly, in both directions.
    const COUNT: u32 = 10_000;
    let (tx, rx) = bounded::<u32>(16).expect("16 is a valid capacity");

    let producer = thread::spawn(move || {
        for value in 0..COUNT {
            let mut item = value;
            while let Err(PushError::Full(returned)) = tx.push(item) {
                item = returned;
                std::hint::spin_loop();
            }
        }
    });

    for expected in 0..COUNT {
        assert_eq!(
            rx.recv().expect("the producer is still sending"),
            expected,
            "items must arrive exactly once and in order"
        );
    }
    producer.join().expect("the producer must not panic");

    assert!(
        matches!(rx.recv(), Err(RecvError::Disconnected)),
        "and the stream ends cleanly once the producer is gone"
    );
}

#[test]
fn the_owned_doorbell_outlives_the_consumers_use_of_it() {
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    let owned = rx.doorbell_owned().expect("duplication must succeed");
    tx.push(1).expect("there is room");

    // SAFETY: `owned` is a live event handle; a zero timeout returns at once.
    let result = unsafe { WaitForSingleObject(owned.as_raw_handle(), 0) };
    assert_eq!(
        result, WAIT_OBJECT_0,
        "a caller holding its own duplicate must see the queue's signals"
    );
}

#[test]
fn the_final_drain_returns_an_item_that_raced_the_disconnection() {
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");

    // The race `Consumer::finish` guards, reconstructed rather than waited for:
    // a producer pushed and then dropped in the window between a receive's
    // first `pop` and its disconnection check. At this point the queue reports
    // disconnected *and* holds an item.
    tx.push(1).expect("there is room");
    drop(tx);
    assert!(rx.is_disconnected(), "the producer is gone");

    assert_eq!(
        rx.finish(),
        Some(1),
        "the end of the stream must not discard an item that was sent before it"
    );
    assert_eq!(
        rx.finish(),
        None,
        "and once genuinely drained, the answer is final"
    );
}

#[test]
fn the_final_drain_is_empty_when_nothing_was_sent() {
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    drop(tx);
    assert_eq!(
        rx.finish(),
        None,
        "nothing was ever sent, so nothing is owed"
    );
}

#[test]
fn recv_timeout_does_not_panic_on_an_unrepresentable_deadline() {
    // `Instant + Duration` panics when the sum is not representable, and
    // `Duration::MAX` is an ordinary way to spell "effectively forever". The
    // queue is disconnected up front so the call has a reason to return at all;
    // the assertion is that it returns rather than aborting the process.
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    drop(tx);

    assert!(
        matches!(
            rx.recv_timeout(Duration::MAX),
            Err(RecvTimeoutError::Disconnected)
        ),
        "an unrepresentable deadline must degrade to the untimed wait it asked for"
    );
}

#[test]
fn recv_timeout_delivers_an_item_under_an_unrepresentable_deadline() {
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    tx.push(3).expect("there is room");

    // The degraded path must still be a working receive, not merely one that
    // does not panic.
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(u64::MAX))
            .expect("an item is queued"),
        3
    );
}

#[test]
fn a_suggested_capacity_is_one_the_constructor_would_accept() {
    // The suggestion exists so a caller can correct the call. One that is
    // itself refused is worse than none, because the caller acts on it.
    //
    // Asks `validate_capacity` rather than `bounded`, and rather than
    // re-listing the rules here. Calling `bounded` would be a truer test of the
    // real path, but a suggestion near the bound is 2^62, and constructing that
    // queue means asking for half the address space -- the first version of
    // this test aborted the process with a four-exabyte allocation failure.
    for requested in [1_usize, 3, 100, 1000, 0, usize::MAX / 2, usize::MAX] {
        let Err(error) = validate_capacity(requested, MIN_CAPACITY) else {
            continue;
        };
        if let Some(previous) = error.previous_valid() {
            assert!(
                validate_capacity(previous, MIN_CAPACITY).is_ok(),
                "previous_valid() for {requested} suggested {previous}, which is itself rejected"
            );
        }
        if let Some(next) = error.next_valid() {
            assert!(
                validate_capacity(next, MIN_CAPACITY).is_ok(),
                "next_valid() for {requested} suggested {next}, which is itself rejected"
            );
        }
    }
}

#[test]
fn the_largest_request_is_clamped_rather_than_rounded() {
    // Rounding `usize::MAX` down to the nearest power of two gives 2^63, which
    // is larger than the largest representable capacity. Before this was fixed
    // the suggestion was exactly that unusable value.
    let error = validate_capacity(usize::MAX, MIN_CAPACITY)
        .expect_err("usize::MAX is not a valid capacity");
    let previous = error
        .previous_valid()
        .expect("there is a valid capacity below usize::MAX");

    assert!(
        previous <= error.max_valid(),
        "the suggestion {previous} must not exceed the shape's own bound {}",
        error.max_valid()
    );
    assert!(
        previous.is_power_of_two(),
        "and it must still be a power of two"
    );
    assert!(
        validate_capacity(previous, MIN_CAPACITY).is_ok(),
        "and must be accepted"
    );
}

#[test]
fn the_real_arm_finds_an_item_that_lands_inside_its_window() {
    // The deterministic indictment of the reversed order, driven through the
    // REAL `Consumer::arm` rather than through a copy of it.
    //
    // The hook fires between `arm`'s clear and its emptiness check -- precisely
    // the window a producer must hit for the hazard to bite. With the correct
    // order the check follows the push and finds it, so arming refuses to bless
    // a wait. With the two statements swapped the check has already happened,
    // arming returns "safe to wait", and the consumer parks on a queue holding
    // an item whose signal the clear erased.
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    rx.doorbell().expect("the doorbell must be creatable");

    // The hook owns the producer outright. An `Arc` would be pointless here and
    // clippy says so: a `Producer` is deliberately `!Sync`, so sharing one is
    // exactly what the type system is built to prevent.
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
    // The complement, so the test above cannot pass by `arm` simply never
    // blessing a wait -- which would satisfy it while breaking every consumer.
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    rx.doorbell().expect("the doorbell must be creatable");
    drop(tx);

    let safe_to_wait = race_hooks::ARM.with(|| {}, || rx.arm().expect("arming must succeed"));

    assert!(
        safe_to_wait,
        "an empty queue must still be safe to wait on, or the wait never happens at all"
    );
}
