// Copyright (c) 2026 Mike Grier
//! Unit tests for the Model A front end: delivery from the handover on, one entry at a time, the
//! handler calling back in, operations from any thread, and ending.

use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use windows_ioring_sys::RegisteredSpan;

use super::EntryDelivery;
use crate::contract::DurableRing;
use crate::dioring::tests::TempFile;
use crate::dioring::{Dioring, FileSetup, Setup};
use crate::fake::{Fake, Signals};
use crate::ids::DioringIds;
use crate::oracle::ConformanceOracle;
use crate::types::{
    DurabilityRequest, Entry, Epoch, EpochState, FileKey, FileOptions, OpKind, PushRefusal,
    Resolution,
};

type Ring = Dioring<Vec<u8>, u64, u32>;
type V = DioringIds<u64>;
type RingEntry = Entry<V, Vec<u8>, u32>;

/// How long a test waits for a delivery before calling it lost.
const BOUND: Duration = Duration::from_secs(5);
const FILE: FileKey = FileKey(1);

fn temp() -> TempFile {
    let temp = TempFile::new("delivery");
    fs::write(&temp.0, [0; 4096]).expect("fill the temporary file");
    temp
}

fn instance(temp: &TempFile, buffers: Vec<Vec<u8>>) -> Ring {
    Ring::new(Setup {
        submission_queue_size: 128,
        completion_queue_size: 128,
        files: vec![FileSetup {
            key: FILE,
            file: temp.open(),
            options: FileOptions::new(),
        }],
        buffers,
        provider: None,
    })
    .expect("build an instance")
    .0
}

/// A front end whose handler forwards every entry to the returned receiver.
fn forwarding(ring: Ring) -> (EntryDelivery<Ring>, mpsc::Receiver<RingEntry>) {
    let (tx, rx) = mpsc::channel();
    let delivery = EntryDelivery::new(
        ring,
        move |entry, _| {
            let _ = tx.send(entry);
        },
        None,
    )
    .expect("start delivery");
    (delivery, rx)
}

fn context(entry: &RingEntry) -> u32 {
    match entry {
        Entry::Op(completion) => completion.context,
        other => panic!("expected a completion: {other:?}"),
    }
}

#[test]
fn entries_queued_before_the_handover_are_delivered_in_queue_order() {
    let mut fake = Fake::new(Signals::OnEveryTransition);
    for _ in 0..3 {
        fake.complete_one();
    }
    // The wake those entries raised has been consumed, as a waiter before the handover would have.
    fake.signal.reset().expect("consume the wake");

    let (tx, rx) = mpsc::channel();
    let delivery = EntryDelivery::new(
        fake,
        move |entry, _| {
            let _ = tx.send(context(&entry));
        },
        None,
    )
    .expect("start delivery");
    let delivered: Vec<u32> = (0..3)
        .map(|_| rx.recv_timeout(BOUND).expect("the backlog is delivered"))
        .collect();
    assert_eq!(delivered, [0, 1, 2]);
    drop(delivery);
}

#[test]
fn a_failed_setup_hands_back_the_instance_and_the_handler() {
    let mut fake = Fake::new(Signals::OnEveryTransition);
    fake.readiness_fails = true;
    let marker = Arc::new(());
    let held = Arc::clone(&marker);
    let error = match EntryDelivery::new(
        fake,
        move |_, _| {
            let _ = &held;
        },
        None,
    ) {
        Ok(_) => panic!("the fake refuses its signal"),
        Err(error) => error,
    };
    assert!(
        error.ring.readiness_fails,
        "the instance given is the one returned"
    );
    assert_eq!(
        Arc::strong_count(&marker),
        2,
        "the handler comes back alive"
    );
    drop(error);
    assert_eq!(Arc::strong_count(&marker), 1);
}

#[test]
fn every_operation_pushed_through_the_owner_is_delivered_once() {
    let temp = temp();
    let (delivery, rx) = forwarding(instance(&temp, Vec::new()));
    let mut oracle = ConformanceOracle::<V, u32>::new();
    let default = delivery.default_lineage();
    let epoch = Epoch::new(default.lineage(), 1);
    for i in 0..16_u32 {
        let id = delivery
            .write(FILE, u64::from(i) * 16, vec![i as u8; 16], default.at(1), i)
            .expect("push a write");
        oracle
            .pushed(id, OpKind::Write { epoch }, i)
            .expect("the oracle accepts the push");
    }
    for _ in 0..16 {
        let entry = rx.recv_timeout(BOUND).expect("a delivery");
        oracle
            .observe(&entry)
            .expect("the oracle accepts the entry");
    }
    oracle.finish().expect("every write was delivered");
}

