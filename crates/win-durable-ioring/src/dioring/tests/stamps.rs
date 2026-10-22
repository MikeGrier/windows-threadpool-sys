// Copyright (c) 2026 Mike Grier
//! Failure stamps (DI-D-38) through a real instance: one built with `with_clock` stamps with the
//! clock it was given, one built with `new` with interrupt time, and the inventory reports the
//! stamp the `Failed` entry carried. Every entry goes through the conformance oracle, which holds
//! the stamps to never decreasing in queue order.

use win_time_sys::{Clock, InterruptClock};

use super::MockClock;
use super::pushes::{Harness, instance, instance_with_clock};
use crate::dioring::TimeBase;
use durable_ioring::DurableRing;
use durable_ioring::{Entry, Failed, ImportScope};

type V = super::V;

fn next_failed<K: TimeBase>(harness: &mut Harness<K>) -> Failed<V> {
    match harness.next_entry() {
        Entry::Failed(failed) => failed,
        other => panic!("expected Failed, got {other:?}"),
    }
}

fn import<K: TimeBase>(harness: &mut Harness<K>) -> Failed<V> {
    harness
        .ring
        .import_failure(ImportScope::All)
        .expect("import");
    next_failed(harness)
}

#[test]
fn stamps_follow_the_clock_and_the_oracle_accepts_them_equal_or_rising() {
    let clock = MockClock::at(40);
    let mut harness = Harness::new(instance_with_clock(Vec::new(), Vec::new(), clock.clone()));
    let mut seen = vec![import(&mut harness).observed.ticks()];
    seen.push(import(&mut harness).observed.ticks());
    clock.advance(25);
    seen.push(import(&mut harness).observed.ticks());
    assert_eq!(seen, [40, 40, 65]);
    let inventory: Vec<u64> = harness
        .ring
        .failures()
        .iter()
        .map(|failure| failure.observed.ticks())
        .collect();
    assert_eq!(inventory, seen, "the inventory reports each entry's stamp");
    harness.finish();
}

#[test]
fn an_instance_built_with_new_stamps_with_interrupt_time() {
    let mut harness = Harness::new(instance(Vec::new(), Vec::new()));
    let before = InterruptClock.now();
    harness
        .ring
        .import_failure(ImportScope::All)
        .expect("import");
    let after = InterruptClock.now();
    let failed = next_failed(&mut harness);
    assert!(
        before <= failed.observed && failed.observed <= after,
        "{:?} outside {before:?}..={after:?}",
        failed.observed
    );
    harness.finish();
}
