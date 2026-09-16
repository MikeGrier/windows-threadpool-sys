// Copyright (c) Mike Grier.

//! Tests for the pure lookup helpers.
//!
//! These decide whether the report prints a ratio or `--`, so they are worth
//! testing directly; none of them needs the 65-second measurement.

use super::*;
use std::time::Duration;

/// A `Run` with everything but the fields under test held constant.
fn run(shape: &'static str, producers: usize, ops_per_second: f64) -> Run {
    Run {
        shape,
        producers,
        nanos_per_op: if ops_per_second > 0.0 {
            1_000_000_000.0 / ops_per_second
        } else {
            0.0
        },
        ops_per_second,
        refusals: 0,
        fastest_nanos_per_op: if ops_per_second > 0.0 {
            1_000_000_000.0 / ops_per_second
        } else {
            0.0
        },
        slowest_nanos_per_op: if ops_per_second > 0.0 {
            1_000_000_000.0 / ops_per_second
        } else {
            0.0
        },
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
    assert_ne!(one.ops_per_second, four.ops_per_second);
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
    assert_ne!(reserving.ops_per_second, slotwise.ops_per_second);
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
    assert_ne!(isolated.ops_per_second, drained.ops_per_second);
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

/// `measured_span` is the correction that round two of this branch's review
/// produced, and it had no test until round seventeen asked for one. The probe
/// previously timed from the coordinator's clock, which understated elapsed time
/// and overstated throughput by roughly 45% at high producer counts. These pin
/// the shape of the replacement: the span runs from the EARLIEST worker start to
/// the LATEST worker finish, so no worker's time is outside it.
///
/// `Instant` cannot be constructed from a literal, so each case builds one from
/// a single `now` and offsets it. That keeps the arithmetic exact without making
/// the test depend on how long it takes to run.
#[test]
fn measured_span_runs_from_the_earliest_start_to_the_latest_finish() {
    let base = Instant::now();
    let ms = Duration::from_millis(1);
    // Three workers, deliberately out of order and overlapping: the earliest
    // start belongs to the second, the latest finish to the third.
    let spans = vec![
        (base + 10 * ms, base + 40 * ms),
        (base + 5 * ms, base + 20 * ms),
        (base + 30 * ms, base + 60 * ms),
    ];
    let nanos = measured_span(&spans);
    // 5ms..60ms
    let expected = (55 * ms).as_nanos() as f64;
    assert!(
        (nanos - expected).abs() < f64::EPSILON,
        "expected {expected} ns, got {nanos}"
    );
}

/// The defect the correction replaced would have measured one worker's slice, or
/// the coordinator's view of it. Any narrower aggregation than min-start to
/// max-end is therefore what this guards against.
#[test]
fn measured_span_is_wider_than_any_single_worker() {
    let base = Instant::now();
    let ms = Duration::from_millis(1);
    let spans = vec![
        (base + 10 * ms, base + 20 * ms),
        (base + 15 * ms, base + 50 * ms),
        (base, base + 5 * ms),
    ];
    let nanos = measured_span(&spans);
    for (began, ended) in &spans {
        let worker = ended.duration_since(*began).as_nanos() as f64;
        assert!(
            nanos >= worker,
            "span {nanos} must cover every worker, but one ran {worker}"
        );
    }
    assert!(
        (nanos - (50 * ms).as_nanos() as f64).abs() < f64::EPSILON,
        "expected the full 0..50ms window, got {nanos}"
    );
}

#[test]
fn measured_span_of_one_worker_is_that_worker() {
    let base = Instant::now();
    let ms = Duration::from_millis(1);
    let nanos = measured_span(&[(base + 3 * ms, base + 11 * ms)]);
    assert!(
        (nanos - (8 * ms).as_nanos() as f64).abs() < f64::EPSILON,
        "expected 8ms, got {nanos}"
    );
}

/// Workers that never overlap still yield one span covering both, because the
/// question the probe asks is how long the whole configuration took.
#[test]
fn measured_span_covers_disjoint_workers() {
    let base = Instant::now();
    let ms = Duration::from_millis(1);
    let nanos = measured_span(&[(base, base + ms), (base + 100 * ms, base + 101 * ms)]);
    assert!(
        (nanos - (101 * ms).as_nanos() as f64).abs() < f64::EPSILON,
        "expected 101ms, got {nanos}"
    );
}

/// The renderer's cells. These were in the binary and therefore untestable until
/// they moved into this module; the non-finite case in particular is documented
/// by `scaling_against_a_zero_rate_is_non_finite_rather_than_a_panic` above and
/// was relying on that documentation rather than on a check.
#[test]
fn format_scaling_renders_a_finite_value_and_marks_everything_else() {
    assert_eq!(format_scaling(Some(1.0)), "1.00x");
    assert_eq!(format_scaling(Some(0.5)), "0.50x");
    assert_eq!(format_scaling(Some(12.345)), "12.35x");
    assert_eq!(format_scaling(None), "--");
    assert_eq!(format_scaling(Some(f64::INFINITY)), "--");
    assert_eq!(format_scaling(Some(f64::NEG_INFINITY)), "--");
    assert_eq!(format_scaling(Some(f64::NAN)), "--");
}

#[test]
fn format_ratio_divides_and_guards_its_denominator() {
    let fast = run(shapes::RESERVING_MPSC, 4, 200_000_000.0);
    let slow = run(shapes::SLOTWISE_MPSC, 4, 100_000_000.0);
    // slow is 10.0 ns/op, fast is 5.0, so fast/slow is 0.50x.
    assert_eq!(format_ratio(Some(fast), Some(slow)), "0.50x");
    assert_eq!(format_ratio(Some(slow), Some(fast)), "2.00x");
    assert_eq!(format_ratio(None, Some(slow)), "--");
    assert_eq!(format_ratio(Some(fast), None), "--");
    assert_eq!(format_ratio(None, None), "--");
}

/// A shape that failed to run reports zero nanoseconds, and dividing by it would
/// put `inf` in a column a reader takes for a measurement.
#[test]
fn format_ratio_refuses_a_zero_denominator() {
    let measured = run(shapes::RESERVING_MPSC, 4, 100_000_000.0);
    let absent = run(shapes::SLOTWISE_MPSC, 4, 0.0);
    assert_eq!(absent.nanos_per_op, 0.0, "the fixture must have zero cost");
    assert_eq!(format_ratio(Some(measured), Some(absent)), "--");
}

#[test]
fn format_nanos_renders_one_decimal_or_the_marker() {
    assert_eq!(
        format_nanos(Some(run(shapes::RESERVING_MPSC, 1, 1e9))),
        "1.0"
    );
    assert_eq!(format_nanos(None), "--");
}

#[test]
fn render_table_writes_a_header_and_one_line_per_run() {
    let rows = vec![
        run(shapes::BASELINE_FETCH_ADD, 1, 400_000_000.0),
        run(shapes::RESERVING_MPSC, 4, 100_000_000.0),
    ];
    let mut out = String::new();
    render_table(&mut out, &rows);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 3, "a header and two rows, got {out:?}");
    assert!(lines[0].contains("shape") && lines[0].contains("ns/op"));
    assert!(lines[1].contains(shapes::BASELINE_FETCH_ADD));
    assert!(lines[2].contains(shapes::RESERVING_MPSC));
    assert!(
        lines[2].contains("10.0"),
        "100M ops/sec is 10.0 ns/op, got {:?}",
        lines[2]
    );
}

#[test]
fn render_table_of_nothing_still_writes_its_header() {
    let mut out = String::new();
    render_table(&mut out, &[]);
    assert_eq!(out.lines().count(), 1, "header only, got {out:?}");
}

/// A timer that replays a scripted sequence instead of measuring anything, so
/// `median_run`'s selection can be checked exactly. The first value is consumed
/// by the untimed warmup pass.
fn scripted(values: Vec<Repetition>) -> impl FnMut(usize) -> Repetition {
    let mut next = 0usize;
    move |_producers| {
        let value = values[next];
        next += 1;
        value
    }
}

/// The scripted repetitions used by the tests below, in the order `median_run`
/// calls for them. Three properties are deliberate and each catches a different
/// regression:
///
/// - the durations are **not** in ascending order, so failing to sort at all
///   selects 5e6 rather than the median 3e6;
/// - the refusal counts are **not** monotonic in duration, so sorting by the
///   wrong tuple element also selects 5e6;
/// - no two durations are equal, so the median is unambiguous.
///
/// Sort *direction* is deliberately not covered, because it cannot be: with
/// `REPETITIONS == 5`, `results[REPETITIONS / 2]` is index 2 of five, which is
/// the median whether the sort ascends or descends. A test claiming to pin
/// direction here would pass under both and be theatre.
fn scripted_repetitions() -> Vec<Repetition> {
    vec![
        (999e6, 9_999), // warmup, discarded
        (4e6, 40),
        (1e6, 10),
        (5e6, 30),
        (2e6, 50),
        (3e6, 20),
    ]
}

#[test]
fn median_run_reports_the_median_repetition_rather_than_the_first_or_last() {
    let measured = median_run(shapes::RESERVING_MPSC, 1, scripted(scripted_repetitions()));
    // 3e6 ns over 1 * 50,000 pushes is 60 ns per op.
    assert!(
        (measured.nanos_per_op - 60.0).abs() < 1e-9,
        "expected the 3e6 ns median, got {} ns/op",
        measured.nanos_per_op
    );
    assert_eq!(measured.shape, shapes::RESERVING_MPSC);
    assert_eq!(measured.producers, 1);
}

/// The refusal count travels with the repetition whose duration was chosen. It
/// is the probe's only signal that a drained row was limited by the consumer
/// rather than by claim contention, so pairing it with a different repetition
/// would misattribute the cause while leaving the timing plausible.
#[test]
fn median_run_pairs_the_refusal_count_with_the_median_repetition() {
    let measured = median_run(shapes::RESERVING_MPSC, 1, scripted(scripted_repetitions()));
    assert_eq!(
        measured.refusals, 20,
        "3e6 ns is the median and its repetition refused 20; got {}",
        measured.refusals
    );
}

/// The warmup pass exists to take the first-call costs out of the sample, so its
/// value must not reach the report. Its scripted duration is the largest in the
/// sequence and its refusal count is unique, so either leaking into the result
/// is visible.
#[test]
fn median_run_discards_the_warmup_pass() {
    let measured = median_run(shapes::RESERVING_MPSC, 1, scripted(scripted_repetitions()));
    assert_ne!(
        measured.refusals, 9_999,
        "the warmup's refusals were reported"
    );
    assert!(
        measured.nanos_per_op < 100.0,
        "the warmup's 999e6 ns reached the report as {} ns/op",
        measured.nanos_per_op
    );
}

/// Both published rates come from the same chosen repetition, so they must agree
/// with each other. A regression that derived one from the median and the other
/// from some different element would leave a report whose two columns describe
/// different runs.
#[test]
fn median_run_derives_both_rates_from_the_one_chosen_repetition() {
    let measured = median_run(shapes::RESERVING_MPSC, 1, scripted(scripted_repetitions()));
    let round_trip = 1_000_000_000.0 / measured.nanos_per_op;
    assert!(
        (measured.ops_per_second - round_trip).abs() < 1e-3,
        "{} ops/sec does not agree with {} ns/op",
        measured.ops_per_second,
        measured.nanos_per_op
    );
}

/// `median_run` scales by the producer count, so the same per-repetition
/// durations must report a lower per-push cost when more producers shared them.
#[test]
fn median_run_divides_the_median_by_every_producers_pushes() {
    let one = median_run(shapes::RESERVING_MPSC, 1, scripted(scripted_repetitions()));
    let four = median_run(shapes::RESERVING_MPSC, 4, scripted(scripted_repetitions()));
    assert!(
        (one.nanos_per_op / four.nanos_per_op - 4.0).abs() < 1e-9,
        "four producers push four times as many items in the same span: {} vs {}",
        one.nanos_per_op,
        four.nanos_per_op
    );
}

/// The refusal column is the probe's diagnosis of *why* a drained row is slow --
/// consumer backpressure rather than claim contention -- so a row that dropped
/// or misformatted it would leave the report looking complete while the central
/// signal was silently absent. Every other renderer test builds rows refusing
/// nothing, which cannot catch that.
#[test]
fn render_table_shows_a_nonzero_refusal_count() {
    let mut refused = run(shapes::SLOTWISE_MPSC, 8, 100_000_000.0);
    refused.refusals = 123_456;
    let mut out = String::new();
    render_table(&mut out, &[refused]);
    let row = out.lines().nth(1).expect("one row was rendered");
    assert!(
        row.contains("123456"),
        "the refusal count is missing from {row:?}"
    );
    assert!(
        out.lines().next().expect("a header").contains("refusals"),
        "the refusal column is unlabelled"
    );
}

/// The dispersion `d-observations-not-verdicts` obliges the crate to publish.
/// `median_run` sorts ascending, so the extremes are the ends of that sort --
/// these pin that the reported range is the whole sample rather than, say, the
/// median repeated or two adjacent repetitions.
#[test]
fn median_run_carries_the_fastest_and_slowest_repetitions() {
    let measured = median_run(shapes::RESERVING_MPSC, 1, scripted(scripted_repetitions()));
    // Scripted timed repetitions are 1e6..5e6 ns over 50,000 pushes: 20..100 ns/op.
    assert!(
        (measured.fastest_nanos_per_op - 20.0).abs() < 1e-9,
        "expected the 1e6 ns repetition as fastest, got {} ns/op",
        measured.fastest_nanos_per_op
    );
    assert!(
        (measured.slowest_nanos_per_op - 100.0).abs() < 1e-9,
        "expected the 5e6 ns repetition as slowest, got {} ns/op",
        measured.slowest_nanos_per_op
    );
}

/// The median must lie inside the range, or the two are describing different
/// samples. This is the cheap invariant that catches a range computed from the
/// wrong vector or from an unsorted one.
#[test]
fn median_run_brackets_its_median_with_the_range() {
    let measured = median_run(shapes::RESERVING_MPSC, 1, scripted(scripted_repetitions()));
    assert!(
        measured.fastest_nanos_per_op <= measured.nanos_per_op,
        "fastest {} must not exceed the median {}",
        measured.fastest_nanos_per_op,
        measured.nanos_per_op
    );
    assert!(
        measured.nanos_per_op <= measured.slowest_nanos_per_op,
        "median {} must not exceed the slowest {}",
        measured.nanos_per_op,
        measured.slowest_nanos_per_op
    );
}

/// The warmup is excluded from the dispersion as well as from the median. Its
/// scripted 999e6 ns would otherwise dominate the range and make every row look
/// wildly unstable.
#[test]
fn median_run_excludes_the_warmup_from_the_range() {
    let measured = median_run(shapes::RESERVING_MPSC, 1, scripted(scripted_repetitions()));
    assert!(
        measured.slowest_nanos_per_op < 1_000.0,
        "the warmup's 999e6 ns reached the range as {} ns/op",
        measured.slowest_nanos_per_op
    );
}

#[test]
fn spread_is_the_slowest_over_the_fastest() {
    let mut run = run(shapes::RESERVING_MPSC, 4, 100_000_000.0);
    run.fastest_nanos_per_op = 10.0;
    run.slowest_nanos_per_op = 13.0;
    assert!((run.spread() - 1.3).abs() < 1e-9, "got {}", run.spread());
}

/// A configuration whose repetitions all took the same time has a spread of
/// exactly one, which is what "this host held still" looks like.
#[test]
fn spread_of_an_identical_sample_is_one() {
    let mut run = run(shapes::RESERVING_MPSC, 4, 100_000_000.0);
    run.fastest_nanos_per_op = 42.0;
    run.slowest_nanos_per_op = 42.0;
    assert!((run.spread() - 1.0).abs() < 1e-9, "got {}", run.spread());
}

/// A shape that failed to run reports zero, and dividing by it would put `inf`
/// in a column a reader takes for a measurement -- the same guard
/// `format_ratio` carries.
#[test]
fn spread_of_a_zero_sample_is_zero_rather_than_infinite() {
    let mut run = run(shapes::SLOTWISE_MPSC, 4, 0.0);
    run.fastest_nanos_per_op = 0.0;
    run.slowest_nanos_per_op = 0.0;
    assert_eq!(run.spread(), 0.0);
    assert!(
        run.spread().is_finite(),
        "the spread must never be infinite"
    );
}

#[test]
fn render_table_publishes_the_range_and_the_spread() {
    let mut row = run(shapes::RESERVING_MPSC, 8, 100_000_000.0);
    row.fastest_nanos_per_op = 9.5;
    row.slowest_nanos_per_op = 12.5;
    let mut out = String::new();
    render_table(&mut out, &[row]);
    let header = out.lines().next().expect("a header");
    let line = out.lines().nth(1).expect("one row");
    assert!(
        header.contains("range") && header.contains("spread"),
        "the dispersion columns are unlabelled: {header:?}"
    );
    assert!(
        line.contains("9.5-12.5"),
        "the range is missing from {line:?}"
    );
    assert!(
        line.contains("1.32x"),
        "the spread is missing from {line:?}"
    );
}

/// Helper: a row with an explicit cost span, for the bound arithmetic.
fn run_spanning(
    shape: &'static str,
    producers: usize,
    fastest: f64,
    median: f64,
    slowest: f64,
) -> Run {
    Run {
        shape,
        producers,
        nanos_per_op: median,
        ops_per_second: if median > 0.0 {
            1_000_000_000.0 / median
        } else {
            0.0
        },
        refusals: 0,
        fastest_nanos_per_op: fastest,
        slowest_nanos_per_op: slowest,
    }
}

/// The bound pairs each side's extreme against the other's opposite extreme,
/// because that is the widest the ratio could be. Rate ratio inverts cost, so
/// the smallest rate ratio is the numerator at its slowest against the
/// denominator at its fastest.
#[test]
fn ratio_bounds_pairs_opposing_extremes() {
    let numerator = run_spanning(shapes::RESERVING_MPSC, 4, 8.0, 10.0, 12.0);
    let denominator = run_spanning(shapes::SLOTWISE_MPSC, 4, 40.0, 50.0, 60.0);
    let (low, high) = ratio_bounds(numerator, denominator).expect("both spans are positive");
    // rate ratio low  = den.fastest / num.slowest = 40 / 12
    // rate ratio high = den.slowest / num.fastest = 60 / 8
    assert!((low - (40.0 / 12.0)).abs() < 1e-9, "low was {low}");
    assert!((high - (60.0 / 8.0)).abs() < 1e-9, "high was {high}");
}

/// The invariant that makes the bound meaningful: whatever point estimate the
/// medians produce must lie inside it. A bound that excluded its own point
/// estimate would be arithmetic nobody should trust.
#[test]
fn ratio_bounds_contain_the_point_estimate() {
    let numerator = run_spanning(shapes::RESERVING_MPSC, 4, 8.0, 10.0, 12.0);
    let denominator = run_spanning(shapes::SLOTWISE_MPSC, 4, 40.0, 50.0, 60.0);
    let point = denominator.nanos_per_op / numerator.nanos_per_op;
    let (low, high) = ratio_bounds(numerator, denominator).expect("both spans are positive");
    assert!(
        low <= point && point <= high,
        "the point estimate {point} falls outside its own bound [{low}, {high}]"
    );
}

/// A row whose span touches zero cannot be divided by, the same case
/// `format_ratio` and `spread` already guard.
#[test]
fn ratio_bounds_of_a_zero_span_is_none() {
    let real = run_spanning(shapes::RESERVING_MPSC, 4, 8.0, 10.0, 12.0);
    let absent = run_spanning(shapes::SLOTWISE_MPSC, 4, 0.0, 0.0, 0.0);
    assert!(ratio_bounds(real, absent).is_none());
    assert!(ratio_bounds(absent, real).is_none());
}

/// A configuration whose repetitions all agreed gives a bound of zero width,
/// which is what "this host held still" looks like for a derived figure.
#[test]
fn ratio_bounds_of_two_exact_samples_is_a_point() {
    let numerator = run_spanning(shapes::RESERVING_MPSC, 4, 10.0, 10.0, 10.0);
    let denominator = run_spanning(shapes::SLOTWISE_MPSC, 4, 50.0, 50.0, 50.0);
    let (low, high) = ratio_bounds(numerator, denominator).expect("positive spans");
    assert!(
        (low - 5.0).abs() < 1e-9 && (high - 5.0).abs() < 1e-9,
        "[{low},{high}]"
    );
}

#[test]
fn format_ratio_bounded_renders_the_point_and_its_interval() {
    let numerator = run_spanning(shapes::RESERVING_MPSC, 4, 8.0, 10.0, 12.0);
    let denominator = run_spanning(shapes::SLOTWISE_MPSC, 4, 40.0, 50.0, 60.0);
    let rendered = format_ratio_bounded(Some(numerator), Some(denominator));
    assert!(
        rendered.starts_with("0.20x ["),
        "expected the point estimate first, got {rendered:?}"
    );
    assert!(
        rendered.contains('[') && rendered.contains(']'),
        "the bound must be bracketed to mark it as not a sampled range: {rendered:?}"
    );
}

#[test]
fn format_ratio_bounded_marks_a_missing_or_zero_row() {
    let real = run_spanning(shapes::RESERVING_MPSC, 4, 8.0, 10.0, 12.0);
    let zero = run_spanning(shapes::SLOTWISE_MPSC, 4, 0.0, 0.0, 0.0);
    assert_eq!(format_ratio_bounded(None, Some(real)), "--");
    assert_eq!(format_ratio_bounded(Some(real), None), "--");
    assert_eq!(format_ratio_bounded(Some(real), Some(zero)), "--");
}

#[test]
fn format_scaling_bounded_renders_point_and_interval_or_the_marker() {
    assert_eq!(
        format_scaling_bounded(Some(2.0), Some((1.5, 2.5))),
        "2.00x [1.50-2.50]"
    );
    assert_eq!(format_scaling_bounded(None, Some((1.5, 2.5))), "--");
    assert_eq!(
        format_scaling_bounded(Some(f64::NAN), Some((1.0, 2.0))),
        "--"
    );
    assert_eq!(
        format_scaling_bounded(Some(f64::INFINITY), Some((1.0, 2.0))),
        "--"
    );
    // A point estimate with no computable bound still renders, unbracketed.
    assert_eq!(format_scaling_bounded(Some(2.0), None), "2.00x");
}

#[test]
fn scaling_bounds_reads_the_one_and_many_producer_rows() {
    let observation = Observation {
        isolated: vec![
            run_spanning(shapes::RESERVING_MPSC, 1, 4.0, 5.0, 6.0),
            run_spanning(shapes::RESERVING_MPSC, 8, 40.0, 50.0, 60.0),
        ],
        drained: Vec::new(),
        available_parallelism: Some(8),
    };
    let (low, high) = observation
        .scaling_bounds(&observation.isolated, shapes::RESERVING_MPSC, 8)
        .expect("both rows present with positive spans");
    let point = observation
        .scaling(&observation.isolated, shapes::RESERVING_MPSC, 8)
        .expect("both rows present");
    assert!(
        low <= point && point <= high,
        "scaling {point} outside its bound [{low}, {high}]"
    );
}

#[test]
fn scaling_bounds_is_none_when_a_row_is_missing() {
    let observation = Observation {
        isolated: vec![run_spanning(shapes::RESERVING_MPSC, 8, 40.0, 50.0, 60.0)],
        drained: Vec::new(),
        available_parallelism: Some(8),
    };
    assert!(
        observation
            .scaling_bounds(&observation.isolated, shapes::RESERVING_MPSC, 8)
            .is_none(),
        "the one-producer row is absent, so no bound exists"
    );
}
