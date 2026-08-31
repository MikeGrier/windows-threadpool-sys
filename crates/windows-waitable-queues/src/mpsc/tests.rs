// Copyright (c) Mike Grier.

//! Tests for the MPSC bounded array queue.
//!
//! Every one runs in memory, and the whole file finishes in well under a
//! second. The multi-producer cases join every thread before asserting, so the
//! assertion runs after the peers have finished rather than after a guess about
//! how long they take.
//!
//! **They assert what the shape actually guarantees, and not more.** A
//! multi-producer queue promises that every item arrives exactly once and that
//! one producer's items keep that producer's order. It does *not* promise a
//! global interleaving, and a test that pinned one down would be asserting the
//! scheduler rather than the queue -- green today, red on a different machine,
//! and evidence of nothing either way.

use super::{Consumer, MIN_CAPACITY, Producer, bounded, validate_capacity};
use crate::arm_race;
use crate::{PushError, RecvError, RecvTimeoutError};
use std::collections::BTreeMap;
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

/// Pushes with a spin on a full queue.
///
/// Spinning rather than sleeping is right here: the consumer is draining
/// concurrently, so a full queue clears in nanoseconds, and a sleep would turn
/// a microsecond test into a millisecond one.
fn push_spinning<T>(producer: &Producer<T>, mut item: T) {
    loop {
        match producer.push(item) {
            Ok(()) => return,
            Err(PushError::Full(returned)) => {
                item = returned;
                std::hint::spin_loop();
            }
            Err(PushError::Disconnected(_)) => panic!("the consumer is alive"),
        }
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
fn the_smallest_capacity_holds_exactly_two() {
    // Two is this shape's floor rather than one, so the two-slot ring is the
    // edge case that `spsc`'s one-slot ring is: every push after the first two
    // is a refusal, and every pop frees exactly one slot.
    let (tx, rx) = bounded::<u32>(MIN_CAPACITY).expect("the shape's own minimum must be accepted");
    tx.push(1).expect("room for two");
    tx.push(2).expect("room for two");
    assert!(matches!(tx.push(3), Err(PushError::Full(3))));

    assert_eq!(rx.pop(), Some(1));
    tx.push(3).expect("the slot was freed");
    assert_eq!(rx.pop(), Some(2));
    assert_eq!(rx.pop(), Some(3));
    assert_eq!(rx.pop(), None);
}

#[test]
fn the_ring_wraps_many_times_without_losing_order() {
    // Far more operations than slots, so every slot is reused repeatedly. This
    // is the test that indicts the sequence arithmetic: a slot freed with the
    // wrong number is either claimed a lap early -- overwriting a live item --
    // or never claimed again, and both show up here as a wrong value or a
    // refusal rather than as a crash.
    let (tx, rx) = bounded::<usize>(4).expect("a power-of-two capacity");
    for round in 0..1000 {
        tx.push(round).expect("the previous item was taken");
        assert_eq!(rx.pop(), Some(round));
    }
    assert!(rx.is_empty());
}

#[test]
fn a_partly_full_ring_wraps_correctly() {
    // Keeps two items resident while cycling, so the head and the tail are
    // never equal and never a whole lap apart -- the case a simple "empty when
    // equal" test never reaches.
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
    // The interesting case for the drop loop: both positions are far from zero
    // and the live range straddles the end of the slot array, so a drop that
    // iterated `0..len` instead of `head..tail` would destroy the wrong slots
    // -- and would drop uninitialized memory.
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

// ---------------------------------------------------------------------------
// Capacity: the rule, and this shape's own floor.
// ---------------------------------------------------------------------------

#[test]
fn a_zero_capacity_is_refused_because_it_could_never_accept_anything() {
    let error = bounded::<u32>(0).expect_err("zero is not a usable capacity");
    assert_eq!(error.requested(), 0);
    assert_eq!(
        error.next_valid(),
        Some(MIN_CAPACITY),
        "the suggestion must be this shape's own floor, not a crate-wide one"
    );
}

#[test]
fn a_capacity_of_one_is_refused_because_the_sequence_protocol_cannot_encode_it() {
    // Not a taste, and not a copy of `spsc`'s rule: with one slot, "published
    // at position p" and "free again at position p + capacity" are the same
    // number, so a producer would read the sequence of the item it had just
    // pushed, conclude the slot was free, and overwrite an unread item.
    //
    // `spsc` accepts one, which is why the minimum belongs to the shape rather
    // than to the crate. Asserted here so that shipping a one-slot MPSC would
    // be a deliberate change to this test rather than a silent regression.
    let error = bounded::<u32>(1).expect_err("one slot cannot carry three states");
    assert_eq!(error.requested(), 1);
    assert_eq!(error.min_valid(), 2);
    assert_eq!(
        error.next_valid(),
        Some(2),
        "the correction must be offered, since one is an entirely reasonable ask"
    );
    assert_eq!(
        error.previous_valid(),
        None,
        "and there is nothing valid below it to suggest"
    );
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
fn every_power_of_two_capacity_from_the_floor_up_is_accepted() {
    for shift in 1..16 {
        let capacity = 1_usize << shift;
        let (tx, rx) = bounded::<usize>(capacity).expect("a power of two at or above the floor");
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
fn a_suggested_capacity_is_one_the_constructor_would_accept() {
    // The suggestion exists so a caller can correct the call. One that is
    // itself refused is worse than none, because the caller acts on it.
    //
    // Asks `validate_capacity` rather than `bounded`, and rather than
    // re-listing the rules here. Calling `bounded` would be a truer test of the
    // real path, but a suggestion near the bound is 2^62, and constructing that
    // queue means asking for half the address space.
    for requested in [0_usize, 1, 3, 100, 1000, usize::MAX / 2, usize::MAX] {
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

// ---------------------------------------------------------------------------
// Disconnection, in both directions.
// ---------------------------------------------------------------------------

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
fn the_queue_is_disconnected_only_when_the_last_producer_goes() {
    // The one place where multi-producer disconnection is genuinely different
    // from single-producer disconnection, and where a flag rather than a count
    // would be wrong: the first producer to leave must not end the stream for
    // the others.
    let (tx, rx) = bounded::<u32>(4).expect("a power-of-two capacity");
    let second = tx.clone();
    let third = second.clone();
    assert!(!rx.is_disconnected());

    drop(tx);
    assert!(!rx.is_disconnected(), "two producers remain");
    drop(second);
    assert!(!rx.is_disconnected(), "one producer remains");
    drop(third);
    assert!(
        rx.is_disconnected(),
        "and only now is the stream genuinely over"
    );
}

#[test]
fn a_producer_that_is_gone_leaves_the_queued_items_takeable() {
    // Disconnection must not discard what was already pushed, which is why the
    // documented order is drain first and check afterwards. The clone matters:
    // the items were pushed through a handle that no longer exists by the time
    // the consumer looks.
    let (tx, rx) = bounded::<u32>(4).expect("a power-of-two capacity");
    let second = tx.clone();
    tx.push(1).expect("room");
    second.push(2).expect("room");
    drop(tx);
    drop(second);

    assert!(rx.is_disconnected());
    assert_eq!(rx.pop(), Some(1), "a dropped producer does not discard");
    assert_eq!(rx.pop(), Some(2));
    assert_eq!(rx.pop(), None);
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
    assert!(rx.is_disconnected(), "the last producer is gone");

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

// ---------------------------------------------------------------------------
// Many producers, which is what this shape exists for.
// ---------------------------------------------------------------------------

/// How many producer threads the concurrent tests use.
///
/// Fixed rather than derived from the machine's core count, so a failure
/// reproduces on the machine that reported it. Four is enough to make the
/// compare-and-swap on the tail genuinely contended even on a two-core box,
/// because more threads than cores is exactly the case that interleaves a
/// producer between its claim and its publish.
const PRODUCERS: usize = 4;

/// How many items each producer sends in the concurrent tests.
const PER_PRODUCER: usize = 500;

/// Runs `PRODUCERS` threads against one queue and returns everything the
/// consumer saw, in arrival order, as `(producer, sequence)` pairs.
fn run_producers(capacity: usize) -> Vec<(usize, usize)> {
    let (tx, rx) = bounded::<(usize, usize)>(capacity).expect("a valid capacity");

    let threads: Vec<_> = (0..PRODUCERS)
        .map(|producer| {
            let handle = tx.clone();
            thread::spawn(move || {
                for sequence in 0..PER_PRODUCER {
                    push_spinning(&handle, (producer, sequence));
                }
            })
        })
        .collect();
    // The original handle would otherwise keep the queue connected for ever.
    drop(tx);

    let mut received = Vec::with_capacity(PRODUCERS * PER_PRODUCER);
    // Drains concurrently rather than after the join, which is the point: with
    // a capacity far below the run length the producers block on a full queue
    // and the consumer on an empty one, repeatedly and in both directions.
    while let Ok(item) = rx.recv() {
        received.push(item);
    }
    for thread in threads {
        thread.join().expect("no producer may panic");
    }
    received
}

#[test]
fn a_clone_pushes_into_the_same_queue() {
    let (tx, rx) = bounded::<u32>(4).expect("a power-of-two capacity");
    let second = tx.clone();
    tx.push(1).expect("room");
    second.push(2).expect("room");

    assert_eq!(rx.len(), 2, "one queue, not two");
    assert_eq!(rx.pop(), Some(1));
    assert_eq!(rx.pop(), Some(2));
}

#[test]
fn many_producers_deliver_every_item_exactly_once() {
    let received = run_producers(64);

    assert_eq!(
        received.len(),
        PRODUCERS * PER_PRODUCER,
        "no item may be lost, and none may be delivered twice"
    );

    let mut seen: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (producer, sequence) in received {
        seen.entry(producer).or_default().push(sequence);
    }
    assert_eq!(seen.len(), PRODUCERS, "every producer must be represented");
    for (producer, sequences) in seen {
        assert_eq!(
            sequences,
            (0..PER_PRODUCER).collect::<Vec<_>>(),
            "producer {producer} must have every one of its items, exactly once, in its own order"
        );
    }
}

#[test]
fn many_producers_against_the_smallest_queue_still_deliver_everything() {
    // The same run through a two-slot ring, so nearly every push is refused at
    // least once and the tail's compare-and-swap is contended continuously.
    // This is where a mis-ordered claim or a slot freed at the wrong sequence
    // stops being theoretical.
    let received = run_producers(MIN_CAPACITY);

    assert_eq!(received.len(), PRODUCERS * PER_PRODUCER);
    let mut per_producer = [0_usize; PRODUCERS];
    for (producer, sequence) in received {
        assert_eq!(
            sequence, per_producer[producer],
            "a producer's own items must arrive in that producer's order"
        );
        per_producer[producer] += 1;
    }
    assert!(per_producer.iter().all(|count| *count == PER_PRODUCER));
}

#[test]
fn a_producer_can_be_moved_to_another_thread_and_cloned() {
    // `Send` is what makes the split useful, and `Clone` is what makes this
    // shape multi-producer. `!Sync` is asserted by the absence of any test that
    // shares one handle across threads: the compiler refuses to write it.
    fn assert_send<T: Send>() {}
    fn assert_clone<T: Clone>() {}
    assert_send::<Producer<u32>>();
    assert_send::<Consumer<u32>>();
    assert_clone::<Producer<u32>>();

    let (tx, rx) = bounded::<u32>(4).expect("a power-of-two capacity");
    let second = tx.clone();
    thread::spawn(move || {
        second.push(7).expect("room");
    })
    .join()
    .expect("the pushing thread");
    tx.push(8).expect("room");

    assert_eq!(rx.pop(), Some(7));
    assert_eq!(rx.pop(), Some(8));
}

#[test]
fn items_cross_a_thread_boundary_intact() {
    // The real test of the memory ordering. Boxed, so each item is a heap
    // pointer the consumer must observe fully initialized -- a missing release
    // on the publishing store would surface as a corrupt pointer rather than as
    // a wrong integer.
    const COUNT: usize = 20_000;
    let (tx, rx) = bounded::<Box<usize>>(64).expect("a power-of-two capacity");

    let producer = thread::spawn(move || {
        for value in 0..COUNT {
            push_spinning(&tx, Box::new(value));
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

// ---------------------------------------------------------------------------
// The doorbell, joined to the queue.
//
// The tests below are about the *pairing* of the two; the doorbell's own
// behaviour as a kernel object is covered in `crate::doorbell`'s suite.
// ---------------------------------------------------------------------------

/// Whether the queue's doorbell is signalled right now, asked of the kernel
/// rather than of the mirror flag.
///
/// Uses a zero timeout, so it does not block, and the event is manual-reset, so
/// asking does not consume the answer.
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
    let second = tx.clone();
    for value in 0..4 {
        if value % 2 == 0 {
            tx.push(value).expect("there is room");
        } else {
            second.push(value).expect("there is room");
        }
    }
    while rx.pop().is_some() {}
    drop(tx);
    drop(second);
    while rx.pop().is_some() {}

    assert!(
        !rx.shared.doorbell.is_armed(),
        "a poll-only consumer must allocate no kernel object, even when a producer disconnects"
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
    // assertion below that arming CLEARS it would hold trivially.
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
    // producer's mirror flag has to have been cleared with it. Pushed through a
    // *different* handle, because the flag belongs to the queue rather than to
    // whichever producer last rang.
    let second = tx.clone();
    second.push(2).expect("there is room");
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

    // Arming creates the event and only then checks, so the item is found
    // instead of waited on. Had the check come first, this would report "safe
    // to wait" and the consumer would block on an item already queued.
    assert!(
        !rx.arm().expect("arming must succeed"),
        "arming must not bless a wait over an item that predates the doorbell"
    );
}

#[test]
fn the_real_arm_finds_an_item_that_lands_inside_its_window() {
    // The deterministic indictment of the reversed order, driven through the
    // REAL `Consumer::arm` rather than through a copy of it.
    //
    // The hook fires between `arm`'s clear and its readiness check -- precisely
    // the window a producer must hit for the hazard to bite. With the correct
    // order the check follows the push and finds it, so arming refuses to bless
    // a wait. With the two statements swapped the check has already happened,
    // arming returns "safe to wait", and the consumer parks on a queue holding
    // an item whose signal the clear erased.
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    rx.doorbell().expect("the doorbell must be creatable");

    // The hook owns the producer outright. Sharing one behind an `Arc` would be
    // pointless: a `Producer` is deliberately `!Sync`.
    let safe_to_wait = arm_race::with(
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
    // blessing anything.
    let (_tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    rx.doorbell().expect("the doorbell must be creatable");

    let safe_to_wait = arm_race::with(|| {}, || rx.arm().expect("arming must succeed"));
    assert!(
        safe_to_wait,
        "nothing arrived, so waiting is exactly what the consumer should do"
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
fn recv_delivers_items_pushed_before_the_last_producer_dropped() {
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    let second = tx.clone();
    tx.push(1).expect("there is room");
    second.push(2).expect("there is room");
    drop(tx);
    drop(second);

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
fn a_blocked_recv_is_released_only_by_the_last_producer_dropping() {
    let (tx, rx) = bounded::<u32>(4).expect("4 is a valid capacity");
    let second = tx.clone();

    let producers = thread::spawn(move || {
        // The first departure must NOT release the consumer, and the second
        // must. Without a signal in the last producer's `Drop` this test hangs
        // forever: the queue would be correct and the program still wedged.
        thread::sleep(Duration::from_millis(30));
        drop(tx);
        thread::sleep(Duration::from_millis(30));
        drop(second);
    });

    let started = Instant::now();
    assert!(
        matches!(rx.recv(), Err(RecvError::Disconnected)),
        "the last producer leaving must wake a parked consumer"
    );
    assert!(
        started.elapsed() >= Duration::from_millis(50),
        "and the FIRST producer leaving must not have released it"
    );
    producers.join().expect("the producers must not panic");
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
fn a_blocking_consumer_receives_every_item_from_every_producer() {
    // The whole mechanism under load, through the blocking path rather than by
    // polling: a capacity far smaller than the run, so the producers block on a
    // full queue and the consumer parks on an empty one, repeatedly.
    let received = run_producers(16);
    assert_eq!(
        received.len(),
        PRODUCERS * PER_PRODUCER,
        "a parked consumer must miss nothing"
    );
}
