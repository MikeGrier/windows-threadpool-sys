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
    assert!(
        (run.spread().expect("a measured row") - 1.3).abs() < 1e-9,
        "got {:?}",
        run.spread()
    );
}

/// A configuration whose repetitions all took the same time has a spread of
/// exactly one, which is what "this host held still" looks like.
#[test]
fn spread_of_an_identical_sample_is_one() {
    let mut run = run(shapes::RESERVING_MPSC, 4, 100_000_000.0);
    run.fastest_nanos_per_op = 42.0;
    run.slowest_nanos_per_op = 42.0;
    assert!(
        (run.spread().expect("a measured row") - 1.0).abs() < 1e-9,
        "got {:?}",
        run.spread()
    );
}

/// A shape that failed to run reports zero, and dividing by it would put `inf`
/// in a column a reader takes for a measurement -- the same guard
/// `format_ratio` carries.
/// A row that never ran has no spread to report. It used to answer `0.0` here,
/// which the report rendered as `0.00x` -- the most reassuring value the column
/// can hold, meaning "perfectly stable", produced by a shape that measured
/// nothing. `None` is the honest answer and the renderer turns it into `--`.
#[test]
fn spread_of_a_zero_sample_is_none_rather_than_a_reassuring_number() {
    let mut run = run(shapes::SLOTWISE_MPSC, 4, 0.0);
    run.fastest_nanos_per_op = 0.0;
    run.slowest_nanos_per_op = 0.0;
    assert_eq!(run.spread(), None, "an unmeasured row has no spread");
    assert_eq!(
        format_scaling(run.spread()),
        "--",
        "and it must not render as a number"
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
    // cost ratio low  = num.fastest / den.slowest =  8 / 60
    // cost ratio high = num.slowest / den.fastest = 12 / 40
    assert!((low - (8.0 / 60.0)).abs() < 1e-9, "low was {low}");
    assert!((high - (12.0 / 40.0)).abs() < 1e-9, "high was {high}");
}

/// The invariant that makes the bound meaningful: whatever point estimate the
/// medians produce must lie inside it. A bound that excluded its own point
/// estimate would be arithmetic nobody should trust.
#[test]
fn ratio_bounds_contain_the_point_estimate() {
    let numerator = run_spanning(shapes::RESERVING_MPSC, 4, 8.0, 10.0, 12.0);
    let denominator = run_spanning(shapes::SLOTWISE_MPSC, 4, 40.0, 50.0, 60.0);
    let point = numerator.nanos_per_op / denominator.nanos_per_op;
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
    // Cost ratio: 10 over 50.
    assert!(
        (low - 0.2).abs() < 1e-9 && (high - 0.2).abs() < 1e-9,
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

/// Zero is the sentinel for "this shape did not run", and it is as meaningful on
/// the numerator side as on the denominator. A zero denominator would render
/// `inf`; a zero numerator renders `0.00x`, which is worse -- `inf` announces
/// itself as broken, and `0.00x` reads as a shape that was immeasurably fast.
#[test]
fn format_ratio_refuses_a_zero_numerator() {
    let measured = run(shapes::RESERVING_MPSC, 4, 100_000_000.0);
    let absent = run(shapes::SLOTWISE_MPSC, 4, 0.0);
    assert_eq!(absent.nanos_per_op, 0.0, "the fixture must have zero cost");
    assert_eq!(
        format_ratio(Some(absent), Some(measured)),
        "--",
        "a row that never ran must not render as a ratio"
    );
}

#[test]
fn format_ratio_bounded_refuses_a_zero_numerator() {
    let measured = run_spanning(shapes::RESERVING_MPSC, 4, 8.0, 10.0, 12.0);
    let absent = run_spanning(shapes::SLOTWISE_MPSC, 4, 0.0, 0.0, 0.0);
    assert_eq!(
        format_ratio_bounded(Some(absent), Some(measured)),
        "--",
        "a row that never ran must not render as a ratio"
    );
}

/// Both formatters agree about what is unmeasurable, in both positions. They are
/// separate functions with separate guards, which is exactly how one of them
/// came to guard only half the cases.
#[test]
fn both_ratio_formatters_reject_the_same_unmeasurable_rows() {
    let measured = run_spanning(shapes::RESERVING_MPSC, 4, 8.0, 10.0, 12.0);
    let absent = run_spanning(shapes::SLOTWISE_MPSC, 4, 0.0, 0.0, 0.0);
    for (numerator, denominator) in [
        (Some(absent), Some(measured)),
        (Some(measured), Some(absent)),
        (Some(absent), Some(absent)),
        (None, Some(measured)),
        (Some(measured), None),
    ] {
        assert_eq!(
            format_ratio(numerator, denominator),
            "--",
            "format_ratio accepted an unmeasurable pair"
        );
        assert_eq!(
            format_ratio_bounded(numerator, denominator),
            "--",
            "format_ratio_bounded accepted an unmeasurable pair"
        );
    }
}

/// The renderer is where the sentinel did its damage, so the guard is asserted
/// there and not only on the accessor. A row that measured nothing must show
/// `--` in the spread column rather than a number a reader would take for
/// stability.
#[test]
fn render_table_marks_an_unmeasured_spread_rather_than_printing_zero() {
    let mut absent = run(shapes::SLOTWISE_MPSC, 8, 0.0);
    absent.fastest_nanos_per_op = 0.0;
    absent.slowest_nanos_per_op = 0.0;
    let mut out = String::new();
    render_table(&mut out, &[absent]);
    let row = out.lines().nth(1).expect("one row was rendered");
    assert!(
        !row.contains("0.00x"),
        "an unmeasured row rendered a spread that reads as perfect stability: {row:?}"
    );
    assert!(
        row.contains("--"),
        "an unmeasured spread must be marked: {row:?}"
    );
}

/// The sentinel is one predicate now, so it is pinned directly.
///
/// [`Run::is_measured`] exists so a renderer cannot forget the test by writing
/// it slightly differently. That only helps if the predicate is itself right,
/// which a test routed through a formatter would not establish.
#[test]
fn is_measured_rejects_every_shape_of_unmeasured_row() {
    assert!(run(shapes::RESERVING_MPSC, 1, 1e9).is_measured());

    let mut row = run(shapes::RESERVING_MPSC, 1, 0.0);
    assert!(
        !row.is_measured(),
        "a zero cost is the did-not-run sentinel"
    );

    row.nanos_per_op = f64::NAN;
    assert!(!row.is_measured(), "NaN is not a measurement");

    row.nanos_per_op = f64::INFINITY;
    assert!(!row.is_measured(), "infinity is not a measurement");

    row.nanos_per_op = -1.0;
    assert!(!row.is_measured(), "a negative cost is not a measurement");
}

/// A row that did not run must not put a number in *any* measured column.
///
/// The test above this one checked the spread cell alone, and passed while the
/// very same row published `0.0` ns/op, `0` ops/sec and a `0.0-0.0` range --
/// three cells that read as a shape too fast to time. Checking one cell of a
/// row is what let the other four drift, so this asserts over the whole row.
#[test]
fn render_table_marks_every_measured_cell_of_a_row_that_did_not_run() {
    let mut absent = run(shapes::SLOTWISE_MPSC, 8, 0.0);
    absent.fastest_nanos_per_op = 0.0;
    absent.slowest_nanos_per_op = 0.0;
    let mut out = String::new();
    render_table(&mut out, &[absent]);
    let row = out.lines().nth(1).expect("one row was rendered");

    // Shape and producer count are configuration, not measurement: they are
    // known whether or not the row ran, and must survive.
    assert!(
        row.contains(shapes::SLOTWISE_MPSC),
        "the shape is configuration and must still be named: {row:?}"
    );
    assert!(
        row.contains('8'),
        "the producer count is configuration and must survive: {row:?}"
    );

    assert_eq!(
        row.matches("--").count(),
        5,
        "ns/op, ops/sec, refusals, range and spread must all be marked: {row:?}"
    );
    assert!(
        !row.contains("0.0"),
        "an unmeasured row published a number: {row:?}"
    );
}

/// A shape that did not run must not publish a cost.
#[test]
fn format_nanos_marks_a_row_that_did_not_run() {
    assert_eq!(format_nanos(Some(run(shapes::SLOTWISE_MPSC, 8, 0.0))), "--");

    let mut broken = run(shapes::SLOTWISE_MPSC, 8, 1e9);
    broken.nanos_per_op = f64::NAN;
    assert_eq!(format_nanos(Some(broken)), "--");
}

/// Zero scaling is the sentinel, and it is the half that renders plausibly.
///
/// [`Observation::scaling`] divides the many-producer rate by the one-producer
/// rate, so a many-producer row that did not run yields exactly `Some(0.0)`.
/// Rendered, that is `0.00x` in a column where values near `1` are the normal
/// reading -- it looks like a queue that failed to *scale* rather than one that
/// failed to *run*. The other sentinel, `infx`, at least announces itself.
#[test]
fn format_scaling_marks_a_zero_rather_than_publishing_it() {
    assert_eq!(format_scaling(Some(0.0)), "--");
    assert_eq!(format_scaling_bounded(Some(0.0), None), "--");
    assert_eq!(format_scaling_bounded(Some(0.0), Some((0.0, 0.0))), "--");
}

/// The whole path, so the guard is pinned where a reader would meet it.
///
/// The formatter tests above supply the sentinel by hand. This one makes the
/// probe's own arithmetic produce it, which is the only way to show the two
/// halves agree about what a did-not-run row looks like.
#[test]
fn a_many_producer_row_that_did_not_run_scales_to_the_marker() {
    let observation = Observation {
        isolated: vec![
            run(shapes::RESERVING_MPSC, 1, 1e8),
            run(shapes::RESERVING_MPSC, 8, 0.0),
        ],
        drained: Vec::new(),
        available_parallelism: Some(8),
    };
    let point = observation.scaling(&observation.isolated, shapes::RESERVING_MPSC, 8);
    assert_eq!(
        point,
        Some(0.0),
        "the sentinel reaches the renderer as a plain zero"
    );
    let bounds = observation.scaling_bounds(&observation.isolated, shapes::RESERVING_MPSC, 8);
    assert_eq!(
        format_scaling_bounded(point, bounds),
        "--",
        "a shape that never ran must not publish a scaling factor"
    );
}

/// The ratio column must fit every cell it is asked to hold.
///
/// A Rust width is a *minimum*, so a value wider than its field is not
/// truncated -- it pushes every column after it out of alignment with its
/// header, silently. The layout table allocated 10 characters to a formatter
/// whose ordinary output is 17, so the second and third ratio columns had been
/// rendering seven and fourteen characters adrift.
///
/// **An earlier version of this test asserted `cell.len() <= RATIO_COLUMN_WIDTH`,
/// and that invariant is not available.** The interval's endpoints are measured
/// spans, so the cell's width is a function of data and has no compile-time
/// bound; the test passed only because its fixtures happened to be narrow. The
/// width is now derived from the cells, and what is pinned is that derivation.
#[test]
fn ratio_column_fits_every_cell_it_must_hold() {
    let ordinary = format_ratio_bounded(
        Some(run_spanning(shapes::CLAIM_WIDE, 32, 40.0, 50.0, 60.0)),
        Some(run_spanning(shapes::CLAIM_NARROW, 32, 4.0, 5.0, 6.0)),
    );
    // The guard is only meaningful if the formatter really does emit the wide
    // point-and-interval form -- otherwise it would pass against a bare `1.00x`.
    assert!(
        ordinary.contains('['),
        "expected a bounded ratio, got {ordinary:?}"
    );

    // A slow outlier -- a 300 ms repetition against a 4 ns one, which is exactly
    // what `median_run` takes a median to survive -- overruns the floor.
    let outlier = format_ratio_bounded(
        Some(run_spanning(shapes::CLAIM_WIDE, 32, 40.0, 50.0, 6000.0)),
        Some(run_spanning(shapes::CLAIM_NARROW, 32, 4.0, 5.0, 6.0)),
    );
    assert!(
        outlier.len() > RATIO_COLUMN_WIDTH,
        "this fixture exists to exceed the floor; if it no longer does, the \
         case it guards has stopped being exercised: {outlier:?}"
    );

    let cells = [ordinary.as_str(), outlier.as_str(), "--"];
    let width = ratio_column_width(cells);
    for cell in cells {
        assert!(
            cell.len() <= width,
            "{cell:?} is {} characters and would push the next column out of \
             line with its header, which allows {width}",
            cell.len()
        );
    }
}

/// The floor applies when every cell is narrower than it.
#[test]
fn ratio_column_never_narrows_below_its_floor() {
    assert_eq!(ratio_column_width(["1.00x", "--"]), RATIO_COLUMN_WIDTH);
    assert_eq!(
        ratio_column_width(std::iter::empty()),
        RATIO_COLUMN_WIDTH,
        "a regime with no rows still needs a header that lines up"
    );
}

/// A row that is present but never ran must not be counted as measured.
///
/// The report says "N apportionments ... measured" above a table in which
/// `render_table` marks every cell of an unmeasured row `--`. Counting presence
/// rather than measurement let those two halves disagree: the sentence claimed
/// four layouts while the table showed one of them as having produced nothing.
#[test]
fn count_measured_counts_rows_that_ran_rather_than_rows_that_exist() {
    let layouts = [
        shapes::CLAIM_NARROW,
        shapes::CLAIM_DEEP,
        shapes::CLAIM_PERPETUAL,
        shapes::CLAIM_WIDE,
    ];
    let observation = Observation {
        isolated: vec![
            run(shapes::CLAIM_NARROW, 1, 1e8),
            run(shapes::CLAIM_DEEP, 1, 1e8),
            // Present, but carrying the did-not-run sentinel.
            run(shapes::CLAIM_PERPETUAL, 1, 0.0),
            // CLAIM_WIDE absent entirely, as it is when cfg-elided.
        ],
        drained: Vec::new(),
        available_parallelism: Some(8),
    };
    assert_eq!(
        observation.count_measured(&observation.isolated, &layouts, 1),
        2,
        "a present-but-unmeasured row must not be counted, and an absent one \
         must not be either"
    );
}

/// A `<= 0.0` test does not reject `NaN`, and this pins that it is rejected.
///
/// Every comparison against `NaN` is false, so the four-way `<= 0.0` guard this
/// function used to carry fell straight through to the division for a `NaN`
/// endpoint and produced `[NaN-NaN]`. The endpoints are public fields, so this
/// is reachable without going through `median_run`.
#[test]
fn ratio_bounds_rejects_non_finite_endpoints() {
    let sound = run_spanning(shapes::RESERVING_MPSC, 8, 4.0, 5.0, 6.0);
    assert!(
        ratio_bounds(sound, sound).is_some(),
        "the fixture must otherwise produce a bound, or this proves nothing"
    );

    // **All four endpoints, each on its own.** An earlier version of this test
    // poisoned `numerator.fastest` and `denominator.slowest` only -- the two
    // that feed the LOWER bound -- so dropping either of the upper bound's
    // endpoints from the guard would have left it green while `ratio_bounds`
    // returned a `NaN` high.
    /// A named span endpoint, so each can be poisoned independently.
    type Endpoint = (&'static str, fn(&mut Run, f64));
    let fields: [Endpoint; 2] = [
        ("fastest", |run, value| run.fastest_nanos_per_op = value),
        ("slowest", |run, value| run.slowest_nanos_per_op = value),
    ];
    for poison in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        for (field, set) in fields {
            let mut numerator = sound;
            set(&mut numerator, poison);
            assert_eq!(
                ratio_bounds(numerator, sound),
                None,
                "numerator.{field} = {poison} must not reach the division"
            );

            let mut denominator = sound;
            set(&mut denominator, poison);
            assert_eq!(
                ratio_bounds(sound, denominator),
                None,
                "denominator.{field} = {poison} must not reach the division"
            );

            // The medians are still sound, so the point estimate is publishable
            // and the *bound* is not. What must never appear is a bracket built
            // from a poisoned endpoint.
            for (label, rendered) in [
                (
                    "numerator",
                    format_ratio_bounded(Some(numerator), Some(sound)),
                ),
                (
                    "denominator",
                    format_ratio_bounded(Some(sound), Some(denominator)),
                ),
            ] {
                assert!(
                    !rendered.contains('['),
                    "{label}.{field} = {poison} rendered an interval: {rendered:?}"
                );
            }
        }
    }
}

/// `> 0.0` admits infinity, and a finite slowest over it is a spread of zero.
///
/// Zero is the reassuring end of the spread column, so this is the same class
/// of defect as the sentinel that reached the renderer earlier: a row that
/// measured nothing coherent reporting perfect stability.
#[test]
fn spread_rejects_non_finite_span_endpoints() {
    let mut row = run(shapes::RESERVING_MPSC, 8, 1e8);
    row.fastest_nanos_per_op = 10.0;
    row.slowest_nanos_per_op = 13.0;
    assert!(row.spread().is_some(), "the fixture must otherwise measure");

    let mut infinite_fastest = row;
    infinite_fastest.fastest_nanos_per_op = f64::INFINITY;
    assert_eq!(
        infinite_fastest.spread(),
        None,
        "an infinite fastest divides to a spread of zero, which reads as \
         perfect stability"
    );

    for poison in [f64::NAN, f64::INFINITY] {
        let mut broken = row;
        broken.slowest_nanos_per_op = poison;
        assert_eq!(broken.spread(), None, "a {poison} slowest has no spread");

        let mut broken = row;
        broken.fastest_nanos_per_op = poison;
        assert_eq!(broken.spread(), None, "a {poison} fastest has no spread");
    }
}

/// At one producer the bound is exact, because the row is divided by itself.
///
/// The report's first row is this case. Passing both spans to `ratio_bounds`
/// treats one measurement as two independent ones and manufactures an interval
/// around a quantity that is 1 by construction -- uncertainty invented by the
/// arithmetic rather than observed, in the most prominent row on the page.
#[test]
fn scaling_bounds_at_one_producer_is_exactly_one() {
    // A deliberately wide span: if the identity case were not special-cased,
    // this row would publish a correspondingly wide bound.
    let observation = Observation {
        isolated: vec![run_spanning(shapes::RESERVING_MPSC, 1, 4.0, 5.0, 6.0)],
        drained: Vec::new(),
        available_parallelism: Some(8),
    };
    assert_eq!(
        observation.scaling(&observation.isolated, shapes::RESERVING_MPSC, 1),
        Some(1.0),
        "the point estimate is one by construction"
    );
    assert_eq!(
        observation.scaling_bounds(&observation.isolated, shapes::RESERVING_MPSC, 1),
        Some((1.0, 1.0)),
        "and so is the bound; a wider one would be invented, not measured"
    );

    // The general case must keep its real bound, or this special case has
    // simply broken the function.
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
        .expect("both rows present");
    assert!(
        low < high,
        "a genuine comparison of two rows still spans an interval, got \
         [{low}, {high}]"
    );

    // A present-but-unmeasured one-producer row must not get the exact bound.
    // `find` returns it, so the identity case would otherwise report perfect
    // certainty about a shape that measured nothing.
    for absent in [0.0, f64::NAN, f64::INFINITY] {
        let mut row = run_spanning(shapes::RESERVING_MPSC, 1, 4.0, 5.0, 6.0);
        row.nanos_per_op = absent;
        let observation = Observation {
            isolated: vec![row],
            drained: Vec::new(),
            available_parallelism: Some(8),
        };
        assert_eq!(
            observation.scaling_bounds(&observation.isolated, shapes::RESERVING_MPSC, 1),
            None,
            "a row reporting {absent} has no scaling to bound"
        );
    }
}

/// Either row being unmeasured is enough; it does not take both.
///
/// Found by `cargo mutants`: replacing the `||` in that guard with `&&` survived
/// the suite, because every case written supplied the same row twice. A bound
/// over one real row and one that never ran is exactly the case the guard is
/// for, and nothing reached it.
#[test]
fn scaling_bounds_needs_both_rows_measured_not_merely_one() {
    let measured = run_spanning(shapes::RESERVING_MPSC, 1, 4.0, 5.0, 6.0);
    let mut absent = run_spanning(shapes::RESERVING_MPSC, 8, 40.0, 50.0, 60.0);
    absent.nanos_per_op = 0.0;

    let one_ran = Observation {
        isolated: vec![measured, absent],
        drained: Vec::new(),
        available_parallelism: Some(8),
    };
    assert_eq!(
        one_ran.scaling_bounds(&one_ran.isolated, shapes::RESERVING_MPSC, 8),
        None,
        "the many-producer row measured nothing, so there is no bound"
    );

    let mut absent_one = measured;
    absent_one.nanos_per_op = 0.0;
    let other_ran = Observation {
        isolated: vec![
            absent_one,
            run_spanning(shapes::RESERVING_MPSC, 8, 40.0, 50.0, 60.0),
        ],
        drained: Vec::new(),
        available_parallelism: Some(8),
    };
    assert_eq!(
        other_ran.scaling_bounds(&other_ran.isolated, shapes::RESERVING_MPSC, 8),
        None,
        "and the one-producer row measuring nothing is equally disqualifying"
    );
}

/// A non-finite bound is not printed, even beside a perfectly good point.
///
/// Found by `cargo mutants`: the `low.is_finite() && high.is_finite()` guard
/// could be replaced with `true`, or its `&&` with `||`, and the suite stayed
/// green -- every case gave the bound a finite pair or no pair at all, so the
/// guard was never asked to reject one. The point estimate is still
/// publishable in that case; only the interval is not.
#[test]
fn format_scaling_bounded_drops_a_non_finite_interval_and_keeps_the_point() {
    for (low, high) in [
        (f64::NAN, 2.0),
        (1.5, f64::NAN),
        (f64::NEG_INFINITY, 2.0),
        (1.5, f64::INFINITY),
    ] {
        let rendered = format_scaling_bounded(Some(2.0), Some((low, high)));
        assert_eq!(
            rendered, "2.00x",
            "[{low}, {high}] is not an interval, so only the point may be \
             published -- got {rendered:?}"
        );
    }

    // And a finite pair must still be printed, or the guard has simply been
    // turned into "never show an interval".
    assert_eq!(
        format_scaling_bounded(Some(2.0), Some((1.5, 2.5))),
        "2.00x [1.50-2.50]"
    );
}

/// The stop flag must be set on the path where nobody sets it explicitly.
///
/// The drained timers cleared the flag on the line after their producer scope,
/// which a producer's assertion unwinds straight past -- leaving the consumer
/// spinning on a flag nobody would ever set, its handle dropped unjoined, and a
/// core burning for the life of the process. The success path was never in
/// doubt; this pins the failure path, which is the one that was broken.
///
/// Note this asserts the *observable* effect through an `Arc` the guard does not
/// own, rather than reading the guard back: a consumer sees the flag through
/// exactly such a clone.
#[test]
fn stop_on_drop_sets_the_flag_when_the_producer_phase_unwinds() {
    let flag = Arc::new(AtomicBool::new(false));

    // The success path, for contrast.
    let observed = Arc::clone(&flag);
    {
        let _stop = StopOnDrop(Arc::clone(&flag));
        assert!(
            !observed.load(Ordering::Relaxed),
            "the flag must stay clear while the guard is alive, or a consumer \
             would stop before the producers had finished"
        );
    }
    assert!(
        observed.load(Ordering::Relaxed),
        "a normal drop must stop it"
    );

    // The path that was broken. `catch_unwind` prints the panic to stderr; no
    // panic hook is installed to silence it, because a hook is process-global
    // and this suite runs its tests as threads in one process.
    let unwound = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&unwound);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _stop = StopOnDrop(Arc::clone(&unwound));
        panic!("a producer failed, as one does when the consumer is gone");
    }));
    assert!(result.is_err(), "the fixture must actually unwind");
    assert!(
        observed.load(Ordering::Relaxed),
        "the consumer would spin forever on a flag nobody sets"
    );
}

