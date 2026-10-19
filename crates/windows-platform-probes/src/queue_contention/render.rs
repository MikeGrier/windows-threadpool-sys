// Copyright (c) Mike Grier.
// Split from queue_contention.rs at bf9786ef.

//! How a run is turned into text: the ratio bounds and the table and cell formatters the report uses.

use super::*;
/// The interval the cost ratio `numerator / denominator` could occupy, given
/// each row's observed span.
///
/// Both arguments are rows whose costs are nanoseconds per operation, and the
/// result is in those same terms: `0.20x` means the numerator cost a fifth of
/// what the denominator cost. A caller wanting a *rate* ratio -- "how much
/// faster" -- passes the two rows the other way round, which is what
/// [`Observation::scaling_bounds`] does.
///
/// See [`Observation::scaling_bounds`] for why this is a bound rather than a
/// sample.
///
/// **An earlier version of this function documented a cost ratio and returned
/// the inverse**, leaving both internal callers to compensate by swapping their
/// arguments. That works until somebody calls it directly, which is the defect a
/// review caught: a public contract that is only correct if you read the
/// implementation is not a contract.
///
/// `None` when either span touches zero, which cannot happen for a real run and
/// is reported rather than divided by.
#[must_use]
pub fn ratio_bounds(numerator: Run, denominator: Run) -> Option<(f64, f64)> {
    // **A `<= 0.0` test does not reject `NaN`.** IEEE comparison against NaN is
    // false whichever way it is written, so every one of these four disjuncts is
    // false for a NaN endpoint and the guard falls through to the division,
    // which then renders `[NaN-NaN]`. Testing `is_finite()` first is what
    // actually excludes it, and it excludes infinities in the same move.
    let endpoints = [
        numerator.fastest_nanos_per_op,
        numerator.slowest_nanos_per_op,
        denominator.fastest_nanos_per_op,
        denominator.slowest_nanos_per_op,
    ];
    if endpoints
        .iter()
        .any(|endpoint| !endpoint.is_finite() || *endpoint <= 0.0)
    {
        return None;
    }
    // Widest is the numerator at its worst over the denominator at its best;
    // narrowest is the reverse.
    let low = numerator.fastest_nanos_per_op / denominator.slowest_nanos_per_op;
    let high = numerator.slowest_nanos_per_op / denominator.fastest_nanos_per_op;
    Some((low, high))
}

/// Renders a ratio together with the interval it could occupy, or `--`.
///
/// The bound is printed in square brackets to mark it as *not* a sampled range:
/// the row ranges above it are observed spans, this is arithmetic over two of
/// them.
///
/// **Both rows must have measured something.** A shape that failed to run
/// reports zero nanoseconds, and zero is the sentinel for "no measurement here"
/// on either side of the division -- a zero numerator would render `0.00x`,
/// which is a number a reader takes for a result rather than for the absence of
/// one. An earlier version guarded only the denominator, on the reasoning that
/// division is what breaks; publishing a plausible figure from a row that never
/// ran is the worse failure of the two.
#[must_use]
pub fn format_ratio_bounded(numerator: Option<Run>, denominator: Option<Run>) -> String {
    match (numerator, denominator) {
        (Some(numerator), Some(denominator))
            if numerator.is_measured() && denominator.is_measured() =>
        {
            let point = numerator.nanos_per_op / denominator.nanos_per_op;
            match ratio_bounds(numerator, denominator) {
                Some((low, high)) => format!("{point:.2}x [{low:.2}-{high:.2}]"),
                None => format!("{point:.2}x"),
            }
        }
        _ => "--".to_owned(),
    }
}

