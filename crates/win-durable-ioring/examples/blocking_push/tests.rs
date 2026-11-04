// Copyright (c) 2026 Mike Grier
//! The example's tests: they run with the crate's, so what the example claims is kept true.

use std::io;
use std::time::{Duration, Instant};

use win_durable_ioring::{DurableRing, IoTargetKey, PushRefusal};

use super::{
    Blocked, Delivery, FILE, GaveUp, OPERATIONS, Scratch, build, push_blocking, run, run_with,
    write_at,
};

/// Ten times the entries the queues hold, so a push the ring will not take is certain.
const WRITES: usize = 40;
const QUEUE: u32 = 4;
/// A small declared capacity, which the writes below are measured against. The claim is that a
/// consumer that seals and pops gets through more than it, and that holds at any size -- but not
/// every defect is reachable at any size: an epoch is an eighth of the capacity, and below about
/// 24 operations an epoch is smaller than the queue, so a seal never meets a completion queue it
/// cannot drain into and a pop refused for want of room never happens, which the sabotage entry
/// "a pop refused for want of room is a fault" needs. Measured, not derived: 16 let it through,
/// 24 and 32 caught it.
const SMALL_OPERATIONS: usize = 32;

/// Three times the capacity a run is built for, so that capacity is certain to be reached.
fn past(operations: usize) -> usize {
    3 * operations
}

fn block() -> Vec<u8> {
    vec![7; 8]
}

/// The instance holds a write's place until its completion is popped and a seal has covered it,
/// so a consumer that never seals is refused at its declared operations however fast it pops.
/// Every delivery has to get through, and has to have sealed to do it.
fn writes_past_the_declared_capacity(operations: usize) {
    for delivery in [Delivery::Pool, Delivery::Pumped, Delivery::Signalled] {
        let writes = past(operations);
        let report = run_with(delivery, writes, QUEUE, operations);
        assert_eq!(report.pushed, writes, "{delivery:?}: {report:?}");
        assert_eq!(report.completed, writes, "{delivery:?}: {report:?}");
        assert!(
            report.sealed > 1,
            "{delivery:?} sealed as it went: {report:?}"
        );
    }
}

#[test]
fn writes_past_a_small_declared_capacity_are_taken_by_sealing_and_popping() {
    writes_past_the_declared_capacity(SMALL_OPERATIONS);
}

/// The same claim at the size the demonstration is built for, which is the full scale: three times
/// its declared operations, under each delivery. Nothing about it differs from the small one but
/// the size.
mod long_haul {
    use super::*;

    #[test]
    #[ignore = "long-haul: three deliveries of three times the 256 operations the example is built for"]
    fn writes_past_the_demonstrations_declared_capacity_are_taken_by_sealing_and_popping() {
        writes_past_the_declared_capacity(OPERATIONS);
    }
}

#[test]
fn a_pumped_instance_takes_far_more_writes_than_its_queues_hold_by_pumping() {
    let report = run(Delivery::Pumped, WRITES, QUEUE);
    assert_eq!(report.pushed, WRITES);
    assert_eq!(report.completed, WRITES);
    assert!(
        report.made_room > 0,
        "the ring refused, and the helper made room: {report:?}"
    );
}

#[test]
fn a_signalled_instance_takes_them_the_same_way() {
    let report = run(Delivery::Signalled, WRITES, QUEUE);
    assert_eq!(report.pushed, WRITES);
    assert_eq!(report.completed, WRITES);
    assert!(report.made_room > 0, "{report:?}");
}

#[test]
fn a_pool_instance_takes_them_by_waiting_for_the_pool() {
    // The pool drains on its own, so whether a push is ever refused is up to its timing; what the
    // helper promises is that every write is taken.
    let report = run(Delivery::Pool, WRITES, QUEUE);
    assert_eq!(report.pushed, WRITES);
    assert_eq!(report.completed, WRITES);
}

