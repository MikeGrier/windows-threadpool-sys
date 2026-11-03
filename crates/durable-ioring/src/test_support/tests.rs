// Copyright (c) 2026 Mike Grier

use std::io;

use super::{ManualClock, SOURCE, Ticks, fault};
use crate::clock::StampClock;
use crate::error_code::ErrorCode;

#[test]
fn ticks_count_their_readings_from_zero() {
    let clock = Ticks::default();
    let readings: Vec<u64> = (0..5).map(|_| clock.now()).collect();
    assert_eq!(readings, [0, 1, 2, 3, 4]);
}

#[test]
fn clones_of_ticks_share_one_count() {
    let clock = Ticks::default();
    let other = clock.clone();
    assert_eq!(clock.now(), 0);
    assert_eq!(other.now(), 1);
    assert_eq!(clock.now(), 2);
}

#[test]
fn a_manual_clock_reads_what_it_was_set_to_and_reading_does_not_move_it() {
    let clock = ManualClock::at(1_000);
    assert_eq!(clock.now(), 1_000);
    assert_eq!(clock.now(), 1_000);
    assert_eq!(ManualClock::default().now(), 0);
}

#[test]
fn a_manual_clock_moves_by_the_amount_advanced_and_clones_share_the_move() {
    let clock = ManualClock::at(10);
    let held = clock.clone();
    clock.advance(5);
    assert_eq!(held.now(), 15);
    held.advance(0);
    assert_eq!(clock.now(), 15, "advancing by nothing changes nothing");
    held.advance(85);
    assert_eq!(clock.now(), 100);
}

#[test]
fn a_manual_clock_never_reads_less_than_before() {
    let clock = ManualClock::at(7);
    let mut last = clock.now();
    for step in [0, 1, 0, 3, 100] {
        clock.advance(step);
        let now = clock.now();
        assert!(now >= last, "{now} after {last}");
        last = now;
    }
}

#[test]
fn a_fault_names_the_test_source_and_the_code_an_os_error_carries() {
    let fault = fault(io::Error::from_raw_os_error(1117));
    assert_eq!(fault.source, SOURCE);
    assert_eq!(fault.code, Some(ErrorCode::from_hresult(1117)));
    assert_eq!(fault.code.map(ErrorCode::hresult), Some(1117));
    assert_eq!(
        fault
            .error
            .downcast_ref::<io::Error>()
            .and_then(io::Error::raw_os_error),
        Some(1117)
    );
}

#[test]
fn a_fault_for_an_error_without_an_os_code_carries_none() {
    let fault = fault(io::Error::other("a report with no code"));
    assert_eq!(fault.source, SOURCE);
    assert_eq!(fault.code, None);
}

#[test]
fn two_faults_made_from_equal_errors_are_not_equal_but_a_clone_is() {
    // `IoFault`'s equality is the shared report, so a test that compares faults must share one.
    let one = fault(io::Error::from_raw_os_error(5));
    let two = fault(io::Error::from_raw_os_error(5));
    assert_ne!(one, two);
    let cloned = one.clone();
    assert_eq!(one, cloned);
    assert_eq!(one.error, cloned.error, "one shared report");
}
