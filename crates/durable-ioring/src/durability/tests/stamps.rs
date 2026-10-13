// Copyright (c) 2026 Mike Grier
//! Failure stamps (DI-D-38), without a ring: the core reads the clock it holds when it records a
//! failure, and reports that one reading on the `Failed` entry and in the inventory. That the
//! Windows driver's default clock is interrupt time is checked through a real instance, in
//! `dioring/tests/stamps.rs`.

use std::io;

use super::{A, DEFAULT, Due, FULL, One, Ops, Stamp, StampClocked, lineage, submitted};
use crate::durability::{Event, Reach};
use crate::ids::FailureToken;
use crate::test_support::ManualClock;
use crate::types::{FailureCause, ImportScope, Resolution};

type Mocked = One<ManualClock>;

fn mocked(clock: &ManualClock) -> Mocked {
    One::with_clock(clock.clone())
}

fn import<K: StampClocked>(lineage: &mut One<K>) -> Due<u64, &'static str, Stamp> {
    let cause = FailureCause::Imported {
        scope: ImportScope::All,
    };
    lineage.import(cause, &Reach::All).1
}

/// The stamps of the failures a change observed, in tick counts.
fn stamps(due: &Due<u64, &'static str, Stamp>) -> Vec<u64> {
    due.events
        .iter()
        .filter_map(|event| match event {
            Event::Failed(failed) => Some(failed.observed),
            _ => None,
        })
        .collect()
}

/// The inventory's stamps, in observation order.
fn inventory<K: StampClocked>(lineage: &One<K>) -> Vec<u64> {
    lineage
        .failures()
        .iter()
        .map(|failure| failure.observed)
        .collect()
}

#[test]
fn an_imported_failure_is_stamped_with_the_clocks_reading_when_it_is_observed() {
    let clock = ManualClock::at(1_000);
    let mut lineage = mocked(&clock);
    clock.advance(4_000);
    assert_eq!(stamps(&import(&mut lineage)), [5_000]);
}

#[test]
fn a_failed_flush_is_stamped_when_its_completion_is_observed_not_when_it_was_sealed() {
    let clock = ManualClock::at(100);
    let mut lineage = mocked(&clock);
    let mut ops = Ops::new();
    let write = ops.push(&mut lineage, 1, A, DEFAULT);
    lineage.completed(write, FULL);
    submitted(lineage.seal(1));
    clock.advance(900);
    let due = lineage.flushed(1, A, Err(io::Error::from_raw_os_error(1117)));
    assert_eq!(stamps(&due), [1_000]);
}

#[test]
fn failures_observed_later_carry_later_stamps_and_within_one_reading_equal_ones() {
    let clock = ManualClock::at(70);
    let mut lineage = mocked(&clock);
    let mut seen = stamps(&import(&mut lineage));
    seen.extend(stamps(&import(&mut lineage)));
    clock.advance(1);
    seen.extend(stamps(&import(&mut lineage)));
    assert_eq!(seen, [70, 70, 71]);
}

#[test]
fn the_inventory_reports_the_stamp_the_entry_carried_not_a_reading_taken_when_asked() {
    let clock = ManualClock::at(10);
    let mut lineage = mocked(&clock);
    let entry = stamps(&import(&mut lineage));
    clock.advance(90);
    assert_eq!(inventory(&lineage), entry);
    assert_eq!(inventory(&lineage), [10]);
}

#[test]
fn the_inventory_keeps_each_failures_own_stamp_in_observation_order() {
    let clock = ManualClock::at(10);
    let mut lineage = mocked(&clock);
    import(&mut lineage);
    clock.advance(10);
    import(&mut lineage);
    clock.advance(10);
    assert_eq!(inventory(&lineage), [10, 20]);
}

#[test]
fn a_healed_failure_keeps_its_stamp_while_its_heal_is_pending() {
    let clock = ManualClock::at(300);
    let mut lineage = mocked(&clock);
    // Suspected, so the failure holds the lineage and the heal waits for its next seal.
    Ops::new().push(&mut lineage, 1, A, DEFAULT);
    let token: FailureToken = import(&mut lineage)
        .events
        .into_iter()
        .find_map(|event| match event {
            Event::Failed(failed) => Some(failed.token),
            _ => None,
        })
        .expect("the import's token");
    clock.advance(50);
    lineage
        .resolve(vec![(token, Resolution::Heal)])
        .expect("heal");
    assert_eq!(inventory(&lineage), [300]);
}

#[test]
fn a_lineage_built_with_the_default_test_clock_stamps_from_zero() {
    let mut lineage = lineage();
    assert_eq!(stamps(&import(&mut lineage)), [0]);
}