/// Arrives at a gate on its own thread and hands back somewhere to read the
/// answer.
///
/// **Every gate test goes through this rather than calling `arrive_and_wait`
/// directly.** A gate regression parks its caller forever, so an assertion made
/// on the test thread would hang the whole suite -- which runs its tests as
/// threads in one process -- instead of failing it. The spawned thread is
/// deliberately never joined, for the same reason.
///
/// Two of these tests were first written the direct way, and sabotage caught
/// both: neutering `release` wedged the run rather than reddening it.
fn spawn_arrival(gate: &Arc<StartGate>) -> Arc<Mutex<Option<bool>>> {
    let gate = Arc::clone(gate);
    let outcome = Arc::new(Mutex::new(None));
    let observed = Arc::clone(&outcome);
    thread::spawn(move || {
        let complete = gate.arrive_and_wait();
        *observed.lock().expect("not poisoned") = Some(complete);
    });
    outcome
}

/// The answer from [`spawn_arrival`], or `None` if the party never came back.
fn await_arrival(outcome: &Arc<Mutex<Option<bool>>>) -> Option<bool> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(complete) = *outcome.lock().expect("not poisoned") {
            return Some(complete);
        }
        if Instant::now() >= deadline {
            return None;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

/// A complete party opens the gate and every member learns that it was complete.
#[test]
fn a_complete_party_opens_the_gate_for_everyone() {
    let gate = StartGate::new(3);
    let arrivals: Vec<_> = (0..4).map(|_| spawn_arrival(&gate)).collect();
    for outcome in &arrivals {
        assert_eq!(
            await_arrival(outcome),
            Some(true),
            "every member of a complete party must be freed, and told so"
        );
    }
}

/// A released gate frees parties waiting for arrivals that will never come.
///
/// This is the deadlock `M4.6` was queued for, reproduced without needing the
/// OS to refuse a thread: the gate is sized for a party that never completes,
/// which is what a panicking `Scope::spawn` leaves behind.
#[test]
fn a_released_gate_frees_parties_that_will_never_be_completed() {
    let gate = StartGate::new(2);
    let parked = spawn_arrival(&gate);

    // Let it reach the gate, then give up on the members that never arrive.
    thread::sleep(Duration::from_millis(50));
    gate.release();

    assert_eq!(
        await_arrival(&parked),
        Some(false),
        "the parked party must be freed and told the party was incomplete, so \
         a worker knows its run was abandoned"
    );
}

/// Arriving at an already-released gate reports the party incomplete.
///
/// The ordering matters: a worker spawned before the failure may arrive after
/// the coordinator has given up, and must reach the same conclusion as one that
/// was already parked.
#[test]
fn arriving_after_a_release_still_reports_an_incomplete_party() {
    let gate = StartGate::new(4);
    gate.release();
    for _ in 0..2 {
        assert_eq!(
            await_arrival(&spawn_arrival(&gate)),
            Some(false),
            "a late arrival must not be told the party completed, and the \
             answer must not change on a second look"
        );
    }
}

/// The guard releases the gate while its scope unwinds.
#[test]
fn release_on_drop_frees_the_gate_when_spawning_panics() {
    let gate = StartGate::new(8);
    let parked = spawn_arrival(&gate);
    thread::sleep(Duration::from_millis(50));

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _release = ReleaseOnDrop(Arc::clone(&gate));
        panic!("the OS refused a thread, as `Scope::spawn` does by panicking");
    }));
    assert!(result.is_err(), "the fixture must actually unwind");

    assert_eq!(
        await_arrival(&parked),
        Some(false),
        "the guard did not release the gate as its scope unwound"
    );
}