#[test]
fn a_seal_made_through_the_handle_is_delivered_as_durable() {
    let temp = temp();
    let (tx, rx) = mpsc::channel();
    let delivery = EntryDelivery::new(
        instance(&temp, Vec::new()),
        move |entry: RingEntry, handle| {
            if let Entry::Op(completion) = &entry {
                let default = handle.default_lineage();
                let answer = handle
                    .make_durable_through(default.at(1))
                    .expect("seal from inside the handler");
                assert_eq!(answer, DurabilityRequest::Submitted, "{:?}", completion.id);
            }
            let _ = tx.send(entry);
        },
        None,
    )
    .expect("start delivery");
    let default = delivery.default_lineage();
    let lineage = default.lineage();
    let epoch = Epoch::new(lineage, 1);
    delivery
        .write(FILE, 0, vec![1; 8], default.at(1), 0)
        .expect("push a write");
    let delivered: Vec<RingEntry> = (0..2).filter_map(|_| rx.recv_timeout(BOUND).ok()).collect();
    let durable = matches!(delivered.as_slice(), [Entry::Op(_), Entry::Durable { through }] if *through == epoch);
    if !durable {
        // As below: a handler stuck on the instance's lock must fail the test, not hang its drop.
        std::mem::forget(delivery);
        panic!("expected the write's completion and then Durable through 1: {delivered:?}");
    }
    assert_eq!(delivery.durable_through(lineage), Ok(Some(1)));
    assert_eq!(delivery.epoch_state(epoch), Ok(EpochState::Durable));
}

#[test]
fn a_failure_resolved_through_the_handle_is_delivered_with_what_follows_it() {
    let temp = temp();
    let mut ring = instance(&temp, Vec::new());
    ring.fail_next_flush(FILE, 1117);
    let (tx, rx) = mpsc::channel();
    let delivery = EntryDelivery::new(
        ring,
        move |entry: RingEntry, handle| {
            // No panic here: it would be on a pool thread. The outcome is sent instead.
            let seen = match entry {
                Entry::Op(_) => "op",
                Entry::Failed(failed) => {
                    match handle.resolve(vec![(failed.token, Resolution::Abandon)]) {
                        Ok(()) => "failed, abandoned from the handler",
                        Err(_) => "failed, and the handle refused to resolve it",
                    }
                }
                Entry::Abandoned { .. } => "abandoned",
                Entry::Durable { .. } => "durable",
                _ => "something else",
            };
            let _ = tx.send(seen);
        },
        None,
    )
    .expect("start delivery");
    let default = delivery.default_lineage();
    let epoch = Epoch::new(default.lineage(), 1);
    delivery
        .write(FILE, 0, vec![1; 8], default.at(1), 0)
        .expect("push a write");
    let op = rx.recv_timeout(BOUND);
    delivery.make_durable_through(default.at(1)).expect("seal");
    let rest: Vec<&str> = (0..3).filter_map(|_| rx.recv_timeout(BOUND).ok()).collect();
    let expected = ["failed, abandoned from the handler", "abandoned", "durable"];
    if op != Ok("op") || rest != expected {
        // As below: a handler stuck on the instance's lock must fail the test, not hang its drop.
        std::mem::forget(delivery);
        panic!("expected the write, then its failure, abandonment and Durable: {op:?} {rest:?}");
    }
    assert!(delivery.failures().is_empty());
    assert_eq!(delivery.epoch_state(epoch), Ok(EpochState::Abandoned));
}

#[test]
fn the_handler_can_push_through_the_handle_it_is_given() {
    let temp = temp();
    let (tx, rx) = mpsc::channel();
    let delivery = EntryDelivery::new(
        instance(&temp, Vec::new()),
        move |entry: RingEntry, handle| {
            let context = context(&entry);
            if context == 0 {
                handle
                    .read(FILE, 0, vec![0; 4], 1)
                    .expect("push from inside the handler");
            }
            let _ = tx.send(context);
        },
        None,
    )
    .expect("start delivery");
    delivery.read(FILE, 0, vec![0; 4], 0).expect("push a read");
    let delivered: Vec<u32> = (0..2).filter_map(|_| rx.recv_timeout(BOUND).ok()).collect();
    if delivered != [0, 1] {
        // A handler stuck on the instance's lock would make the drop wait for it forever: leak
        // the front end, so the regression fails instead of hanging.
        std::mem::forget(delivery);
        panic!("the handler's own push was not delivered: {delivered:?}");
    }
}