/// The **minimum** width the report gives a bounded ratio column.
///
/// Named here, beside the formatter, rather than written as a literal in the
/// report's format string. [`format_ratio_bounded`] emits a point estimate *and*
/// its interval -- `1.00x [1.00-1.00]` is 17 characters, not the 5 a bare
/// `1.00x` would take -- and a Rust width is a minimum rather than a maximum, so
/// a field narrower than the value does not truncate it, it pushes every later
/// column out of line with its header. The report had been allocating 10.
///
/// **This is a floor, not a bound, because the formatter has no bound.** The
/// interval's endpoints come from measured spans, so a slow outlier -- an
/// ordinary event on a loaded or virtualized host, and the reason `median_run`
/// takes a median at all -- widens the cell without limit: a 300 ms repetition
/// against a 4 ns one renders `10.00x [6.67-1500.00]`, which is 21. Use
/// [`ratio_column_width`] to size the column against the values it must actually
/// hold; this constant only stops a table of narrow values from looking cramped.
pub const RATIO_COLUMN_WIDTH: usize = 20;

/// The width a ratio column must take to keep its rows aligned with its header.
///
/// The widest cell the column has to hold, or [`RATIO_COLUMN_WIDTH`] when that
/// is wider. Derived from the rendered cells rather than assumed, because
/// [`format_ratio_bounded`]'s output length is a function of measured data and
/// therefore has no compile-time bound -- see [`RATIO_COLUMN_WIDTH`] for the
/// case that overruns it. A caller must render every cell of the column before
/// emitting the header, which is the only ordering that can get this right.
#[must_use]
pub fn ratio_column_width<'a>(cells: impl IntoIterator<Item = &'a str>) -> usize {
    column_width(cells, RATIO_COLUMN_WIDTH)
}

/// The width a column must take to keep its rows aligned with its header.
///
/// The widest cell, or `floor` when that is wider. Every column in this report
/// holds rendered measurements, whose length is a function of the data rather
/// than a constant, so a fixed field silently shifts everything to its right the
/// first time a value outgrows it. `floor` only stops a table of narrow values
/// from looking cramped; it is never an upper bound.
#[must_use]
pub fn column_width<'a>(cells: impl IntoIterator<Item = &'a str>, floor: usize) -> usize {
    cells
        .into_iter()
        .map(str::len)
        .max()
        .map_or(floor, |widest| widest.max(floor))
}

/// Renders a scaling factor together with the interval it could occupy.
///
/// See [`Observation::scaling_bounds`]: the bracketed interval is a bound over
/// two unpaired spans, not a distribution.
#[must_use]
pub fn format_scaling_bounded(point: Option<f64>, bounds: Option<(f64, f64)>) -> String {
    match (point, bounds) {
        // A scaling of zero is the sentinel, not a measurement: it means the
        // many-producer row reported no throughput at all. Guarded here as well
        // as against non-finite values, because zero is the half that renders
        // plausibly -- `0.00x` looks like a contended queue, `infx` does not.
        (Some(point), _) if !point.is_finite() || point <= 0.0 => "--".to_owned(),
        (Some(point), Some((low, high))) if low.is_finite() && high.is_finite() => {
            format!("{point:.2}x [{low:.2}-{high:.2}]")
        }
        (Some(point), _) => format!("{point:.2}x"),
        (None, _) => "--".to_owned(),
    }
}