/// A producer does not start timing until the consumer says it is draining.
///
/// This is the window `M4.3` closed: the gate proves the consumer exists and is
/// scheduled, not that it has reached its first `pop`, so a producer released by
/// the gate could push into a queue nobody was draining yet -- an undrained
/// opening to a run whose whole subject is that it is drained.
///
/// Asserted by holding the flag clear and showing the producer stays put, then
/// setting it and showing the producer moves. A test that only set the flag
/// first would pass against a missing handshake, which is the shape that has
/// slipped through on this branch before.
#[test]
fn a_producer_waits_for_the_consumer_to_announce_that_it_is_draining() {
    let ready = Arc::new(AtomicBool::new(false));
    let waited = Arc::new(AtomicBool::new(false));

    let consumer_ready = Arc::clone(&ready);
    let observed = Arc::clone(&waited);
    // Detached rather than joined: a regression leaves this parked forever, and
    // joining would hang the suite instead of failing it.
    thread::spawn(move || {
        await_consumer(&consumer_ready);
        observed.store(true, Ordering::Release);
    });

    // While the consumer has not announced itself, the producer must not pass.
    thread::sleep(Duration::from_millis(100));
    assert!(
        !waited.load(Ordering::Acquire),
        "a producer started before the consumer was draining"
    );

    ready.store(true, Ordering::Release);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !waited.load(Ordering::Acquire) {
        assert!(
            Instant::now() < deadline,
            "the producer never observed the consumer's announcement"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

/// The handshake drains before it announces, not after.
///
/// The test above proves a producer waits for the announcement. It says nothing
/// about what the announcement means, so it would pass just as happily if the
/// consumer announced first and drained second -- and that ordering is the whole
/// of `M4.3`. Announcing first makes the flag mean "about to drain", which a
/// descheduling between the store and the first `pop` falsifies; draining first
/// makes it mean "has executed the pop path", which nothing can.
///
/// Reachable only because [`drain_then_announce`] states the ordering once. The
/// four drained timers that use it cannot be tested directly -- running one runs
/// the whole probe -- so a hand-written `pop`-then-`store` in each was four
/// copies of a guarantee nothing could check.
///
/// The fake records what the flag said *at the moment the pop ran*. If the store
/// had already happened, it sees `true`.
#[test]
fn the_handshake_drains_before_it_announces() {
    let ready = AtomicBool::new(false);
    let already_announced = AtomicBool::new(false);

    drain_then_announce(
        || already_announced.store(ready.load(Ordering::Acquire), Ordering::Release),
        &ready,
    );

    assert!(
        !already_announced.load(Ordering::Acquire),
        "readiness was published before the consumer drained, so a producer \
         released by it can push into a queue whose consumer has not run -- the \
         undrained opening M4.3 closed, in its narrower form"
    );
    assert!(
        ready.load(Ordering::Acquire),
        "the handshake drained but never announced, so every producer would spin \
         forever in await_consumer"
    );
}
