// Copyright (c) 2026 Mike Grier
//! Pumped delivery through the driver (DI-7.3, DI-D-55): an instance built with
//! [`DeliveryMode::Pumped`] has no thread draining it, so what it delivers it delivers from the
//! consumer's own calls to [`Dioring::pump`](crate::Dioring::pump), over a real ring and real
//! files. The delivery layer's own rules, over the in-memory backend, are `pump/tests.rs`.
//!
//! The thing to show is what is *absent*: nothing here relies on a thread pool, and an instance
//! nothing pumps delivers nothing. The negative half is a bounded look for something that must not
//! happen -- evidence, as its length is chosen -- so it is paired with the positive half, which no
//! timing window decides: the same instance, pumped, delivers.

use std::io;
use std::time::{Duration, Instant};

use win_sync_sys::{WaitOutcome, wait_for_single_object};

use super::pushes::{BOUND, GIVEN, Harness, Ring, RingEntry, given, temp};
use super::{keeper, setup};
use crate::contract::DurableRing;
use crate::dioring::{DeliveryMode, Setup};
use crate::types::{DurabilityRequest, Entry, EpochState};

/// How long a test looks for an entry that must not arrive.
const QUIET: Duration = Duration::from_millis(150);

/// A pumped instance over `files`.
fn pumped(files: Vec<crate::dioring::TargetSetup>) -> Harness {
    pumped_with(Setup {
        delivery: DeliveryMode::Pumped,
        ..setup(files, Vec::new(), None)
    })
}

fn pumped_with(setup: Setup<'static, 'static, u64, Vec<u8>>) -> Harness {
    let ring = Ring::new(setup).expect("build a pumped instance").0;
    assert!(
        ring.delivery.as_ref().expect("live").is_pumped(),
        "built as the setup said"
    );
    Harness::new(ring)
}

/// The next entry, pumping for it, within the bound.
fn next(harness: &mut Harness) -> RingEntry {
    let started = Instant::now();
    loop {
        if let Some(entry) = harness.ring.pop().expect("pop") {
            harness
                .oracle
                .observe(&entry)
                .expect("the oracle accepts the entry");
            return entry;
        }
        assert!(
            started.elapsed() < BOUND,
            "no entry within {BOUND:?}; {} outstanding",
            harness.oracle.outstanding()
        );
        harness
            .ring
            .pump(Some(Duration::from_millis(50)))
            .expect("pump");
    }
}

/// Whether the readiness event is set now. Taking it resets it, as the instance's consumer does.
fn readiness_set(harness: &mut Harness) -> bool {
    let event = harness.ring.readiness().expect("the readiness event");
    match wait_for_single_object(&event, Some(Duration::ZERO)).expect("a poll") {
        WaitOutcome::Signalled => true,
        WaitOutcome::TimedOut => false,
        other => panic!("an event ended a wait as {other:?}"),
    }
}

#[test]
fn a_pumped_instance_delivers_nothing_until_it_is_pumped() {
    let temp = temp(&[0; 64]);
    let mut harness = pumped(vec![given(GIVEN, &temp)]);
    let id = harness.write(GIVEN, 0, b"pumped", 1, 7);

    // Nothing drains it: no entry arrives, and nothing tells the consumer one might.
    let started = Instant::now();
    while started.elapsed() < QUIET {
        assert!(harness.ring.pop().expect("pop").is_none());
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!readiness_set(&mut harness), "no drain has set the signal");

    // The same instance, pumped, delivers it, and says so.
    let done = match next(&mut harness) {
        Entry::Op(done) => done,
        other => panic!("expected the write's completion, got {other:?}"),
    };
    assert_eq!(done.id, id);
    assert_eq!(done.context, 7);
    assert_eq!(super::pushes::transferred(&done), 6);
    assert_eq!(
        std::fs::read(&temp.0).expect("read the file")[..6],
        *b"pumped",
        "and it reached the file"
    );
    assert!(
        readiness_set(&mut harness),
        "the pump that drained it set the signal"
    );
    harness.finish();
}

#[test]
fn a_pump_that_took_completions_reports_entries_and_one_that_did_not_reports_none() {
    let temp = temp(&[0; 64]);
    let mut harness = pumped(vec![given(GIVEN, &temp)]);
    harness.write(GIVEN, 0, b"x", 1, 1);
    let started = Instant::now();
    let mut ready = false;
    while !ready {
        assert!(started.elapsed() < BOUND, "no pump took the completion");
        ready = harness
            .ring
            .pump(Some(Duration::from_millis(50)))
            .expect("pump");
    }
    harness.next();
    // Nothing left in flight: the next pump takes nothing, whatever woke it.
    assert!(
        !harness
            .ring
            .pump(Some(Duration::from_millis(20)))
            .expect("pump")
    );
    harness.finish();
}

#[test]
fn a_pump_drains_every_completion_the_ring_has() {
    let temp = temp(&[0; 256]);
    let mut harness = pumped(vec![given(GIVEN, &temp)]);
    let mut pushed = Vec::new();
    for i in 0..12_u8 {
        pushed.push(harness.write(GIVEN, u64::from(i) * 8, &[i; 8], 1, u32::from(i)));
    }
    let mut seen = Vec::new();
    for _ in 0..pushed.len() {
        match next(&mut harness) {
            Entry::Op(done) => seen.push(done.id),
            other => panic!("expected a completion, got {other:?}"),
        }
    }
    seen.sort_by_key(|id| format!("{id:?}"));
    pushed.sort_by_key(|id| format!("{id:?}"));
    assert_eq!(seen, pushed, "each once");
    harness.finish();
}

