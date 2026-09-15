// Copyright (c) Mike Grier.

//! Tests for the pure lookup helpers.
//!
//! These decide whether the report prints a ratio or `--`, so they are worth
//! testing directly; none of them needs the 65-second measurement.

use super::*;

/// A `Run` with everything but the fields under test held constant.
fn run(shape: &'static str, producers: usize, pushes_per_second: f64) -> Run {
    Run {
        shape,
        producers,
        nanos_per_push: if pushes_per_second > 0.0 {
            1_000_000_000.0 / pushes_per_second
        } else {
            0.0
        },
        pushes_per_second,
        refusals: 0,
    }
}

fn observation(isolated: Vec<Run>, drained: Vec<Run>) -> Observation {
    Observation {
        isolated,
        drained,
        available_parallelism: Some(8),
    }
}

fn sample() -> Observation {
    observation(
        vec![
            run(shapes::BASELINE_FETCH_ADD, 1, 400_000_000.0),
            run(shapes::BASELINE_FETCH_ADD, 4, 800_000_000.0),
            run(shapes::RESERVING_MPSC, 1, 200_000_000.0),
            run(shapes::RESERVING_MPSC, 4, 100_000_000.0),
            run(shapes::SLOTWISE_MPSC, 1, 100_000_000.0),
        ],
        vec![
            run(shapes::RESERVING_MPSC, 1, 40_000_000.0),
            run(shapes::RESERVING_MPSC, 4, 10_000_000.0),
        ],
    )
}

#[test]
fn find_returns_the_row_matching_both_shape_and_producer_count() {
    let observed = sample();
    let found = observed
        .find(&observed.isolated, shapes::RESERVING_MPSC, 4)
        .expect("the row is present");
    assert_eq!(found.shape, shapes::RESERVING_MPSC);
    assert_eq!(found.producers, 4);
}

#[test]
fn find_distinguishes_rows_that_share_a_shape() {
    let observed = sample();
    let one = observed
        .find(&observed.isolated, shapes::RESERVING_MPSC, 1)
        .expect("present");
    let four = observed
        .find(&observed.isolated, shapes::RESERVING_MPSC, 4)
        .expect("present");
    assert_ne!(one.pushes_per_second, four.pushes_per_second);
}

#[test]
fn find_distinguishes_rows_that_share_a_producer_count() {
    let observed = sample();
    let reserving = observed
        .find(&observed.isolated, shapes::RESERVING_MPSC, 1)
        .expect("present");
    let slotwise = observed
        .find(&observed.isolated, shapes::SLOTWISE_MPSC, 1)
        .expect("present");
    assert_ne!(reserving.pushes_per_second, slotwise.pushes_per_second);
}

#[test]
fn find_returns_none_for_a_shape_that_was_not_measured() {
    let observed = sample();
    assert!(
        observed
            .find(&observed.isolated, shapes::CLAIM_WIDE, 1)
            .is_none()
    );
}

#[test]
fn find_returns_none_for_a_producer_count_that_was_not_measured() {
    let observed = sample();
    assert!(
        observed
            .find(&observed.isolated, shapes::RESERVING_MPSC, 32)
            .is_none()
    );
}

/// The regime is a parameter, so the same shape and count must not leak across.
#[test]
fn find_reads_only_the_regime_it_is_given() {
    let observed = sample();
    let isolated = observed
        .find(&observed.isolated, shapes::RESERVING_MPSC, 1)
        .expect("present in isolated");
    let drained = observed
        .find(&observed.drained, shapes::RESERVING_MPSC, 1)
        .expect("present in drained");
    assert_ne!(isolated.pushes_per_second, drained.pushes_per_second);
    assert!(
        observed
            .find(&observed.drained, shapes::SLOTWISE_MPSC, 1)
            .is_none(),
        "slotwise was measured only in the isolated regime here"
    );
}

#[test]
fn find_on_an_empty_regime_is_none_rather_than_a_panic() {
    let observed = observation(Vec::new(), Vec::new());
    assert!(
        observed
            .find(&observed.isolated, shapes::RESERVING_MPSC, 1)
            .is_none()
    );
}

#[test]
fn scaling_divides_the_many_producer_rate_by_the_one_producer_rate() {
    let observed = sample();
    // baseline: 800M at four producers against 400M at one.
    let scaled = observed
        .scaling(&observed.isolated, shapes::BASELINE_FETCH_ADD, 4)
        .expect("both rows present");
    assert!(
        (scaled - 2.0).abs() < f64::EPSILON,
        "expected 2.0, got {scaled}"
    );
}