#[test]
fn handler_calls_never_overlap_while_threads_push_concurrently() {
    let temp = temp();
    let active = Arc::new(AtomicUsize::new(0));
    let widest = Arc::new(AtomicUsize::new(0));
    let (tx, rx) = mpsc::channel();
    let (in_handler, most) = (Arc::clone(&active), Arc::clone(&widest));
    let delivery = EntryDelivery::new(
        instance(&temp, Vec::new()),
        move |entry: RingEntry, _| {
            let now = in_handler.fetch_add(1, Ordering::SeqCst) + 1;
            most.fetch_max(now, Ordering::SeqCst);
            // Long enough for an overlapping call, were one possible, to land inside this one.
            std::thread::sleep(Duration::from_micros(200));
            in_handler.fetch_sub(1, Ordering::SeqCst);
            let _ = tx.send(context(&entry));
        },
        None,
    )
    .expect("start delivery");

    std::thread::scope(|scope| {
        for thread in 0..4_u32 {
            let delivery = &delivery;
            scope.spawn(move || {
                for i in 0..16_u32 {
                    delivery
                        .read(FILE, 0, vec![0; 16], thread * 100 + i)
                        .expect("push from another thread");
                }
            });
        }
    });
    let mut delivered: Vec<u32> = (0..64)
        .map(|_| rx.recv_timeout(BOUND).expect("a delivery"))
        .collect();
    delivered.sort_unstable();
    let expected: Vec<u32> = (0..4)
        .flat_map(|t| (0..16).map(move |i| t * 100 + i))
        .collect();
    assert_eq!(delivered, expected);
    assert_eq!(
        widest.load(Ordering::SeqCst),
        1,
        "no two handler calls ran at once"
    );
}

#[test]
fn into_inner_hands_back_an_instance_that_has_lost_nothing() {
    let temp = temp();
    let (delivery, rx) = forwarding(instance(&temp, Vec::new()));
    for i in 0..8 {
        delivery.read(FILE, 0, vec![0; 4], i).expect("push a read");
    }
    let mut ring = delivery.into_inner();

    // Each read was delivered before the handover back, or is still in the instance's queue.
    let started = Instant::now();
    let mut seen: Vec<u32> = rx.try_iter().map(|entry| context(&entry)).collect();
    while seen.len() < 8 {
        assert!(started.elapsed() < BOUND, "lost an entry: saw {seen:?}");
        match ring.pop().expect("pop") {
            Some(entry) => seen.push(context(&entry)),
            None => std::thread::sleep(Duration::from_millis(1)),
        }
    }
    assert!(
        rx.try_recv().is_err(),
        "nothing is delivered after into_inner"
    );
    seen.sort_unstable();
    assert_eq!(seen, (0..8).collect::<Vec<_>>());

    ring.read(FILE, 0, vec![0; 4], 9)
        .expect("the instance still takes pushes");
}

#[test]
fn dropping_with_operations_in_flight_returns() {
    let temp = temp();
    let (delivery, _rx) = forwarding(instance(&temp, Vec::new()));
    for i in 0..16 {
        delivery.read(FILE, 0, vec![0; 16], i).expect("push a read");
    }
    let (dropped, done) = mpsc::channel();
    std::thread::spawn(move || {
        drop(delivery);
        let _ = dropped.send(());
    });
    done.recv_timeout(BOUND).expect("the drop returned");
}

#[test]
fn a_refusal_through_the_handle_hands_back_what_it_took() {
    let temp = temp();
    let (delivery, rx) = forwarding(instance(&temp, Vec::new()));
    let error = delivery
        .read(FileKey(99), 0, vec![7], 3)
        .expect_err("an unknown file");
    assert!(matches!(
        error.reason,
        PushRefusal::UnknownFile(FileKey(99))
    ));
    assert_eq!((error.buffer, error.context), (Some(vec![7]), 3));
    let error = delivery
        .add_file(FILE, temp.open())
        .expect_err("a key in use");
    assert_eq!(error.key, FILE);
    assert!(
        rx.recv_timeout(Duration::from_millis(50)).is_err(),
        "a refused push delivers nothing"
    );
}

#[test]
fn registered_buffers_are_reached_through_the_handle() {
    let temp = temp();
    let (delivery, rx) = forwarding(instance(&temp, vec![vec![0; 32]; 2]));
    delivery
        .with_registered_buffer_mut(0, |bytes| bytes[..6].copy_from_slice(b"handle"))
        .expect("fill buffer 0");
    let default = delivery.default_lineage();
    let span = |buffer_index| RegisteredSpan {
        buffer_index,
        offset: 0,
        len: 6,
    };
    delivery
        .write_registered(FILE, 100, span(0), default.at(1), 1)
        .expect("push a registered write");
    assert_eq!(context(&rx.recv_timeout(BOUND).expect("the write")), 1);
    delivery
        .read_registered(FILE, 100, span(1), 2)
        .expect("push a registered read");
    assert_eq!(context(&rx.recv_timeout(BOUND).expect("the read")), 2);
    let read = delivery
        .with_registered_buffer(1, |bytes| bytes[..6].to_vec())
        .expect("read buffer 1");
    assert_eq!(read, b"handle");
    assert!(
        delivery.with_registered_buffer(2, |_| ()).is_err(),
        "out of range"
    );
}

#[test]
fn the_handle_reports_the_instances_lineages() {
    let temp = temp();
    let (delivery, _rx) = forwarding(instance(&temp, Vec::new()));
    let lineage = delivery.default_lineage().lineage();
    let lineages = delivery.lineages();
    assert_eq!(lineages.len(), 1);
    assert_eq!(lineages[0].lineage, lineage);
    assert_eq!(delivery.into_inner().default_lineage().lineage(), lineage);
}