#[test]
fn a_seal_is_made_durable_through_pumps_alone() {
    let temp = temp(&[0; 64]);
    let mut harness = pumped(vec![given(GIVEN, &temp)]);
    harness.write(GIVEN, 0, b"log", 1, 1);
    assert!(matches!(next(&mut harness), Entry::Op(_)));
    assert_eq!(harness.seal(1), DurabilityRequest::Submitted);
    match next(&mut harness) {
        Entry::Durable { through } => assert_eq!(through, harness.epoch(1)),
        other => panic!("expected Durable, got {other:?}"),
    }
    assert_eq!(harness.state(1), EpochState::Durable);
    assert_eq!(harness.durable(), Some(1));
    harness.finish();
}

#[test]
fn the_provider_is_called_from_the_pump_and_not_only_from_a_pop() {
    // Sealed with its write in flight, so the write's completion is drained, and the seal's request
    // made, by a pump. Under pool delivery the drain is off the consumer's thread and the request
    // waits for a consumer call; here the pump is that call, so the request is made at once.
    let temp = temp(&[0; 16]);
    let (provider, kept) = keeper(&["log"]);
    let mut harness = pumped_with(Setup {
        delivery: DeliveryMode::Pumped,
        ..setup(
            vec![super::file(GIVEN.0, &temp, &["log"])],
            Vec::new(),
            provider,
        )
    });
    harness.write(GIVEN, 0, b"log", 1, 1);
    assert_eq!(harness.seal(1), DurabilityRequest::Submitted);

    let started = Instant::now();
    let request = loop {
        let asked = std::mem::take(&mut *kept.lock().expect("the requests")).pop();
        if let Some(request) = asked {
            break request;
        }
        assert!(started.elapsed() < BOUND, "no pump made the request");
        harness
            .ring
            .pump(Some(Duration::from_millis(50)))
            .expect("pump");
    };
    for part in request.domains {
        part.completion.succeed();
    }
    assert!(matches!(next(&mut harness), Entry::Op(_)));
    match next(&mut harness) {
        Entry::Durable { through } => assert_eq!(through, harness.epoch(1)),
        other => panic!("expected Durable, got {other:?}"),
    }
    harness.finish();
}

#[test]
fn an_answer_from_another_thread_reaches_a_pumped_instance() {
    // A provider's answer takes the engine's lock on whatever thread gives it, and sets the
    // readiness signal itself, so a pumped instance needs no pump to learn of it.
    let temp = temp(&[0; 16]);
    let (provider, kept) = keeper(&["log"]);
    let mut harness = pumped_with(Setup {
        delivery: DeliveryMode::Pumped,
        ..setup(
            vec![super::file(GIVEN.0, &temp, &["log"])],
            Vec::new(),
            provider,
        )
    });
    harness.write(GIVEN, 0, b"log", 1, 1);
    assert!(matches!(next(&mut harness), Entry::Op(_)));
    assert_eq!(harness.seal(1), DurabilityRequest::Submitted);
    let request = loop {
        if let Some(request) = std::mem::take(&mut *kept.lock().expect("the requests")).pop() {
            break request;
        }
        harness.ring.pop().expect("pop");
        std::thread::sleep(Duration::from_millis(1));
    };
    // The pumps above set it, and nothing has taken it: take it, so a set now is the answer's.
    let _ = readiness_set(&mut harness);
    assert!(!readiness_set(&mut harness));
    std::thread::spawn(move || {
        for part in request.domains {
            part.completion.succeed();
        }
    })
    .join()
    .expect("the answering thread");
    assert!(readiness_set(&mut harness), "the answer set the signal");
    match harness.ring.pop().expect("pop") {
        Some(Entry::Durable { through }) => assert_eq!(through, harness.epoch(1)),
        other => panic!("expected Durable with no pump, got {other:?}"),
    }
}

#[test]
fn pumping_an_instance_built_with_a_pool_is_refused() {
    let temp = temp(&[0; 16]);
    let mut harness = Harness::new(super::pushes::instance(
        vec![given(GIVEN, &temp)],
        Vec::new(),
    ));
    assert!(!harness.ring.delivery.as_ref().expect("live").is_pumped());
    let error = harness
        .ring
        .pump(Some(Duration::ZERO))
        .expect_err("the pool drains it");
    assert_eq!(error.kind(), io::ErrorKind::Unsupported);
    // And it is untouched: it still delivers, on the pool.
    harness.write(GIVEN, 0, b"pool", 1, 1);
    harness.next();
    harness.finish();
}

#[test]
fn a_pumped_instance_ends_with_work_outstanding() {
    let temp = temp(&[0; 64]);
    let mut harness = pumped(vec![given(GIVEN, &temp)]);
    harness.write(GIVEN, 0, b"closing", 1, 1);
    let leftovers = harness.ring.close();
    assert!(
        leftovers.held.is_empty() && leftovers.failures.is_empty(),
        "nothing was gated, and nothing failed"
    );
    // Dropped instead of closed, with the same work outstanding, which the ring runs down.
    let temp = super::pushes::temp(&[0; 64]);
    let mut harness = pumped(vec![given(GIVEN, &temp)]);
    harness.write(GIVEN, 0, b"dropping", 1, 1);
    drop(harness);
    assert!(!temp.is_held(), "the ring let go of the file");
}