/// The direction matters: a contended claim scales *below* one, and reporting
/// the reciprocal would turn the finding upside down.
#[test]
fn scaling_below_one_means_more_producers_pushed_fewer_items() {
    let observed = sample();
    let scaled = observed
        .scaling(&observed.isolated, shapes::RESERVING_MPSC, 4)
        .expect("both rows present");
    assert!(
        (scaled - 0.5).abs() < f64::EPSILON,
        "expected 0.5, got {scaled}"
    );
    assert!(scaled < 1.0, "this is what a contended claim looks like");
}

#[test]
fn scaling_at_one_producer_is_one_by_construction() {
    let observed = sample();
    let scaled = observed
        .scaling(&observed.isolated, shapes::RESERVING_MPSC, 1)
        .expect("the one-producer row is present");
    assert!(
        (scaled - 1.0).abs() < f64::EPSILON,
        "expected 1.0, got {scaled}"
    );
}

#[test]
fn scaling_is_none_when_the_one_producer_row_is_missing() {
    let observed = observation(
        vec![run(shapes::RESERVING_MPSC, 4, 100_000_000.0)],
        Vec::new(),
    );
    assert!(
        observed
            .scaling(&observed.isolated, shapes::RESERVING_MPSC, 4)
            .is_none(),
        "without the one-producer row there is nothing to scale against"
    );
}

#[test]
fn scaling_is_none_when_the_many_producer_row_is_missing() {
    let observed = observation(
        vec![run(shapes::RESERVING_MPSC, 1, 200_000_000.0)],
        Vec::new(),
    );
    assert!(
        observed
            .scaling(&observed.isolated, shapes::RESERVING_MPSC, 32)
            .is_none()
    );
}

#[test]
fn scaling_is_none_for_a_shape_absent_from_the_regime() {
    let observed = sample();
    assert!(
        observed
            .scaling(&observed.drained, shapes::BASELINE_FETCH_ADD, 4)
            .is_none()
    );
}

/// A zero denominator yields a non-finite value rather than a panic. That is
/// deliberate -- `scaling` is arithmetic, not a renderer -- and it is why
/// `format_scaling` in the binary must filter non-finite values before
/// formatting, or a degenerate observation prints `infx` in a column of
/// measurements. This test pins the half of that contract the library owns.
#[test]
fn scaling_against_a_zero_rate_is_non_finite_rather_than_a_panic() {
    let observed = observation(
        vec![
            run(shapes::RESERVING_MPSC, 1, 0.0),
            run(shapes::RESERVING_MPSC, 4, 100_000_000.0),
        ],
        Vec::new(),
    );
    let scaled = observed
        .scaling(&observed.isolated, shapes::RESERVING_MPSC, 4)
        .expect("both rows are present");
    assert!(!scaled.is_finite(), "expected non-finite, got {scaled}");
}

/// Every shape name must be distinct, or `find` would return whichever row
/// happened to come first and two columns would silently show one measurement.
#[test]
fn every_shape_name_is_distinct() {
    let names = [
        shapes::BASELINE_FETCH_ADD,
        shapes::SLOTWISE_MPSC,
        shapes::RESERVING_MPSC,
        shapes::PERMIT_MPSC,
        shapes::CLAIM_NARROW,
        shapes::CLAIM_DEEP,
        shapes::CLAIM_PERPETUAL,
        shapes::CLAIM_WIDE,
    ];
    for (index, name) in names.iter().enumerate() {
        assert!(
            !names[..index].contains(name),
            "{name} appears more than once"
        );
    }
}

/// `capacity_for` must leave room for every push, or the isolated regime would
/// refuse and stop being the regime it claims to be.
///
/// `>=` rather than `>`: a `bounded(n)` queue accepts exactly `n` items
/// (measured, not assumed), so an exact fit is sufficient. Today the product is
/// never a power of two, so the distinction is unreachable -- but M4.2 makes the
/// push count settable, and a stricter assertion than the contract requires would
/// reject a valid configuration then.
#[test]
fn capacity_for_leaves_room_for_every_push_at_every_producer_count() {
    for &producers in PRODUCER_COUNTS {
        let capacity = capacity_for(producers);
        let pushes = producers * PUSHES_PER_PRODUCER;
        assert!(
            capacity >= pushes,
            "{producers} producers push {pushes} but capacity is {capacity}"
        );
        assert!(
            capacity.is_power_of_two(),
            "{capacity} must be a power of two"
        );
    }
}
