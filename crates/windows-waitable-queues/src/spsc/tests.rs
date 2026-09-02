// Copyright (c) Mike Grier.

//! Tests for the SPSC bounded ring.
//!
//! Every one runs in memory in microseconds. The cross-thread cases use a
//! joined thread rather than a sleep, so they are deterministic: the assertion
//! runs after the peer has finished, not after a guess about how long it takes.

use super::{BOUNDS, Consumer, Producer, bounded, bounded_with, validate_capacity};
use crate::race_hooks;
use crate::{Disposal, Options};
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
    // **Both directions.** Asserting only that a full queue says so is
    // satisfied by an `is_full` that always says so -- which is a queue that
    // refuses every push, and a mutation run found exactly that constant alive
    // on all three shapes.
    assert!(!tx.is_full(), "an empty queue is not full");
    tx.push(1).expect("room");
    assert!(!tx.is_full(), "nor is a partly filled one");
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
fn arm_reports_safe_to_wait_on_an_empty_disconnected_queue() {
    // **The exception `arm`'s contract has to state, and the reason the
    // documented protocol needs a fourth step.**
    //
    // `arm` answers one question -- can a later *push* be missed -- and on a
    // queue with no producers left the answer is trivially no, so it says
    // `true`. Read as "safe to wait", which is what the contract used to say
    // flatly, that is a permanent hang: the last producer's drop rings the
    // doorbell exactly once, `arm` clears precisely that ring, and nothing
    // remains to ring it again.
    //
    // `blocking::recv` has always had the missing step -- it checks
    // disconnection and takes one last item before waiting. What was wrong was
    // every *statement* of the protocol: the trait's contract, three shapes'
    // method docs, three worked examples, and the README all described the
    // three-step form a caller could follow into an indefinite wait.
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    rx.doorbell().expect("the doorbell must be creatable");
    drop(tx);

    assert!(
        rx.is_disconnected(),
        "the producer is gone, so the stream has ended"
    );
    assert_eq!(rx.pop(), None, "and nothing is left to take");
    assert!(
        rx.arm().expect("arming must succeed"),
        "arm reports on missed pushes, not on the end of the stream -- so it \
         says `true` here, and a caller that treats that as permission to wait \
         indefinitely never wakes"
    );
    assert!(
        !doorbell_is_lit(&rx),
        "and it has consumed the one-shot wakeup the producer's drop left, \
         which is what makes the wait permanent rather than merely long"
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
        let Err(error) = validate_capacity(requested, BOUNDS) else {
            continue;
        };
        if let Some(previous) = error.previous_valid() {
            assert!(
                validate_capacity(previous, BOUNDS).is_ok(),
                "previous_valid() for {requested} suggested {previous}, which is itself rejected"
            );
        }
        if let Some(next) = error.next_valid() {
            assert!(
                validate_capacity(next, BOUNDS).is_ok(),
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
    let error =
        validate_capacity(usize::MAX, BOUNDS).expect_err("usize::MAX is not a valid capacity");
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
        validate_capacity(previous, BOUNDS).is_ok(),
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

// ---------------------------------------------------------------------------
// Reservation.
//
// The mechanism here is a plain counter written only by the producer's thread,
// where `reserving_mpsc` needs a compare-and-swap against a packed word. The
// two implementations share nothing, so the guarantee has to be asserted
// separately on each -- a point made empirically rather than by argument: the
// sabotage sweep found this whole section missing, because the reserving_mpsc
// tests covered the slotwise_mpsc path and left this one unguarded.
// ---------------------------------------------------------------------------

/// Fills every slot the best-effort path is allowed to take, and reports how
/// many went in.
fn fill(producer: &Producer<u32>) -> usize {
    let mut pushed = 0;
    while producer.push(0).is_ok() {
        pushed += 1;
    }
    pushed
}

#[test]
fn a_reservation_withholds_a_slot_from_the_best_effort_path() {
    let (tx, _rx) = bounded::<u32>(8).expect("8 is a valid capacity");
    assert_eq!(
        fill(&tx),
        8,
        "with nothing reserved, every slot is available"
    );

    let (tx, _rx) = bounded::<u32>(8).expect("8 is a valid capacity");
    let reservations: Vec<_> = (0..3).map(|_| tx.reserve().expect("room")).collect();
    assert_eq!(tx.outstanding_reservations(), 3);
    assert_eq!(
        fill(&tx),
        5,
        "three reserved leaves five for the best-effort path"
    );
    drop(reservations);
}

#[test]
fn a_reserved_slot_is_delivered_into_a_queue_that_is_otherwise_full() {
    // The contract in one test: reserve, let the best-effort path take
    // everything it is allowed to, and redeem anyway.
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    let slot = tx.reserve().expect("a fresh queue has room");

    assert_eq!(fill(&tx), 3, "the reservation withheld exactly one slot");
    assert!(tx.is_full(), "and now nothing more may be pushed");

    slot.send(99).expect("the room was already ours");

    let drained: Vec<u32> = std::iter::from_fn(|| rx.pop()).collect();
    assert_eq!(
        drained,
        vec![0, 0, 0, 99],
        "the reserved item lands where it was redeemed, not where it was claimed"
    );
}

#[test]
fn a_push_refused_for_a_reservation_is_still_reported_as_full() {
    // A best-effort caller cannot tell "no slots" from "the only slot is
    // reserved", and should not have to: both mean "no room for you".
    let (tx, _rx) = bounded::<u32>(2).expect("2 is a valid capacity");
    let _slot = tx.reserve().expect("room");
    tx.push(1).expect("one slot is unreserved");

    assert!(
        matches!(tx.push(2), Err(PushError::Full(2))),
        "the reserved slot is not available to the best-effort path"
    );
}

#[test]
fn dropping_a_reservation_returns_the_slot() {
    let (tx, _rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    let slot = tx.reserve().expect("room");
    assert_eq!(tx.outstanding_reservations(), 1);

    drop(slot);
    assert_eq!(tx.outstanding_reservations(), 0);
    assert_eq!(
        fill(&tx),
        4,
        "a released reservation is capacity given back, not capacity lost"
    );
}

#[test]
fn a_redeemed_reservation_does_not_also_release_its_slot() {
    // The double-release bug `send` avoids by consuming `self` and suppressing
    // the drop. If both ran, the count would underflow and the queue would
    // over-admit for ever afterwards.
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
    assert_eq!(fill(&tx), 4, "and the capacity is intact after ten cycles");
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
    fill(&tx);
    assert!(
        tx.reserve().is_none(),
        "and a full queue has nothing left to promise"
    );
}

#[test]
fn a_reservation_survives_many_wraps_of_the_ring() {
    // The counter must be independent of the positions, so hundreds of laps
    // beneath a held reservation must not disturb it.
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
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
        doorbell_is_lit(&rx),
        "a reserved delivery must ring like any other"
    );
    assert!(
        !rx.arm().expect("arming must succeed"),
        "and must be visible to the arming protocol"
    );
}

#[test]
fn a_blocked_consumer_is_woken_by_a_reserved_delivery() {
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    let slot = tx.reserve().expect("room");

    // **The reservation cannot cross a thread boundary, and the compiler says
    // so**: it borrows a producer that is not `Sync`, so `&Producer` is not
    // `Send` and even a scoped thread is refused. That is the borrow doing its
    // job rather than an inconvenience -- see the `compile_fail` doctest on
    // `Producer::reserve`, which asserts the refusal directly.
    //
    // So the *consumer* goes across instead. It is a separate handle and is
    // `Send`, which is what makes this test expressible at all.
    let receiver = thread::spawn(move || rx.recv());
    thread::sleep(Duration::from_millis(50));
    slot.send(7).expect("the consumer is alive");

    assert_eq!(
        receiver
            .join()
            .expect("the consumer must not panic")
            .expect("the reservation is redeemed"),
        7,
        "a parked consumer must be woken by a reserved delivery"
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

#[test]
fn dropping_the_queue_drops_a_reserved_item_it_still_holds() {
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
        "every undrained item must be dropped, including the reserved one"
    );
}

// ---------------------------------------------------------------------------
// Teardown: what becomes of items nobody drained.
//
// The disposal policy's own behaviour is covered in `crate::disposal`'s suite.
// What is asserted here is that THIS shape's teardown walk actually reaches it
// -- each shape finds its survivors by walking its own layout, so covering one
// says nothing about the others.
// ---------------------------------------------------------------------------

/// Records that it was destroyed, and where.
///
/// The distinction the whole mechanism turns on is "handed to the owner" versus
/// "destructor run by whichever thread dropped last", so a test needs to be able
/// to tell those apart rather than merely count survivors.
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
                // Moved out of teardown rather than destroyed in it, which is
                // the entire point: the owner now decides when and where.
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

    let rescued: Vec<u32> = reaper.iter().map(|item| item.id).collect();
    assert_eq!(
        rescued,
        vec![0, 1, 2, 3, 4],
        "every undrained item must reach the sink, in queue order"
    );
    assert_eq!(
        destroyed.load(Ordering::Relaxed),
        5,
        "and be destroyed only once the owner has finished with them"
    );
}

#[test]
fn only_the_undrained_items_reach_the_sink() {
    // What the consumer already took is the consumer's, and must not be
    // reported as abandoned.
    let (undelivered, reaper) = std::sync::mpsc::channel();
    let destroyed = Arc::new(AtomicUsize::new(0));

    {
        let (tx, rx) = bounded_with::<Tracked>(
            8,
            Options::new().disposal(Disposal::new(move |item: Tracked| {
                let _ = undelivered.send(item.id);
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
        assert_eq!(rx.pop().expect("an item").id, 0);
        assert_eq!(rx.pop().expect("an item").id, 1);
    }

    assert_eq!(
        reaper.iter().collect::<Vec<_>>(),
        vec![2, 3, 4],
        "the two the consumer took are not abandoned items"
    );
}

#[test]
fn an_empty_queue_hands_nothing_to_the_sink() {
    let (undelivered, reaper) = std::sync::mpsc::channel();
    {
        let (tx, rx) = bounded_with::<u32>(
            4,
            Options::new().disposal(Disposal::new(move |item| {
                let _ = undelivered.send(item);
            })),
        )
        .expect("4 is a valid capacity");
        tx.push(1).expect("room");
        assert_eq!(rx.pop(), Some(1));
    }
    assert_eq!(
        reaper.iter().collect::<Vec<_>>(),
        Vec::<u32>::new(),
        "a queue drained to empty has nothing to account for"
    );
}

#[test]
fn the_sink_sees_survivors_after_the_ring_has_wrapped() {
    // The teardown walk is over a wrapped range, which is where an index error
    // would show up as the wrong items rather than as a crash.
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
    assert_eq!(
        reaper.iter().collect::<Vec<_>>(),
        vec![100, 101, 102],
        "the survivors are the resident range, not the whole slot array"
    );
}

#[test]
fn a_queue_torn_down_by_the_producer_still_reaches_the_sink() {
    // Which handle happens to die last is not knowable in advance, and the
    // guarantee must not depend on it. Here the consumer goes first, so the
    // producer's drop is what tears the queue down.
    let (undelivered, reaper) = std::sync::mpsc::channel();
    let (tx, rx) = bounded_with::<u32>(
        4,
        Options::new().disposal(Disposal::new(move |item| {
            let _ = undelivered.send(item);
        })),
    )
    .expect("4 is a valid capacity");

    tx.push(1).expect("room");
    tx.push(2).expect("room");
    drop(rx);
    drop(tx);

    assert_eq!(
        reaper.iter().collect::<Vec<_>>(),
        vec![1, 2],
        "teardown accounts for the survivors whichever handle releases last"
    );
}

#[test]
fn a_queue_torn_down_on_another_thread_still_reaches_the_sink() {
    // The dropping thread is whichever one happens to release last, which is
    // exactly why disposal cannot be left to it implicitly.
    let (undelivered, reaper) = std::sync::mpsc::channel();
    let (tx, rx) = bounded_with::<u32>(
        4,
        Options::new().disposal(Disposal::new(move |item| {
            let _ = undelivered.send(item);
        })),
    )
    .expect("4 is a valid capacity");

    tx.push(1).expect("room");
    drop(rx);

    thread::spawn(move || drop(tx))
        .join()
        .expect("the dropping thread must not panic");

    assert_eq!(reaper.iter().collect::<Vec<_>>(), vec![1]);
}

#[test]
fn without_a_sink_undrained_items_are_destroyed_in_place() {
    // The default, asserted rather than assumed -- it is the behaviour every
    // existing caller has, and the reason a queue of `u32` need not think about
    // any of this.
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
    assert_eq!(
        destroyed.load(Ordering::Relaxed),
        3,
        "with no sink there is nowhere else for them to go"
    );
}

// ---------------------------------------------------------------------------
// The hazard itself, stated as a test rather than as a paragraph.
//
// The claim disposal exists to make good on is not "the sink receives the
// items" -- that is the mechanism. The claim is that **a destructor which
// blocks does not run on whichever thread happened to release the last
// handle**, because that thread may be a pool callback that must not block. So
// these two assert where the destructor actually runs, with a control proving
// the test can tell the difference.
// ---------------------------------------------------------------------------

/// Records the thread its destructor ran on.
#[derive(Debug)]
struct ThreadWitness(Arc<std::sync::Mutex<Vec<std::thread::ThreadId>>>);

impl Drop for ThreadWitness {
    fn drop(&mut self) {
        self.0
            .lock()
            .expect("no test holds this poisoned")
            .push(std::thread::current().id());
    }
}

#[test]
fn a_sink_keeps_the_destructor_off_the_thread_that_tore_the_queue_down() {
    let ran_on = Arc::new(std::sync::Mutex::new(Vec::new()));
    let (undelivered, reaper) = std::sync::mpsc::channel();

    let (tx, rx) = bounded_with::<ThreadWitness>(
        4,
        Options::new().disposal(Disposal::new(move |item: ThreadWitness| {
            // The sink's whole job: move it somewhere a thread that may block
            // will find it. Nothing here runs the destructor.
            let _ = undelivered.send(item);
        })),
    )
    .expect("4 is a valid capacity");

    tx.push(ThreadWitness(Arc::clone(&ran_on))).expect("room");

    // Tear the queue down somewhere that is emphatically not this thread,
    // standing in for the pool callback that must not block.
    let teardown_thread = thread::spawn(move || {
        drop(rx);
        drop(tx);
        std::thread::current().id()
    })
    .join()
    .expect("the tearing-down thread must not panic");

    assert!(
        ran_on.lock().expect("not poisoned").is_empty(),
        "the destructor must not have run yet: the item is the owner's now, and \
         the thread that dropped the queue has already moved on"
    );

    // The owner takes delivery here, and *this* is where the destructor runs.
    let rescued = reaper.recv().expect("the sink was handed the survivor");
    drop(rescued);

    let ran_on = ran_on.lock().expect("not poisoned");
    assert_eq!(ran_on.len(), 1);
    assert_ne!(
        ran_on[0], teardown_thread,
        "a blocking destructor must not run on the thread that released the last handle"
    );
    assert_eq!(
        ran_on[0],
        std::thread::current().id(),
        "it runs where the owner chose to take delivery"
    );
}

#[test]
fn without_a_sink_the_destructor_does_run_on_the_thread_that_tore_the_queue_down() {
    // The control. Without it the test above could pass for the wrong reason --
    // it would look identical if destructors simply never ran anywhere
    // observable. This is also the honest statement of the default: it is not
    // that nothing blocks, it is that the blocking lands on a thread nobody
    // chose.
    let ran_on = Arc::new(std::sync::Mutex::new(Vec::new()));

    let (tx, rx) = bounded::<ThreadWitness>(4).expect("4 is a valid capacity");
    tx.push(ThreadWitness(Arc::clone(&ran_on))).expect("room");

    let teardown_thread = thread::spawn(move || {
        drop(rx);
        drop(tx);
        std::thread::current().id()
    })
    .join()
    .expect("the tearing-down thread must not panic");

    let ran_on = ran_on.lock().expect("not poisoned");
    assert_eq!(ran_on.len(), 1, "the item was destroyed at teardown");
    assert_eq!(
        ran_on[0], teardown_thread,
        "and with no sink it was destroyed on whichever thread released last, \
         which is exactly the behaviour a disposal sink exists to replace"
    );
}

// ---------------------------------------------------------------------------
// Observability.
//
// The counters' arithmetic is covered in `crate::metrics`. What is asserted
// here is that this shape *feeds* them from the right places -- and, for the
// doorbell, that the number reports syscalls rather than signal attempts,
// which is what makes the skip rule measurable rather than assumed.
// ---------------------------------------------------------------------------

#[test]
fn refusals_are_counted_but_disconnections_are_not() {
    // The two are different facts and must not be summed. A full queue is
    // backpressure; a departed consumer is the end of the stream, and a queue
    // shutting down should not read as an overloaded one.
    let (tx, rx) = bounded::<u32>(2).expect("2 is a valid capacity");
    assert_eq!(tx.refused(), 0);

    tx.push(1).expect("room");
    tx.push(2).expect("room");
    assert!(tx.push(3).is_err());
    assert!(tx.push(4).is_err());
    assert_eq!(tx.refused(), 2, "two pushes were refused for want of room");
    assert_eq!(rx.refused(), 2, "and both handles report the same queue");

    drop(rx);
    assert!(matches!(tx.push(5), Err(PushError::Disconnected(5))));
    assert_eq!(
        tx.refused(),
        2,
        "a push refused because the consumer is gone is not a loss to backpressure"
    );
}

#[test]
fn high_water_is_untracked_by_default() {
    // Off unless asked for, because it is the one metric that cannot be made
    // free. `None` rather than `Some(0)` so a caller cannot mistake "nobody was
    // counting" for "it never filled".
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    tx.push(1).expect("room");
    tx.push(2).expect("room");

    assert_eq!(tx.high_water(), None);
    assert_eq!(rx.high_water(), None);
}

#[test]
fn high_water_records_the_peak_when_asked_for() {
    let (tx, rx) = bounded_with::<u32>(8, Options::new().tracking_high_water())
        .expect("8 is a valid capacity");
    assert_eq!(tx.high_water(), Some(0), "counting, and nothing seen yet");

    for value in 0..5 {
        tx.push(value).expect("room");
    }
    assert_eq!(tx.high_water(), Some(5));

    // Draining does not lower it: the peak is a fact about the past.
    while rx.pop().is_some() {}
    assert_eq!(rx.len(), 0);
    assert_eq!(
        rx.high_water(),
        Some(5),
        "the mark is the deepest it got, not the depth right now"
    );

    // And a smaller later burst does not replace it.
    tx.push(0).expect("room");
    tx.push(1).expect("room");
    assert_eq!(tx.high_water(), Some(5));
}

#[test]
fn high_water_counts_reserved_deliveries_like_any_other() {
    let (tx, rx) = bounded_with::<u32>(4, Options::new().tracking_high_water())
        .expect("4 is a valid capacity");

    let slot = tx.reserve().expect("room");
    tx.push(1).expect("room");
    tx.push(2).expect("room");
    slot.send(3).expect("the room was ours");

    assert_eq!(
        tx.high_water(),
        Some(3),
        "a redeemed reservation is an ordinary queued item, and counts as depth like one"
    );
    assert_eq!(rx.len(), 3);
}

#[test]
fn a_poll_only_consumer_rings_no_doorbells() {
    // The laziness being visible rather than a gap: a consumer that never asks
    // for the handle never creates the event, so there is nothing to ring.
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    for value in 0..4 {
        tx.push(value).expect("room");
    }
    while rx.pop().is_some() {}

    assert_eq!(
        rx.doorbell_rings(),
        0,
        "no kernel object was created, so no signal was ever issued"
    );
}

#[test]
fn the_ring_count_reports_syscalls_rather_than_signal_attempts() {
    // **The number the skip rule is measured by.** Four pushes against a
    // doorbell nobody clears is one real `SetEvent` and three skips, because a
    // manual-reset event does not count and setting an already-set one changes
    // nothing. If this ever reported four, the skip would have stopped
    // happening -- which is exactly what the sabotage entry for it asserts.
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
fn clearing_the_doorbell_makes_the_next_push_ring_again() {
    // The complement: the count must not be stuck at one. Each drain-and-arm
    // cycle costs exactly one more ring, which is the shape a parked consumer
    // actually produces.
    let (tx, rx) = bounded::<u32>(8).expect("8 is a valid capacity");
    rx.doorbell().expect("the doorbell must be creatable");

    for round in 1..=3 {
        tx.push(round).expect("room");
        tx.push(round).expect("room");
        assert_eq!(
            rx.doorbell_rings(),
            round as u64,
            "round {round}: one ring per cycle, not one per push"
        );
        while rx.pop().is_some() {}
        assert!(rx.arm().expect("arming must succeed"));
    }
}

#[test]
fn the_debug_renderings_name_the_type_and_its_state() {
    // A `Debug` that writes nothing satisfies any test which only checks that
    // formatting does not panic, and a mutation run found exactly that constant
    // alive on every handle in this crate. These are the diagnostic surface a
    // reader reaches for when a queue is stuck, so an empty rendering is the
    // moment it is least affordable.
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    tx.push(1).expect("room");

    let producer = format!("{tx:?}");
    assert!(producer.contains("spsc::Producer"), "got {producer}");
    assert!(producer.contains('4'), "the capacity must show: {producer}");

    let consumer = format!("{rx:?}");
    assert!(consumer.contains("spsc::Consumer"), "got {consumer}");

    let reservation = tx.reserve().expect("there is room");
    let rendered = format!("{reservation:?}");
    assert!(rendered.contains("spsc::Reservation"), "got {rendered}");
}

// ---------------------------------------------------------------------------
// The gauges: reservations withdraw capacity without becoming items, and the
// two position loads are not one instant.
// ---------------------------------------------------------------------------

#[test]
fn remaining_subtracts_outstanding_reservations() {
    // The defect. `Bounded`'s default is `capacity - len`, and a reservation
    // withdraws a slot without becoming an item -- so with every slot reserved
    // the default answered the full capacity while both `push` and `reserve`
    // refuse. This shape reserves too, which is exactly what made it easy to
    // miss when the sibling was fixed.
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    let slot = tx.reserve().expect("an empty queue has room");

    assert_eq!(tx.len(), 0, "a reservation is not an item");
    assert_eq!(tx.remaining(), 3, "one of the four slots is spoken for");
    assert_eq!(
        crate::Bounded::remaining(&rx),
        3,
        "both handles describe the same queue and must agree"
    );

    for i in 0..3 {
        tx.push(i).expect("remaining() said there was room");
    }
    assert_eq!(tx.remaining(), 0);
    assert!(tx.is_full());
    assert!(matches!(tx.push(99), Err(PushError::Full(99))));

    slot.send(7).expect("the consumer is still here");
    assert_eq!(rx.len(), 4, "the redeemed reservation is now an item");
}

#[test]
fn remaining_is_zero_when_every_slot_is_reserved() {
    // The case the finding named directly: reserve everything, and the queue is
    // empty of items yet has no room at all.
    let (tx, _rx) = bounded::<u32>(2).expect("2 is a valid capacity");
    let _first = tx.reserve().expect("room");
    let _second = tx.reserve().expect("room");

    assert_eq!(tx.len(), 0, "no item has been sent");
    assert_eq!(
        tx.remaining(),
        0,
        "every slot is spoken for, so nothing further fits"
    );
    assert!(tx.is_full());
    assert!(tx.reserve().is_none(), "and no further slot can be claimed");
}

#[test]
fn remaining_agrees_through_the_bounded_trait() {
    // The override is on the trait impls, not only the inherent methods: a
    // caller generic over `Bounded` is exactly who would be misled by the
    // default, since it cannot reach `outstanding_reservations` to correct it.
    fn room_through_trait<B: crate::Bounded>(handle: &B) -> usize {
        handle.remaining()
    }

    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    let _slot = tx.reserve().expect("an empty queue has room");

    assert_eq!(room_through_trait(&tx), 3);
    assert_eq!(room_through_trait(&rx), 3);
}

#[test]
fn len_is_clamped_when_head_has_passed_the_sampled_tail() {
    // `len` reads `tail` and then `head`, which are two instants rather than
    // one. If the consumer drains past the value `tail` held, `head` overtakes
    // it and the wrapping subtraction yields a number near `usize::MAX`.
    //
    // The skewed pair is written directly rather than raced for: it is a
    // transient a reader observes, not a state the queue rests in.
    let (tx, _rx) = bounded::<u32>(4).expect("4 is a valid capacity");

    tx.shared.tail.0.store(1, Ordering::Release);
    tx.shared.head.0.store(2, Ordering::Release);

    assert_eq!(
        tx.len(),
        tx.capacity(),
        "a bounded queue must never report holding more than it can"
    );
    assert_eq!(tx.remaining(), 0, "the clamp resolves towards full");

    // Restored before the handles drop: teardown walks `head..tail`, and an
    // inverted pair sets it a `usize::MAX`-length loop that hangs rather than
    // fails.
    tx.shared.head.0.store(0, Ordering::Release);
    tx.shared.tail.0.store(0, Ordering::Release);
}

#[test]
fn the_gauges_are_exact_when_nothing_is_reserved_or_skewed() {
    // The guard must not have been bought by clamping or subtracting always.
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
