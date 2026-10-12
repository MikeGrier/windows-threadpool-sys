// Copyright (c) 2026 Mike Grier
//! Signalled delivery through the driver (DI-7.5, DI-D-55): an instance built with
//! [`DeliveryMode::Signalled`] is drained by its consumer's calls to
//! [`Dioring::pump`](crate::Dioring::pump), and the thread pool tells the consumer, through an event
//! of the instance's own, when completions may be waiting -- over a real ring and real files.
//!
//! The signal is a tee: the ring crate's wake owns the ring's event and sets the consumer's, so
//! these tests wait on it and set and reset it without touching the ring's. Setting it as the wake
//! is made, and twice for each activation, is the protocol, so a test that needs a quiet event
//! first takes what has been set.

use std::io;
use std::time::{Duration, Instant};

use win_sync_sys::{Event, WaitOutcome, wait_for_single_object};

use super::pumped::{next, readiness_set};
use super::pushes::{BOUND, GIVEN, Harness, Ring, given, temp};
use super::setup;
use crate::contract::DurableRing;
use crate::dioring::{DeliveryMode, Setup};
use crate::types::Entry;

/// How long a test looks for a signal that must not come.
const QUIET: Duration = Duration::from_millis(100);

/// A signalled instance over `files`.
fn signalled(files: Vec<crate::dioring::TargetSetup>) -> Harness {
    let ring = Ring::new(Setup {
        delivery: DeliveryMode::Signalled(None),
        ..setup(files, Vec::new(), None)
    })
    .expect("build a signalled instance")
    .0;
    assert!(
        ring.delivery.as_ref().expect("live").is_signalled(),
        "built as the setup said"
    );
    Harness::new(ring)
}

/// Whether `signal` is set, waiting up to `timeout` for it.
fn set_within(signal: &Event, timeout: Duration) -> bool {
    match wait_for_single_object(signal, Some(timeout)).expect("a wait") {
        WaitOutcome::Signalled => true,
        WaitOutcome::TimedOut => false,
        other => panic!("an event ended a wait as {other:?}"),
    }
}

/// Take what the wake has set so far, until it is quiet.
fn settle(signal: &Event) {
    for _ in 0..50 {
        if !set_within(signal, QUIET) {
            return;
        }
    }
    panic!("the signal never went quiet");
}

#[test]
fn a_signalled_instance_delivers_nothing_until_the_consumer_drains() {
    let temp = temp(&[0; 64]);
    let mut harness = signalled(vec![given(GIVEN, &temp)]);
    let signal = harness.ring.completion_signal().expect("the signal");
    settle(&signal);
    let id = harness.write(GIVEN, 0, b"signal", 1, 7);

    // The pool says so, and nothing drains: the consumer's own call does.
    assert!(
        set_within(&signal, BOUND),
        "a completion arrived and the pool said so"
    );
    let started = Instant::now();
    while started.elapsed() < QUIET {
        assert!(harness.ring.pop().expect("pop").is_none());
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        !readiness_set(&mut harness),
        "nothing has drained, so nothing is poppable"
    );

    assert!(
        harness.ring.pump(Some(Duration::ZERO)).expect("pump"),
        "one pump drains it"
    );
    let entry = harness.ring.pop().expect("pop").expect("an entry");
    harness
        .oracle
        .observe(&entry)
        .expect("the oracle accepts the entry");
    match entry {
        Entry::Op(done) => assert_eq!(done.id, id),
        other => panic!("expected the write's completion, got {other:?}"),
    }
    assert!(
        readiness_set(&mut harness),
        "the pump that drained it set the readiness signal"
    );
    harness.finish();
}

#[test]
fn the_signal_is_set_when_a_completion_arrives_and_not_before() {
    let temp = temp(&[0; 64]);
    let mut harness = signalled(vec![given(GIVEN, &temp)]);
    let signal = harness.ring.completion_signal().expect("the signal");
    settle(&signal);
    assert!(!set_within(&signal, QUIET), "nothing in flight, so quiet");
    harness.write(GIVEN, 0, b"arrives", 1, 1);
    assert!(set_within(&signal, BOUND), "the completion set it");
}

