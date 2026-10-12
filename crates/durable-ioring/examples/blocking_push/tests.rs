// Copyright (c) 2026 Mike Grier
//! The example's tests: they run with the crate's, so what the example claims is kept true.

use std::io;
use std::time::{Duration, Instant};

use durable_ioring::{DurableRing, IoTargetKey, PushRefusal};

use super::{Blocked, Delivery, FILE, GaveUp, Scratch, build, push_blocking, run, write_at};

/// Far more writes than the queues hold, so a push the ring will not take is certain.
const WRITES: usize = 200;
const QUEUE: u32 = 4;

fn block() -> Vec<u8> {
    vec![7; 8]
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
    let mut ring = build(Delivery::Pumped, &scratch, 8);
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
fn fill(ring: &mut super::Ring, lineage: &durable_ioring::LineageHandle) -> usize {
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
    let mut ring = build(Delivery::Pumped, &scratch, QUEUE);
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
    let mut ring = build(Delivery::Pumped, &scratch, QUEUE);
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
    let mut ring = build(Delivery::Pumped, &scratch, 8);
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