/// Renders one regime's rows as the report's table body.
///
/// Here rather than in the binary so it can be tested without running the
/// measurement. It takes a sink rather than writing to stdout for the reason
/// [`crate::report`] records: a helper writing to stdout while its caller
/// composes a string emits its lines first, reordering the report without losing
/// any of it.
pub fn render_table(out: &mut dyn fmt::Write, runs: &[Run]) {
    const HEADERS: [&str; 7] = [
        "shape",
        "producers",
        "ns/op",
        "ops/sec",
        "refusals",
        "ns/op range",
        "spread",
    ];
    // The widths this table has always used, kept as floors so an ordinary
    // report is unchanged. Every column is a measurement rendered at its natural
    // width, and `spread` and `ns/op range` are quotients and pairs of measured
    // endpoints with no upper bound -- a long pause, the very outlier
    // `median_run` exists to tolerate, renders wider than any fixed field and
    // pushes every column after it out of line. See `ratio_column_width`, which
    // is the same argument for the ratio tables.
    const FLOOR: [usize; 7] = [18, 10, 14, 16, 14, 18, 9];

    let mut rows: Vec<[String; 7]> = Vec::with_capacity(runs.len());
    for run in runs {
        // `shape` and `producers` are configuration and always mean something.
        // Every other column is a measurement, so a row that did not run has
        // nothing to put in any of them -- including `refusals`, whose zero
        // would otherwise read as "nothing was refused" rather than "nothing
        // was attempted". See `Run::is_measured`.
        let (nanos, ops, refusals, range, spread) = if run.is_measured() {
            (
                format!("{:.1}", run.nanos_per_op),
                format!("{:.0}", run.ops_per_second),
                run.refusals.to_string(),
                format!(
                    "{:.1}-{:.1}",
                    run.fastest_nanos_per_op, run.slowest_nanos_per_op
                ),
                format_scaling(run.spread()),
            )
        } else {
            (
                "--".to_owned(),
                "--".to_owned(),
                "--".to_owned(),
                "--".to_owned(),
                "--".to_owned(),
            )
        };
        rows.push([
            run.shape.to_owned(),
            run.producers.to_string(),
            nanos,
            ops,
            refusals,
            range,
            spread,
        ]);
    }

    let mut width = FLOOR;
    for row in &rows {
        for (column, cell) in row.iter().enumerate() {
            width[column] = width[column].max(cell.len());
        }
    }

    let _ = writeln!(
        out,
        "{:<a$} {:>b$} {:>c$} {:>d$} {:>e$} {:>f$} {:>g$}",
        HEADERS[0],
        HEADERS[1],
        HEADERS[2],
        HEADERS[3],
        HEADERS[4],
        HEADERS[5],
        HEADERS[6],
        a = width[0],
        b = width[1],
        c = width[2],
        d = width[3],
        e = width[4],
        f = width[5],
        g = width[6],
    );
    for row in &rows {
        let _ = writeln!(
            out,
            "{:<a$} {:>b$} {:>c$} {:>d$} {:>e$} {:>f$} {:>g$}",
            row[0],
            row[1],
            row[2],
            row[3],
            row[4],
            row[5],
            row[6],
            a = width[0],
            b = width[1],
            c = width[2],
            d = width[3],
            e = width[4],
            f = width[5],
            g = width[6],
        );
    }
}

/// A scaling factor, or `--` when it is missing or not a number.
///
/// Guards non-finite values for the same reason [`format_ratio`] guards its
/// denominator, and the guard belongs here rather than in [`Observation::scaling`]:
/// a shape whose one-producer row reports zero makes the quotient infinite, and
/// `infx` in a column of measurements reads as a measurement. `scaling` is
/// deliberately allowed to return the non-finite value -- it is arithmetic, not
/// a renderer -- so the display layer is where it has to be caught.
#[must_use]
pub fn format_scaling(scaling: Option<f64>) -> String {
    match scaling {
        Some(value) if value.is_finite() && value > 0.0 => format!("{value:.2}x"),
        _ => "--".to_owned(),
    }
}

/// `numerator / denominator` as a cost ratio, or `--` when either is missing.
///
/// Guards both rows rather than trusting them: a shape that failed to run
/// reports zero, so a zero denominator would print `inf` or `NaN` and a zero
/// numerator would print `0.00x` -- and of those two the second is the more
/// dangerous, because it looks like a measurement rather than like a failure.
#[must_use]
pub fn format_ratio(numerator: Option<Run>, denominator: Option<Run>) -> String {
    match (numerator, denominator) {
        (Some(numerator), Some(denominator))
            if numerator.is_measured() && denominator.is_measured() =>
        {
            format!("{:.2}x", numerator.nanos_per_op / denominator.nanos_per_op)
        }
        _ => "--".to_owned(),
    }
}

/// One row's nanoseconds per operation, or `--` when the row is missing or did
/// not run.
///
/// Guards the sentinel for the reason [`Run::is_measured`] records: this column
/// is the report's most-read number, and `0.0` in it reads as a shape too fast
/// to time rather than as one that never ran.
#[must_use]
pub fn format_nanos(run: Option<Run>) -> String {
    match run {
        Some(run) if run.is_measured() => format!("{:.1}", run.nanos_per_op),
        _ => "--".to_owned(),
    }
}