#[test]
fn a_consumers_wait_then_a_zero_pump_delivers() {
    let temp = temp(&[0; 64]);
    let mut harness = signalled(vec![given(GIVEN, &temp)]);
    let signal = harness.ring.completion_signal().expect("the signal");
    settle(&signal);
    harness.write(GIVEN, 0, b"consumer", 1, 1);
    // What a consumer waiting beside other handles does: wake on the signal, then pump without
    // waiting, then pop.
    assert!(set_within(&signal, BOUND));
    let started = Instant::now();
    let mut ready = false;
    while !ready {
        assert!(started.elapsed() < BOUND, "no pump took the completion");
        ready = harness.ring.pump(Some(Duration::ZERO)).expect("pump");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(matches!(
        harness.ring.pop().expect("pop"),
        Some(Entry::Op(_))
    ));
}

#[test]
fn a_pump_after_the_consumer_took_the_signal_still_delivers_at_once() {
    let temp = temp(&[0; 64]);
    let mut harness = signalled(vec![given(GIVEN, &temp)]);
    let signal = harness.ring.completion_signal().expect("the signal");
    settle(&signal);
    harness.write(GIVEN, 0, b"taken", 1, 1);
    // The consumer's own wait takes the signal the completion set.
    assert!(set_within(&signal, BOUND));
    let started = Instant::now();
    let ready = harness.ring.pump(Some(BOUND)).expect("pump");
    assert!(ready, "what the signal announced is delivered");
    assert!(started.elapsed() < BOUND, "took {:?}", started.elapsed());
}

#[test]
fn a_consumer_that_resets_the_signal_costs_nothing_but_its_own_wakeup() {
    let temp = temp(&[0; 64]);
    let mut harness = signalled(vec![given(GIVEN, &temp)]);
    let signal = harness.ring.completion_signal().expect("the signal");
    let id = harness.write(GIVEN, 0, b"reset", 1, 1);
    for _ in 0..10 {
        signal.reset().expect("the consumer's handle may reset it");
        std::thread::sleep(Duration::from_millis(2));
    }
    match next(&mut harness) {
        Entry::Op(done) => assert_eq!(done.id, id),
        other => panic!("expected the write's completion, got {other:?}"),
    }
    harness.finish();
}

#[test]
fn only_a_signalled_instance_has_a_signal_and_each_call_is_the_same_event() {
    let temp = temp(&[0; 16]);
    let pooled = Harness::new(super::pushes::instance(
        vec![given(GIVEN, &temp)],
        Vec::new(),
    ));
    let error = pooled
        .ring
        .completion_signal()
        .expect_err("a pool has none");
    assert_eq!(error.kind(), io::ErrorKind::Unsupported);

    let temp = super::pushes::temp(&[0; 16]);
    let harness = signalled(vec![given(GIVEN, &temp)]);
    let first = harness.ring.completion_signal().expect("a signal");
    let second = harness.ring.completion_signal().expect("a signal");
    settle(&first);
    settle(&second);
    first.set().expect("set it through one duplicate");
    assert!(
        set_within(&second, BOUND),
        "and the other sees it: one event"
    );
}

#[test]
fn a_signalled_instance_delivers_on_the_pool_its_environment_names() {
    // That the pool named is the one the callback runs on is a declared blind spot, as pool
    // delivery's is; this pins that an instance built with an environment delivers at all.
    use windows_threadpool_sys::callback_env::CallbackEnviron;
    use windows_threadpool_sys::pool::ThreadpoolPool;

    let threads = ThreadpoolPool::new().expect("create a pool");
    let mut env = CallbackEnviron::new();
    env.set_pool(&threads);
    let file = temp(&[0; 16]);
    let ring = Ring::new(Setup {
        delivery: DeliveryMode::Signalled(Some(&mut env)),
        ..setup(vec![given(GIVEN, &file)], Vec::new(), None)
    })
    .expect("build")
    .0;
    let mut harness = Harness::new(ring);
    let signal = harness.ring.completion_signal().expect("the signal");
    settle(&signal);
    harness.write(GIVEN, 0, b"env", 1, 1);
    assert!(
        set_within(&signal, BOUND),
        "the named pool's callback set the signal"
    );
    assert!(matches!(next(&mut harness), Entry::Op(_)));
    // The instance ends first, then `env`, then the pool it names -- the reverse of declaration.
    harness.finish();
}

#[test]
fn a_signalled_instance_ends_with_work_outstanding_and_the_signal_outlives_it() {
    let temp = temp(&[0; 64]);
    let mut harness = signalled(vec![given(GIVEN, &temp)]);
    let signal = harness.ring.completion_signal().expect("the signal");
    harness.write(GIVEN, 0, b"closing", 1, 1);
    let leftovers = harness.ring.close();
    assert!(leftovers.held.is_empty() && leftovers.failures.is_empty());
    // A duplicate the consumer kept is a handle to an event like any other: still valid, and
    // nothing sets it any more.
    signal.set().expect("the consumer's handle is still good");

    let temp = super::pushes::temp(&[0; 64]);
    let mut harness = signalled(vec![given(GIVEN, &temp)]);
    harness.write(GIVEN, 0, b"dropping", 1, 1);
    drop(harness);
    assert!(!temp.is_held(), "the ring let go of the file");
}