#[test]
fn a_refusal_waiting_cannot_clear_is_returned_at_once_with_its_buffer() {
    let scratch = Scratch::new("unknown-file");
    let mut ring = build(Delivery::Pumped, &scratch, 8, OPERATIONS);
    let lineage = ring.default_lineage();
    let mut rooms = 0;
    let Err(Blocked { refused, why }) = push_blocking(
        &mut ring,
        block(),
        (),
        Duration::from_secs(5),
        write_at(&lineage, IoTargetKey(99), 0),
        |_, _| {
            rooms += 1;
            Ok(())
        },
    ) else {
        panic!("a file the instance was not given cannot be written");
    };
    assert!(matches!(why, GaveUp::NotRetryable), "{why:?}");
    assert!(matches!(
        refused.reason,
        PushRefusal::UnknownFile(IoTargetKey(99))
    ));
    assert_eq!(
        refused.buffer,
        Some(block()),
        "the buffer comes back, untouched"
    );
    assert_eq!(rooms, 0, "nothing was waited for");
}

/// Push with plain attempts until the ring refuses for want of room, which a pumped instance does
/// once its queues are full, and nothing drains.
fn fill(ring: &mut super::Ring, lineage: &win_durable_ioring::LineageHandle) -> usize {
    for pushed in 0..1_000 {
        let offset = u64::try_from(pushed).expect("small") * 8;
        if let Err(refused) = ring.write(FILE, offset, block(), lineage.at(1_u64), ()) {
            assert!(
                matches!(&refused.reason, PushRefusal::Backend(cause) if cause.is_retryable()),
                "{:?}",
                refused.reason
            );
            return pushed;
        }
    }
    panic!("the ring never refused");
}

#[test]
fn a_push_that_never_gets_room_times_out_and_hands_its_buffer_back() {
    let scratch = Scratch::new("times-out");
    let mut ring = build(Delivery::Pumped, &scratch, QUEUE, OPERATIONS);
    let lineage = ring.default_lineage();
    let filled = fill(&mut ring, &lineage);
    let mut rooms = 0;
    let patience = Duration::from_millis(80);
    let started = Instant::now();
    let Err(Blocked { refused, why }) = push_blocking(
        &mut ring,
        block(),
        (),
        patience,
        write_at(&lineage, FILE, u64::try_from(filled).expect("small") * 8),
        // Makes no room: the wait elapses and the ring is as full as it was.
        |_, slice| {
            rooms += 1;
            std::thread::sleep(slice);
            Ok(())
        },
    ) else {
        panic!("nothing made room");
    };
    assert!(matches!(why, GaveUp::TimedOut), "{why:?}");
    assert!(
        started.elapsed() >= patience,
        "gave up after {:?}",
        started.elapsed()
    );
    assert!(rooms > 0);
    assert_eq!(refused.buffer, Some(block()));
}

#[test]
fn a_make_room_that_fails_is_reported_and_hands_the_push_back() {
    let scratch = Scratch::new("room-fails");
    let mut ring = build(Delivery::Pumped, &scratch, QUEUE, OPERATIONS);
    let lineage = ring.default_lineage();
    let filled = fill(&mut ring, &lineage);
    let Err(Blocked { refused, why }) = push_blocking(
        &mut ring,
        block(),
        (),
        Duration::from_secs(5),
        write_at(&lineage, FILE, u64::try_from(filled).expect("small") * 8),
        |_, _| Err(io::Error::other("no room could be made")),
    ) else {
        panic!("the ring was full and nothing made room");
    };
    match why {
        GaveUp::MakeRoom(error) => assert!(error.to_string().contains("no room"), "{error}"),
        other => panic!("expected the make-room error, got {other:?}"),
    }
    assert_eq!(refused.buffer, Some(block()));
}

#[test]
fn a_push_the_ring_takes_never_calls_make_room() {
    let scratch = Scratch::new("takes-it");
    let mut ring = build(Delivery::Pumped, &scratch, 8, OPERATIONS);
    let lineage = ring.default_lineage();
    let mut rooms = 0;
    let id = push_blocking(
        &mut ring,
        block(),
        (),
        Duration::from_secs(5),
        write_at(&lineage, FILE, 0),
        |_, _| {
            rooms += 1;
            Ok(())
        },
    );
    assert!(id.is_ok());
    assert_eq!(rooms, 0);
}
