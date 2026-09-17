// Copyright (c) Mike Grier.

//! How does the array queue's push path scale with producer count?
//!
//! The question behind it is whether the **tail claim** contends badly enough to
//! justify other MPSC shapes -- but what is timed is each shape's whole push
//! path, so the curve is push-path scaling and the claim is one term in it. See
//! the regime notes below before attributing any difference to the claim.
//!
//! **An experiment, not a component.** These probes measure platform behaviour
//! and are not for production use. Do not call them from production code, and
//! do not lift a technique out of here. See this crate's
//! [DESIGN-NOTES.md](../DESIGN-NOTES.md).
//!
//! # The two decisions this exists to force
//!
//! **1. Are the linked and sharded MPSC shapes needed at all?** They are parked
//! in a plan that is not in this repository yet, gated on this measurement
//! rather than on taste. (The named checklist file arrives with the rest of the
//! queue work; naming a path that does not resolve is what this crate's own
//! link rule forbids, and an earlier draft did it here.)
//! If N threads compare-and-swapping one tail does not collapse
//! at the producer counts a real system reaches, the bounded array queue is the
//! only MPSC the queue crate ever needs, and two speculative shapes never get
//! written.
//!
//! **2. Should `slotwise_mpsc` and `reserving_mpsc` merge?** They ship as peers because
//! honouring a reservation costs the producer a read of the consumer's
//! position -- one line every thread touches -- and *how much* that costs was a
//! judgement rather than a measurement. This probe does not turn it into one:
//! the drained rows compare two complete push paths and cannot separate that
//! read from the other differences between the shapes. What they supply is an
//! end-to-end comparison in the regime where the read is most expensive, which
//! is an input to that decision rather than the decision.
//!
//! # Two regimes, because one of them cannot answer the second question
//!
//! Producers are timed twice, and the pair is the point.
//!
//! - **Isolated** -- capacity large enough that nothing is ever refused, and no
//!   consumer running. Nothing else touches the queue, so the curve against N
//!   is the producer side alone, with no consumer traffic in it.
//!
//!   **It is not the compare-and-swap alone, and an earlier draft said it
//!   was.** What is timed is each shape's whole push path: the tail claim, but
//!   also the slot-sequence load, the item write, the publication store, and
//!   the doorbell's fence. `permit_mpsc` takes two shared read-modify-writes
//!   where the others take one. So a difference between shapes here is a
//!   difference in PUSH COST, and attributing it to the claim alone would be
//!   reading more out of the number than is in it. Found by a review.
//!
//! - **Drained** -- a consumer looping on `pop` while the producers push.
//!   This is the regime in which `reserving_mpsc`'s read of `head` is at its
//!   most expensive, because `head` is only costly to read when a consumer is
//!   *writing* it. Measured in isolation that read hits a clean, shared line and
//!   looks free -- which would be a confident wrong answer.
//!
//!   **It does not isolate that read, and it does not bound it either** -- an
//!   earlier correction here claimed a bound, which is no better than the
//!   over-claim it replaced. The ratio is between two complete push paths, and
//!   `reserving_mpsc` and `slotwise_mpsc` differ in claim protocol, slot metadata
//!   and retry behaviour as well as in that one load. Writing `R` and `S` for the
//!   two totals, `R - S` is the read plus those other differences, and **those
//!   terms are not ordered** -- so the difference constrains the read in neither
//!   direction. Read these rows as an end-to-end comparison of two shapes in the
//!   regime where the read is most expensive, and nothing finer. Found by a
//!   review -- the second one to correct this sentence.
//!
//! # What is deliberately not claimed
//!
//! The drained regime has a **single** consumer, because that is what MPSC
//! means. At high producer counts it is therefore expected to become
//! consumer-bound, and a throughput plateau there says nothing about the tail
//! claim. The probe reports each run's refusal count -- from the queue's own
//! `Observable` counters -- so a backpressure-bound run is visible as a fact
//! rather than mistaken for contention. Read the isolated regime for push-path
//! scaling with producer count, and the drained one for the end-to-end shape
//! comparison taken while `head` is being written.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Instant;

use windows_waitable_queues::{permit_mpsc, reserving_mpsc, slotwise_mpsc};

use windows_waitable_queues::reserving_mpsc::{Balanced, ClaimLayout, Enduring, Perpetual};

/// The 128-bit layout exists only where a 128-bit exchange is native.
///
/// The condition is duplicated in this crate's `Cargo.toml`, which adds the
/// `dwcas` feature under the same `cfg`; see the comment there for why the
/// architectures are named rather than testing `target_has_atomic = "128"`, and
/// why enabling the feature unconditionally breaks the workspace's deliberately
/// supported `i686-pc-windows-msvc` build. Changing one without the other yields
/// either a missing type or an unused feature.
#[cfg(any(
    all(target_arch = "x86_64", target_feature = "cmpxchg16b"),
    target_arch = "aarch64"
))]
use windows_waitable_queues::reserving_mpsc::Wide;

#[cfg(test)]
mod tests;

/// How many pushes each producer thread performs in one timed run.
pub const PUSHES_PER_PRODUCER: usize = 50_000;

/// How many times each configuration is repeated; the median is reported.
///
/// Odd, so the median is an observed value rather than an average of two. Five
/// because these probes run on a virtual machine, where a single run can be
/// perturbed by something entirely outside the process.
pub const REPETITIONS: usize = 5;

/// The producer counts measured, in order.
///
/// Fixed rather than derived from the host's processor count, so two runs on
/// different machines produce comparable rows. The host's own count is reported
/// alongside, since the interesting region is around and beyond it.
pub const PRODUCER_COUNTS: &[usize] = &[1, 2, 4, 8, 16, 32];

/// The names a run is filed under.
///
/// **Named once because a lookup by string literal is a rename waiting to
/// fail, and this one already did.** The `mpsc` -> `slotwise_mpsc` rename
/// updated the recording side and not the reporting binary, which went on
/// asking for `"mpsc"`; every lookup returned `None` and two entire columns of
/// the report rendered as `--` without anything erroring. A wrong shape name is
/// not a compile error, so the only defence is that both sides read the same
/// definition.
pub mod shapes {
    /// The bounded-array MPSC.
    pub const SLOTWISE_MPSC: &str = "slotwise_mpsc";
    /// The reservation-based MPSC.
    pub const RESERVING_MPSC: &str = "reserving_mpsc";
    /// The experimental permit-claiming MPSC, measured against
    /// [`RESERVING_MPSC`] because it is a candidate replacement for it.
    pub const PERMIT_MPSC: &str = "permit_mpsc";
    /// The contended-atomic floor the queues are measured against.
    ///
    /// Contended, not uncontended: every producer thread increments the **same**
    /// `AtomicU64`, which is the point -- it is the cheapest possible thing N
    /// threads can do to one cache line, so it separates what the queue costs
    /// from what this processor does to a fought-over line.
    pub const BASELINE_FETCH_ADD: &str = "baseline_fetch_add";
    /// `reserving_mpsc` on its default layout: a `u64` split 32 / 32.
    ///
    /// The same configuration as [`RESERVING_MPSC`], run again under its own
    /// name so the layout comparison reads without a reader having to know
    /// which layout the default is.
    pub const CLAIM_NARROW: &str = "reserving(32/32)";
    /// `reserving_mpsc` on `Enduring`: a `u64` split 16 / 48.
    pub const CLAIM_DEEP: &str = "reserving(16/48)";
    /// `reserving_mpsc` on `Perpetual`: a `u64` split 8 / 56.
    pub const CLAIM_PERPETUAL: &str = "reserving(8/56)";
    /// `reserving_mpsc` on `Wide`: a `u128` split 64 / 64.
    pub const CLAIM_WIDE: &str = "reserving(64/64)";
}
/// One configuration's result.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Run {
    /// Which queue shape, or the baseline.
    pub shape: &'static str,
    /// How many producer threads pushed concurrently.
    pub producers: usize,
    /// Median nanoseconds per successful operation, across all producers.
    ///
    /// An *operation* is one successful push for every queue shape. For
    /// [`shapes::BASELINE_FETCH_ADD`] it is one `fetch_add` on a shared
    /// `AtomicU64` -- that row is a floor rather than a queue, so it has no
    /// pushes to report, and labelling this field per-push would publish it with
    /// units it does not have.
    pub nanos_per_op: f64,
    /// Successful operations per second, summed across producers. See
    /// [`Run::nanos_per_op`] for what counts as an operation in each row.
    pub ops_per_second: f64,
    /// Pushes refused for want of room during the median run.
    ///
    /// Non-zero means the run was at least partly bounded by the consumer
    /// rather than by the claim, which is a fact about the measurement and not
    /// about the queue.
    pub refusals: u64,
    /// Fastest of the [`REPETITIONS`] timed repetitions, in nanoseconds per
    /// operation.
    ///
    /// Carried because [`d-observations-not-verdicts`] obliges every published
    /// figure to arrive with its run count *and its dispersion*: a median alone
    /// is an anecdote a reader cannot compare against their own hardware. The
    /// four repetitions the median discards are the only evidence this probe has
    /// about its own stability within a run, and discarding them silently was
    /// the crate publishing a figure its own contract forbids.
    ///
    /// [`d-observations-not-verdicts`]: ../DESIGN-NOTES.md#d-observations-not-verdicts
    pub fastest_nanos_per_op: f64,
    /// Slowest of the [`REPETITIONS`] timed repetitions, in nanoseconds per
    /// operation. See [`Run::fastest_nanos_per_op`].
    pub slowest_nanos_per_op: f64,
}

impl Run {
    /// Whether this row carries a measurement at all.
    ///
    /// **This is the single definition of the "did not run" sentinel, and every
    /// renderer asks it rather than restating the test.** A shape that did not
    /// run reports zero, and zero is not an obviously broken value in any of
    /// this report's columns: `0.0` ns/op reads as immeasurably fast, `0.00x`
    /// as a ratio of one, `0.0-0.0` as perfect stability. Each is the most
    /// flattering cell its column can hold, produced by a row that measured
    /// nothing.
    ///
    /// It lives here because the test was previously written inline in the
    /// renderers that remembered it and simply absent from those that did not
    /// -- which is how [`Run::spread`] came to render `0.00x` for a shape that
    /// never ran. Fixing that one accessor left the same hole in four other
    /// paths, because the sentinel was a convention rather than a definition.
    /// A renderer can now only get this wrong by not asking.
    ///
    /// **Every field a renderer prints, not just the median.** An earlier
    /// version tested `nanos_per_op` alone, so a row with a plausible median and
    /// a poisoned `ops_per_second` or range endpoint answered `true` and
    /// [`render_table`] then formatted those fields directly -- publishing `NaN`
    /// or `inf` in a column of measurements, which is the failure the sentinel
    /// exists to prevent. `refusals` is an integer and carries no such value.
    #[must_use]
    pub fn is_measured(&self) -> bool {
        [
            self.nanos_per_op,
            self.ops_per_second,
            self.fastest_nanos_per_op,
            self.slowest_nanos_per_op,
        ]
        .iter()
        .all(|value| value.is_finite() && *value > 0.0)
    }

    /// The spread across this configuration's repetitions, as a multiple.
    ///
    /// `1.00` would mean every repetition took the same time. A wide spread
    /// says the figure beside it is one draw from a distribution this host does
    /// not hold still, which is the reading the median alone hides.
    ///
    /// `None` when the fastest repetition took no measurable time, which cannot
    /// happen for a real run and marks a shape that did not run at all.
    ///
    /// **This returns an `Option` rather than a sentinel, and that is the whole
    /// point.** An earlier version returned `0.0` for the unmeasurable case,
    /// reasoning that zero is not a plausible spread. It renders as `0.00x`,
    /// which reads as *perfect stability* -- the most reassuring cell the column
    /// can contain, produced by a row that measured nothing. Every neighbouring
    /// accessor already returns `Option` for the same situation; this one was
    /// the exception, and the exception is what a renderer got wrong.
    #[must_use]
    pub fn spread(&self) -> Option<f64> {
        // `> 0.0` alone is not the test: `f64::INFINITY > 0.0` is true, and a
        // finite slowest over an infinite fastest is `0.0` -- a spread of zero,
        // which is the reassuring end of this column. Both endpoints must be
        // real numbers before dividing them.
        if self.fastest_nanos_per_op.is_finite()
            && self.fastest_nanos_per_op > 0.0
            && self.slowest_nanos_per_op.is_finite()
        {
            Some(self.slowest_nanos_per_op / self.fastest_nanos_per_op)
        } else {
            None
        }
    }
}

/// Everything one invocation measured.
#[derive(Debug, Clone)]
pub struct Observation {
    /// Producers timed with no consumer and no possibility of refusal.
    pub isolated: Vec<Run>,
    /// Producers timed against a consumer looping on `pop`.
    ///
    /// The handshake in `await_consumer` guarantees that loop has executed at
    /// least once before any producer starts timing. It does not guarantee the
    /// consumer is never descheduled afterwards, so "looping" describes what the
    /// consumer thread runs, not how continuously it is scheduled to run it.
    pub drained: Vec<Run>,
    /// Processors available to **this process**, when it could be determined.
    ///
    /// This is `available_parallelism`, which is the process-available estimate
    /// and not the host's logical-processor count: an affinity mask or a job
    /// object narrows it, so under either it is legitimately smaller than the
    /// banner's `16p`. It is reported because it is what decides whether a
    /// producer count oversubscribes *this run*, which is the question a reader
    /// of these rows actually has; the host's own shape is already on the banner.
    ///
    /// `None` when the query failed. An earlier version mapped failure to `0`,
    /// which the report then printed as a zero-processor host -- a value no host
    /// has, presented with the same confidence as a measured one.
    pub available_parallelism: Option<usize>,
}

impl Observation {
    /// Look one run up.
    #[must_use]
    pub fn find(&self, regime: &[Run], shape: &str, producers: usize) -> Option<Run> {
        regime
            .iter()
            .find(|run| run.shape == shape && run.producers == producers)
            .copied()
    }

    /// How many of `shapes` actually produced a measurement at `producers`.
    ///
    /// Counted rather than written as a literal because the 64/64 rows are
    /// `cfg`-elided on a target with no native 128-bit exchange, so a hardcoded
    /// count would be false there.
    ///
    /// **Presence is not measurement.** A row can be present and still carry the
    /// did-not-run sentinel, and the report's prose is where that distinction
    /// escapes: `render_table` marks such a row `--` in every measured cell while
    /// a sentence above it counts the shape as measured. Asking
    /// [`Run::is_measured`] here is what keeps the two halves of the report
    /// telling the same story.
    #[must_use]
    pub fn count_measured(&self, regime: &[Run], shapes: &[&str], producers: usize) -> usize {
        shapes
            .iter()
            .filter(|shape| {
                self.find(regime, shape, producers)
                    .is_some_and(|run| run.is_measured())
            })
            .count()
    }

    /// How far throughput scaled from one producer to `producers`.
    ///
    /// 1.0 means N producers together push no faster than one did, which is
    /// what a badly contended claim looks like. Perfect scaling would be N,
    /// which no shared-tail queue can reach.
    #[must_use]
    pub fn scaling(&self, regime: &[Run], shape: &str, producers: usize) -> Option<f64> {
        let one = self.find(regime, shape, 1)?;
        let many = self.find(regime, shape, producers)?;
        Some(many.ops_per_second / one.ops_per_second)
    }

    /// The interval [`Observation::scaling`] could occupy, given the two rows'
    /// observed spans.
    ///
    /// **This is a bound, not a sampled distribution, and the difference
    /// matters.** The probe measures each configuration in its own pass, so the
    /// repetitions behind the numerator and the denominator are not paired:
    /// there is no set of per-repetition ratios to take a median or a range
    /// over. What can be said is that if one producer's cost lies in `[a, b]`
    /// and N producers' in `[c, d]`, their ratio cannot fall outside
    /// `[a / d, b / c]` -- so this is the widest the scaling could be, which is
    /// conservative in the direction that matters.
    ///
    /// Carried because [`d-observations-not-verdicts`] obliges every published
    /// figure to arrive with its dispersion, and a *derived* figure is exactly
    /// where a bare point estimate is most likely to be over-read. Reporting
    /// the ratio alone, while the rows beneath it carry ranges, would put the
    /// least certain number on the page in the most confident dress.
    ///
    /// Pairing the repetitions would give a real distribution rather than a
    /// bound, and that is a change to how the probe measures rather than to how
    /// it reports -- see `M4.4`, which asks for candidates to be interleaved
    /// with their controls.
    ///
    /// [`d-observations-not-verdicts`]: ../DESIGN-NOTES.md#d-observations-not-verdicts
    #[must_use]
    pub fn scaling_bounds(
        &self,
        regime: &[Run],
        shape: &str,
        producers: usize,
    ) -> Option<(f64, f64)> {
        let one = self.find(regime, shape, 1)?;
        let many = self.find(regime, shape, producers)?;
        // Asked before the identity case below, not after: a row that did not
        // run is still a row, so `find` returns it and `producers == 1` would
        // otherwise hand back an exact `(1.0, 1.0)` for a shape that measured
        // nothing. The point estimate goes `NaN` in that case and the current
        // renderer suppresses it, but this is a public method and a caller
        // reading the bound on its own would see a certainty that is not there.
        if !one.is_measured() || !many.is_measured() {
            return None;
        }
        if producers == 1 {
            // **The one-producer row is a row divided by itself.** Its scaling is
            // exactly 1.0 by construction, not approximately 1.0 by measurement,
            // so there is no interval to report. Handing both spans to
            // `ratio_bounds` would treat one measurement as two independent ones
            // and manufacture a bound like `[0.84-1.20]` around a quantity that
            // cannot be anything but 1 -- uncertainty invented by the arithmetic
            // rather than observed, printed in the report's first row.
            //
            // The bound stays sound either way, since it contains 1.0. It is
            // tightness that is at stake, and at the identity it is exact.
            return Some((1.0, 1.0));
        }
        // Scaling is a RATE ratio -- many over one -- which is the COST ratio
        // one over many, so the rows go in that order.
        ratio_bounds(one, many)
    }
}

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

/// Time every configuration.
#[must_use]
pub fn measure() -> Observation {
    let mut isolated = Vec::new();
    let mut drained = Vec::new();

    for &producers in PRODUCER_COUNTS {
        isolated.push(median_run(shapes::BASELINE_FETCH_ADD, producers, |count| {
            time_contended_atomic(count)
        }));
        isolated.push(median_run(shapes::SLOTWISE_MPSC, producers, |count| {
            time_isolated_mpsc(count)
        }));
        isolated.push(median_run(shapes::RESERVING_MPSC, producers, |count| {
            time_isolated_reserving(count)
        }));
        isolated.push(median_run(shapes::PERMIT_MPSC, producers, |count| {
            time_isolated_permit(count)
        }));

        drained.push(median_run(shapes::SLOTWISE_MPSC, producers, |count| {
            time_drained_mpsc(count)
        }));
        drained.push(median_run(shapes::RESERVING_MPSC, producers, |count| {
            time_drained_reserving(count)
        }));
        drained.push(median_run(shapes::PERMIT_MPSC, producers, |count| {
            time_drained_permit(count)
        }));

        isolated.push(median_run(shapes::CLAIM_NARROW, producers, |count| {
            time_isolated_layout::<Balanced>(count)
        }));
        isolated.push(median_run(shapes::CLAIM_DEEP, producers, |count| {
            time_isolated_layout::<Enduring>(count)
        }));
        isolated.push(median_run(shapes::CLAIM_PERPETUAL, producers, |count| {
            time_isolated_layout::<Perpetual>(count)
        }));
        // Gated on the architectures where a 128-bit exchange is native; see the
        // `Wide` import above. `#[cfg]` governs only the statement that follows
        // it, so each of the two pushes carries its own.
        #[cfg(any(
            all(target_arch = "x86_64", target_feature = "cmpxchg16b"),
            target_arch = "aarch64"
        ))]
        isolated.push(median_run(shapes::CLAIM_WIDE, producers, |count| {
            time_isolated_layout::<Wide>(count)
        }));

        drained.push(median_run(shapes::CLAIM_NARROW, producers, |count| {
            time_drained_layout::<Balanced>(count)
        }));
        drained.push(median_run(shapes::CLAIM_DEEP, producers, |count| {
            time_drained_layout::<Enduring>(count)
        }));
        drained.push(median_run(shapes::CLAIM_PERPETUAL, producers, |count| {
            time_drained_layout::<Perpetual>(count)
        }));
        #[cfg(any(
            all(target_arch = "x86_64", target_feature = "cmpxchg16b"),
            target_arch = "aarch64"
        ))]
        drained.push(median_run(shapes::CLAIM_WIDE, producers, |count| {
            time_drained_layout::<Wide>(count)
        }));
    }

    Observation {
        isolated,
        drained,
        available_parallelism: thread::available_parallelism()
            .ok()
            .map(std::num::NonZeroUsize::get),
    }
}

/// Raw result of one timed repetition: elapsed nanoseconds and refusals.
type Repetition = (f64, u64);

/// Run one configuration [`REPETITIONS`] times and keep the median.
///
/// The median rather than the mean, because on a virtual machine the failure
/// mode is one run being hugely slower rather than a spread around a centre,
/// and a mean would carry that outlier into the reported number.
fn median_run(
    shape: &'static str,
    producers: usize,
    mut timer: impl FnMut(usize) -> Repetition,
) -> Run {
    // One untimed pass first. Be exact about what this does and does not warm:
    // for the queue timers, every call to `timer` builds and drops its OWN
    // queue, so this does not pre-touch the allocation any timed repetition will
    // use. The baseline timer allocates no queue at all -- one `AtomicU64` and a
    // barrier -- so for that row there is no allocation to pre-touch either way.
    // What the pass warms in both cases is the process: the allocator's size
    // class, the OS page cache, the instruction cache, and the branch
    // predictors, which is why the first timed repetition is no longer an
    // outlier. An earlier comment here claimed it faulted in "the" allocation,
    // which is not true of an allocation made fresh each pass, and a later one
    // said every timer builds a queue, which is not true of the baseline.
    // Both found by review.
    let _ = timer(producers);

    let mut results: Vec<Repetition> = (0..REPETITIONS).map(|_| timer(producers)).collect();
    results.sort_by(|left, right| left.0.total_cmp(&right.0));
    let (elapsed_nanos, refusals) = results[REPETITIONS / 2];
    // The sort is ascending by elapsed time, so the extremes are the ends. They
    // are carried rather than discarded because a median without its dispersion
    // is what `d-observations-not-verdicts` forbids publishing.
    let (fastest_nanos, _) = results[0];
    let (slowest_nanos, _) = results[REPETITIONS - 1];

    let pushes = (producers * PUSHES_PER_PRODUCER) as f64;
    Run {
        shape,
        producers,
        nanos_per_op: elapsed_nanos / pushes,
        ops_per_second: pushes / (elapsed_nanos / 1e9),
        refusals,
        fastest_nanos_per_op: fastest_nanos / pushes,
        slowest_nanos_per_op: slowest_nanos / pushes,
    }
}

/// The floor: N threads incrementing one shared counter.
///
/// Not a queue, and not trying to be. It is the cheapest possible operation on
/// a contended line, so it says how much of a queue's scaling curve is the
/// queue and how much is simply what this processor does when N cores fight
/// over one cache line.
fn time_contended_atomic(producers: usize) -> Repetition {
    let counter = Arc::new(AtomicU64::new(0));
    // One party per worker plus this thread. Every worker is created, then waits
    // here; the clock starts as the barrier releases, so neither thread creation
    // nor a solo head start by an early worker is inside the measurement. See
    // `start_barrier`'s note for why that matters at these producer counts.
    let gate = start_gate(producers);
    let spans = thread::scope(|scope| {
        let _release = ReleaseOnDrop(Arc::clone(&gate));
        let mut workers = Vec::with_capacity(producers);
        for _ in 0..producers {
            let counter = Arc::clone(&counter);
            let gate = Arc::clone(&gate);
            workers.push(scope.spawn(move || {
                if !gate.arrive_and_wait() {
                    // Abandoned before the party completed; see `StartGate`.
                    return (Instant::now(), Instant::now());
                }
                let began = Instant::now();
                for _ in 0..PUSHES_PER_PRODUCER {
                    counter.fetch_add(1, Ordering::Relaxed);
                }
                (began, Instant::now())
            }));
        }
        let _ = gate.arrive_and_wait();
        workers
            .into_iter()
            .map(|worker| worker.join().expect("a producer must not panic"))
            .collect::<Vec<_>>()
    });
    (measured_span(&spans), 0)
}

/// Capacity big enough that a whole run fits, so nothing is ever refused.
fn capacity_for(producers: usize) -> usize {
    (producers * PUSHES_PER_PRODUCER).next_power_of_two()
}

/// A gate holding every participant until all of them exist, or until the
/// coordinator gives up on the ones that do not.
///
/// **Without this the row labelled N producers need not have measured N of
/// them.** Spawning is not instant, and each worker used to start pushing the
/// moment it was created, so at 50,000 pushes an early producer could complete
/// a long uncontended prefix -- or finish entirely -- before the last thread was
/// spawned. The reported interval also began before any worker existed, folding
/// thread-creation cost into a per-push number. The curve against N is the whole
/// output of this probe, and both effects bend it downward exactly where it is
/// steepest.
///
/// The count includes the coordinating thread, so no worker can start before the
/// last one exists. It does NOT start the clock -- see [`measured_span`] for why
/// that is a separate job.
///
/// **Why this is not `std::sync::Barrier`.** A `Barrier`'s party count, once
/// set, must be met: there is no way to say "nobody else is coming". The timers
/// size the gate for every planned worker and then spawn them with
/// `Scope::spawn`, which *panics* if the OS refuses a thread. If that happened
/// after an earlier worker had already parked, the coordinator never reached its
/// own arrival, the count was never met, and `thread::scope` joined a
/// permanently parked worker while unwinding -- so the probe **hung rather than
/// failed**, which is the worse of the two. [`StartGate::release`] is the
/// missing operation, and [`ReleaseOnDrop`] performs it on the unwind path.
///
/// **The property the measurement depends on is preserved.** Every parked party
/// is woken by one `notify_all` and returns as a group, exactly as
/// `Barrier::wait` does -- which is what [`measured_span`] relies on when it
/// argues that this thread cannot time the workers and each must time itself.
struct StartGate {
    state: Mutex<GateState>,
    opened: Condvar,
}

struct GateState {
    /// Participants still to arrive. Reaching zero opens the gate.
    remaining: usize,
    /// Whether waiters may proceed, for either reason.
    open: bool,
    /// Whether the gate opened because everyone arrived, rather than because
    /// the coordinator released it. A participant that reads `false` learns its
    /// run was abandoned and should not do the work.
    complete: bool,
}

impl StartGate {
    /// `participants` workers plus the coordinating thread.
    fn new(participants: usize) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(GateState {
                remaining: participants + 1,
                open: false,
                complete: false,
            }),
            opened: Condvar::new(),
        })
    }

    /// Poisoning is stepped over rather than propagated.
    ///
    /// Nothing but the gate's own bookkeeping runs under this lock, so a
    /// poisoned mutex means some *other* thread panicked while parked here. The
    /// whole point of this type is to unblock that situation; panicking on the
    /// way -- from inside a `Drop` that is already unwinding -- would abort the
    /// process instead.
    fn locked(&self) -> MutexGuard<'_, GateState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Arrive, then block until every participant has, or until the gate is
    /// released.
    ///
    /// `true` when the gate opened because the party was complete, which is the
    /// only case in which a run's timings mean anything. `false` says the
    /// coordinator gave up; the caller should return without doing the work.
    #[must_use]
    fn arrive_and_wait(&self) -> bool {
        let mut state = self.locked();
        state.remaining = state.remaining.saturating_sub(1);
        if state.remaining == 0 && !state.open {
            state.open = true;
            state.complete = true;
        }
        if state.open {
            let complete = state.complete;
            drop(state);
            self.opened.notify_all();
            return complete;
        }
        while !state.open {
            state = self
                .opened
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
        state.complete
    }

    /// Open the gate now, however many participants are missing.
    ///
    /// A no-op once the gate is open, so the guard that calls this on the
    /// ordinary path costs nothing.
    fn release(&self) {
        let mut state = self.locked();
        state.open = true;
        drop(state);
        self.opened.notify_all();
    }
}

/// Releases the start gate however the spawning phase ends.
///
/// The failure this exists for is a `Scope::spawn` panic partway through
/// creating the workers: without it, the parties already parked wait for a count
/// that will never be met, and the join that `thread::scope` performs while
/// unwinding never returns. Held inside the scope's closure, so it drops while
/// that closure unwinds -- before the join loop it needs to unblock.
struct ReleaseOnDrop(Arc<StartGate>);

impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        self.0.release();
    }
}

fn start_gate(participants: usize) -> Arc<StartGate> {
    StartGate::new(participants)
}

/// The wall-clock window the producers were actually inside: from the first to
/// begin to the last to finish.
///
/// **Each worker times itself, because this thread cannot time them.** The
/// obvious arrangement -- release the barrier, call `Instant::now()` here, and
/// read `elapsed()` after the scope ends -- is wrong at both ends, and a review
/// caught it:
///
/// - `Barrier::wait` releases every party together, and this thread is just
///   another party. A worker can return from `wait` and run an arbitrary prefix
///   of its pushes before this thread is scheduled again to read the clock, so
///   the start could land after work had already happened. That understates the
///   interval, which OVERSTATES throughput.
/// - `thread::scope` joins every worker before it returns, so an `elapsed()`
///   read after it includes thread exit and join. That overstates the interval,
///   which understates throughput.
///
/// Neither error is bounded by anything this probe controls, and both bite
/// hardest on the fast low-producer rows where a run is only hundreds of
/// microseconds. Taking the earliest start and the latest finish measures the
/// span the producers were contending over and nothing else.
fn measured_span(spans: &[(Instant, Instant)]) -> f64 {
    let began = spans
        .iter()
        .map(|(began, _)| *began)
        .min()
        .expect("a run has at least one producer");
    let ended = spans
        .iter()
        .map(|(_, ended)| *ended)
        .max()
        .expect("a run has at least one producer");

    ended.duration_since(began).as_nanos() as f64
}

fn time_isolated_mpsc(producers: usize) -> Repetition {
    let (tx, rx) =
        slotwise_mpsc::bounded::<u64>(capacity_for(producers)).expect("a valid capacity");
    let gate = start_gate(producers);
    let spans = thread::scope(|scope| {
        let _release = ReleaseOnDrop(Arc::clone(&gate));
        let mut workers = Vec::with_capacity(producers);
        for producer in 0..producers {
            let tx = tx.clone();
            let gate = Arc::clone(&gate);
            workers.push(scope.spawn(move || {
                if !gate.arrive_and_wait() {
                    // Abandoned before the party completed; see `StartGate`.
                    return (Instant::now(), Instant::now());
                }
                let began = Instant::now();
                for index in 0..PUSHES_PER_PRODUCER {
                    tx.push((producer * PUSHES_PER_PRODUCER + index) as u64)
                        .expect("the run fits in the capacity");
                }
                (began, Instant::now())
            }));
        }
        let _ = gate.arrive_and_wait();
        workers
            .into_iter()
            .map(|worker| worker.join().expect("a producer must not panic"))
            .collect::<Vec<_>>()
    });
    let elapsed = measured_span(&spans);
    let refusals = tx.refused();
    // Drain before dropping: teardown would otherwise walk every slot, and that
    // is not part of what is being timed.
    while rx.pop().is_ok() {}
    (elapsed, refusals)
}

fn time_isolated_reserving(producers: usize) -> Repetition {
    let (tx, rx) =
        reserving_mpsc::bounded::<u64>(capacity_for(producers)).expect("a valid capacity");
    let gate = start_gate(producers);
    let spans = thread::scope(|scope| {
        let _release = ReleaseOnDrop(Arc::clone(&gate));
        let mut workers = Vec::with_capacity(producers);
        for producer in 0..producers {
            let tx = tx.clone();
            let gate = Arc::clone(&gate);
            workers.push(scope.spawn(move || {
                if !gate.arrive_and_wait() {
                    // Abandoned before the party completed; see `StartGate`.
                    return (Instant::now(), Instant::now());
                }
                let began = Instant::now();
                for index in 0..PUSHES_PER_PRODUCER {
                    tx.push((producer * PUSHES_PER_PRODUCER + index) as u64)
                        .expect("the run fits in the capacity");
                }
                (began, Instant::now())
            }));
        }
        let _ = gate.arrive_and_wait();
        workers
            .into_iter()
            .map(|worker| worker.join().expect("a producer must not panic"))
            .collect::<Vec<_>>()
    });
    let elapsed = measured_span(&spans);
    let refusals = tx.refused();
    while rx.pop().is_ok() {}
    (elapsed, refusals)
}

/// The experimental permit claim, with no consumer and no possibility of refusal.
///
/// Not "the regime that isolates the claim", which an earlier wording said: this
/// times the whole push path, including slot metadata, the item write,
/// publication and the doorbell's fence. See the module header.
///
/// A line-for-line twin of [`time_isolated_reserving`] with one shape
/// substituted. Deliberately not factored into a generic over the two, which
/// would need a trait both implement and would put a dynamic or monomorphised
/// indirection inside the timed region -- in a measurement whose whole output is
/// a difference of a few nanoseconds per push.
fn time_isolated_permit(producers: usize) -> Repetition {
    let (tx, rx) = permit_mpsc::bounded::<u64>(capacity_for(producers)).expect("a valid capacity");
    let gate = start_gate(producers);
    let spans = thread::scope(|scope| {
        let _release = ReleaseOnDrop(Arc::clone(&gate));
        let mut workers = Vec::with_capacity(producers);
        for producer in 0..producers {
            let tx = tx.clone();
            let gate = Arc::clone(&gate);
            workers.push(scope.spawn(move || {
                if !gate.arrive_and_wait() {
                    // Abandoned before the party completed; see `StartGate`.
                    return (Instant::now(), Instant::now());
                }
                let began = Instant::now();
                for index in 0..PUSHES_PER_PRODUCER {
                    tx.push((producer * PUSHES_PER_PRODUCER + index) as u64)
                        .expect("the run fits in the capacity");
                }
                (began, Instant::now())
            }));
        }
        let _ = gate.arrive_and_wait();
        workers
            .into_iter()
            .map(|worker| worker.join().expect("a producer must not panic"))
            .collect::<Vec<_>>()
    });
    let elapsed = measured_span(&spans);
    let refusals = tx.refused();
    while rx.pop().is_ok() {}
    (elapsed, refusals)
}

/// A capacity a real system would choose, so the drained regime exercises
/// backpressure the way a real one would.
pub const DRAINED_CAPACITY: usize = 1024;

/// Stops a drained timer's consumer however the producer phase ends.
///
/// Each drained timer parks a consumer in `while !done { ... }` and clears the
/// flag once the producer scope returns. On the **failure** path that line is
/// never reached: a producer's assertion unwinds straight past it, so the
/// consumer keeps spinning on a flag nobody will ever set, and its `JoinHandle`
/// is dropped without a join. The probe then leaves a thread burning a core for
/// the life of the process -- in exactly the run someone is trying to read a
/// failure out of.
///
/// Clearing the flag from `Drop` runs on both paths, so the consumer observes
/// the stop and finishes whether the producers succeeded or panicked. The join
/// is still done explicitly on the success path, where its return value is the
/// refusal count; on the unwind path the thread is detached, but it terminates
/// promptly rather than spinning, which is the part that mattered.
struct StopOnDrop(Arc<AtomicBool>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// Holds a producer until the consumer has drained at least once.
///
/// **The gate is not enough, and the difference is the regime being measured.**
/// Arriving at the gate proves the consumer exists, is scheduled and is past
/// thread start-up; it does not prove the consumer has reached its first `pop`.
/// The gate releases every party together, so a producer could push into a queue
/// nobody was taking from yet -- an undrained opening to a run whose whole point
/// is that it is drained. The window was bounded by a scheduling quantum rather
/// than by thread creation, which is why the gate was still worth having, but it
/// was not zero.
///
/// **What this guarantees, stated exactly.** No producer begins timing until the
/// consumer has executed its pop path at least once. It does *not* guarantee the
/// consumer is draining continuously from then on -- nothing a flag can express
/// would, since the consumer can be descheduled at any point afterwards, as it
/// can at any point during the run. What it removes is the case where producers
/// push into a queue whose consumer has not yet run at all.
///
/// `Acquire`/`Release` rather than `Relaxed`, though the flag carries no data:
/// this is the standing "promote the load" answer recorded in the queue crate's
/// [D-40](../../windows-waitable-queues/DESIGN-NOTES.md#d-40) -- an acquire that
/// proves unnecessary costs little, while a relaxed load that turns out to have
/// been load-bearing fails only on hardware nobody here owns.
///
/// **This changes what the drained rows measure**, which is why it is `M4.3` and
/// why the figures taken before it are kept beside the ones taken after rather
/// than replaced: they are measurements of two different pieces of code, and
/// both are real.
fn await_consumer(ready: &AtomicBool) {
    while !ready.load(Ordering::Acquire) {
        std::hint::spin_loop();
    }
}

/// Drains once, then announces -- the producing half of the `M4.3` handshake.
///
/// **The one statement of the ordering.** Four drained timers need it, and a
/// hand-written `pop` followed by a `store` in each of them is four chances for
/// the two lines to end up the other way round, which no test could see: the
/// timers cannot run without running the whole probe. Defining it here gives the
/// ordering a single home that the `the_handshake_drains_before_it_announces`
/// test can drive with a recording fake.
///
/// Swapping these two statements reintroduces the undrained opening in its
/// narrower form -- the announcement would mean "this consumer is about to
/// drain", which a descheduling can falsify, rather than "this consumer has
/// executed the pop path", which nothing can.
///
/// `Release` pairs with the `Acquire` in [`await_consumer`], so a producer that
/// observes the flag has the pop ordered before it.
fn drain_then_announce(pop_once: impl FnOnce(), ready: &AtomicBool) {
    pop_once();
    ready.store(true, Ordering::Release);
}

fn time_drained_mpsc(producers: usize) -> Repetition {
    let (tx, rx) = slotwise_mpsc::bounded::<u64>(DRAINED_CAPACITY).expect("a valid capacity");
    let done = Arc::new(AtomicBool::new(false));
    let consumer_done = Arc::clone(&done);
    // Set on every exit path, not just the one that returns. See StopOnDrop.
    let stop = StopOnDrop(done);
    // Closes the undrained opening the gate alone leaves. See await_consumer.
    let ready = Arc::new(AtomicBool::new(false));
    let consumer_ready = Arc::clone(&ready);
    // The consumer is a gate participant, not merely spawned: spawning is not
    // readiness, and a consumer still in thread start-up while producers push
    // turns the opening of the run into an undrained regime.
    //
    // The gate alone does not finish the job, which is why `await_consumer`
    // exists. Arriving proves the consumer exists, is scheduled and is past
    // start-up; it does not prove the consumer has reached its first `pop`, and
    // the gate releases every party together. The handshake narrows that
    // remainder to a stated guarantee: no producer begins timing until the
    // consumer has executed its pop path at least once. Continuous draining is
    // not guaranteed and cannot be by a flag. This is the M4.3 change, and it
    // MOVED the drained numbers -- the figures taken before it are kept beside
    // the ones taken after rather than replaced, because they measure two
    // different pieces of code.
    let gate = start_gate(producers + 1);
    let consumer_gate = Arc::clone(&gate);

    let consumer = thread::spawn(move || {
        if !consumer_gate.arrive_and_wait() {
            return rx.refused();
        }
        // **One drain attempt BEFORE announcing, not merely reaching the loop.**
        // Publishing first proves only that the consumer is about to drain: it
        // can be descheduled between the store and its first `pop`, which is the
        // same undrained opening in a narrower form. Popping first makes the
        // announcement mean `this consumer has executed the pop path`, which is a
        // fact rather than an intention. The queue is empty here, so it costs one
        // failed pop, and it happens before any producer has started timing.
        drain_then_announce(
            || {
                let _ = rx.pop();
            },
            &consumer_ready,
        );
        // Spin rather than park: the doorbell's cost is `doorbell_cost`'s
        // question, and parking here would measure that instead of the claim.
        while !consumer_done.load(Ordering::Relaxed) {
            while rx.pop().is_ok() {}
            std::hint::spin_loop();
        }
        while rx.pop().is_ok() {}
        rx.refused()
    });

    let spans = thread::scope(|scope| {
        let _release = ReleaseOnDrop(Arc::clone(&gate));
        let mut workers = Vec::with_capacity(producers);
        for producer in 0..producers {
            let tx = tx.clone();
            let gate = Arc::clone(&gate);
            let ready = Arc::clone(&ready);
            workers.push(scope.spawn(move || {
                if !gate.arrive_and_wait() {
                    // Abandoned before the party completed; see `StartGate`.
                    return (Instant::now(), Instant::now());
                }
                await_consumer(&ready);
                let began = Instant::now();
                for index in 0..PUSHES_PER_PRODUCER {
                    let mut item = (producer * PUSHES_PER_PRODUCER + index) as u64;
                    // Retry a FULL queue, which is what a real producer does;
                    // the refusal count is what makes that visible. Anything
                    // else is not retryable -- a disconnected queue never
                    // drains -- and retrying it is an infinite spin that
                    // presents as a hung probe rather than as the consumer
                    // failure it actually is. The queue crate says so itself:
                    // "retrying the first is sensible and retrying the second
                    // is a spin".
                    while let Err(error) = tx.push(item) {
                        assert!(
                            error.is_retryable(),
                            "the consumer is gone, so this push can never \
                             succeed: {error}"
                        );
                        item = error.into_inner();
                        std::hint::spin_loop();
                    }
                }
                (began, Instant::now())
            }));
        }
        let _ = gate.arrive_and_wait();
        workers
            .into_iter()
            .map(|worker| worker.join().expect("a producer must not panic"))
            .collect::<Vec<_>>()
    });
    let elapsed = measured_span(&spans);

    drop(stop);
    drop(tx);
    let refusals = consumer.join().expect("the consumer must not panic");
    (elapsed, refusals)
}

fn time_drained_reserving(producers: usize) -> Repetition {
    // **Defaults on both sides, and that is a correction.** This row previously
    // enabled high-water tracking here and nowhere else, to "also price the
    // switch M31.4 made opt-in". But the number it feeds is presented as the
    // cost of *reservation*, and tracking adds work to this shape's push path
    // alone, so the ratio measured reservation plus a handicap with no way for a
    // reader to separate them.
    //
    // **What the handicap actually is, corrected:** an earlier version of this
    // comment said tracking adds a load of the consumer's position. It does not.
    // `reserving_mpsc::publish` loads `head` **unconditionally** -- the slot
    // write needs that acquire edge whether or not anything is measured, as the
    // comment at that load says in as many words. What the switch adds is the
    // depth arithmetic and the metric update on the far side of a branch that is
    // taken either way. Smaller than claimed, and still not part of what this row
    // is presented as measuring.
    //
    // Nothing consumes the high-water figure here either, so the tracking was
    // paying a cost to produce a number nobody read. Pricing that switch is a
    // worthwhile measurement and needs its own row, with both shapes tracking,
    // rather than being folded into this comparison.
    let (tx, rx) = reserving_mpsc::bounded::<u64>(DRAINED_CAPACITY).expect("a valid capacity");
    let done = Arc::new(AtomicBool::new(false));
    let consumer_done = Arc::clone(&done);
    // Set on every exit path, not just the one that returns. See StopOnDrop.
    let stop = StopOnDrop(done);
    // Closes the undrained opening the gate alone leaves. See await_consumer.
    let ready = Arc::new(AtomicBool::new(false));
    let consumer_ready = Arc::clone(&ready);
    // The consumer joins the gate here for the reason it does in the slotwise
    // twin: a run whose opening is undrained is not the regime being measured.
    let gate = start_gate(producers + 1);
    let consumer_gate = Arc::clone(&gate);

    let consumer = thread::spawn(move || {
        if !consumer_gate.arrive_and_wait() {
            return rx.refused();
        }
        // **One drain attempt BEFORE announcing, not merely reaching the loop.**
        // Publishing first proves only that the consumer is about to drain: it
        // can be descheduled between the store and its first `pop`, which is the
        // same undrained opening in a narrower form. Popping first makes the
        // announcement mean `this consumer has executed the pop path`, which is a
        // fact rather than an intention. The queue is empty here, so it costs one
        // failed pop, and it happens before any producer has started timing.
        drain_then_announce(
            || {
                let _ = rx.pop();
            },
            &consumer_ready,
        );
        while !consumer_done.load(Ordering::Relaxed) {
            while rx.pop().is_ok() {}
            std::hint::spin_loop();
        }
        while rx.pop().is_ok() {}
        rx.refused()
    });

    let spans = thread::scope(|scope| {
        let _release = ReleaseOnDrop(Arc::clone(&gate));
        let mut workers = Vec::with_capacity(producers);
        for producer in 0..producers {
            let tx = tx.clone();
            let gate = Arc::clone(&gate);
            let ready = Arc::clone(&ready);
            workers.push(scope.spawn(move || {
                if !gate.arrive_and_wait() {
                    // Abandoned before the party completed; see `StartGate`.
                    return (Instant::now(), Instant::now());
                }
                await_consumer(&ready);
                let began = Instant::now();
                for index in 0..PUSHES_PER_PRODUCER {
                    let mut item = (producer * PUSHES_PER_PRODUCER + index) as u64;
                    while let Err(error) = tx.push(item) {
                        // Only a FULL queue is retryable; see the note on the
                        // first of these loops.
                        assert!(
                            error.is_retryable(),
                            "the consumer is gone, so this push can never \
                             succeed: {error}"
                        );
                        item = error.into_inner();
                        std::hint::spin_loop();
                    }
                }
                (began, Instant::now())
            }));
        }
        let _ = gate.arrive_and_wait();
        workers
            .into_iter()
            .map(|worker| worker.join().expect("a producer must not panic"))
            .collect::<Vec<_>>()
    });
    let elapsed = measured_span(&spans);

    drop(stop);
    drop(tx);
    let refusals = consumer.join().expect("the consumer must not panic");
    (elapsed, refusals)
}

/// The experimental permit claim, against a consumer looping on `pop`.
///
/// The regime that can price the claim honestly, for the same reason the
/// reserving twin needs it: the shared line a producer touches is only
/// expensive when a consumer is writing it. Measured in isolation, an
/// uncontended line looks free -- which would be a confident wrong answer, and
/// this shape has more riding on that answer than the others, because it trades
/// `reserving_mpsc`'s *load* of the consumer's position for a read-modify-write
/// on a count the consumer also writes.
fn time_drained_permit(producers: usize) -> Repetition {
    let (tx, rx) = permit_mpsc::bounded::<u64>(DRAINED_CAPACITY).expect("a valid capacity");
    let done = Arc::new(AtomicBool::new(false));
    let consumer_done = Arc::clone(&done);
    // Set on every exit path, not just the one that returns. See StopOnDrop.
    let stop = StopOnDrop(done);
    // Closes the undrained opening the gate alone leaves. See await_consumer.
    let ready = Arc::new(AtomicBool::new(false));
    let consumer_ready = Arc::clone(&ready);
    let gate = start_gate(producers + 1);
    let consumer_gate = Arc::clone(&gate);

    let consumer = thread::spawn(move || {
        if !consumer_gate.arrive_and_wait() {
            return rx.refused();
        }
        // **One drain attempt BEFORE announcing, not merely reaching the loop.**
        // Publishing first proves only that the consumer is about to drain: it
        // can be descheduled between the store and its first `pop`, which is the
        // same undrained opening in a narrower form. Popping first makes the
        // announcement mean `this consumer has executed the pop path`, which is a
        // fact rather than an intention. The queue is empty here, so it costs one
        // failed pop, and it happens before any producer has started timing.
        drain_then_announce(
            || {
                let _ = rx.pop();
            },
            &consumer_ready,
        );
        while !consumer_done.load(Ordering::Relaxed) {
            while rx.pop().is_ok() {}
            std::hint::spin_loop();
        }
        while rx.pop().is_ok() {}
        rx.refused()
    });

    let spans = thread::scope(|scope| {
        let _release = ReleaseOnDrop(Arc::clone(&gate));
        let mut workers = Vec::with_capacity(producers);
        for producer in 0..producers {
            let tx = tx.clone();
            let gate = Arc::clone(&gate);
            let ready = Arc::clone(&ready);
            workers.push(scope.spawn(move || {
                if !gate.arrive_and_wait() {
                    // Abandoned before the party completed; see `StartGate`.
                    return (Instant::now(), Instant::now());
                }
                await_consumer(&ready);
                let began = Instant::now();
                for index in 0..PUSHES_PER_PRODUCER {
                    let mut item = (producer * PUSHES_PER_PRODUCER + index) as u64;
                    while let Err(error) = tx.push(item) {
                        // Only a FULL queue is retryable; see the note on the
                        // first of these loops.
                        assert!(
                            error.is_retryable(),
                            "the consumer is gone, so this push can never \
                             succeed: {error}"
                        );
                        item = error.into_inner();
                        std::hint::spin_loop();
                    }
                }
                (began, Instant::now())
            }));
        }
        let _ = gate.arrive_and_wait();
        workers
            .into_iter()
            .map(|worker| worker.join().expect("a producer must not panic"))
            .collect::<Vec<_>>()
    });
    let elapsed = measured_span(&spans);

    drop(stop);
    drop(tx);
    let refusals = consumer.join().expect("the consumer must not panic");
    (elapsed, refusals)
}

/// One claim-word layout, with no consumer and no possibility of refusal.
///
/// As with the other isolated timers, this is the whole push path and not the
/// claim word alone; only the layout differs between these rows, so a difference
/// is still attributable to the layout, but its magnitude is a share of total
/// push cost rather than of the exchange.
///
/// **Generic over the layout, where [`time_isolated_permit`] is deliberately
/// duplicated, and the difference is the point.** That twin compares two
/// *different types*, which a generic could only unify behind a trait, putting
/// an indirection that might not inline identically inside the timed region.
/// These are the *same type* at different layout parameters, so this
/// monomorphises to exactly the code a hand-written copy would produce -- there
/// is nothing left to dispatch.
///
/// Measures `reserving_mpsc` itself rather than a stand-in. An earlier form of
/// this probe carried its own duplicated implementation of the claim protocol,
/// built so the layouts could be compared before the shipping crate had them;
/// it drifted from the original twice while doing so. The shipping type takes
/// the layout as a parameter now, so the duplicate is gone.
fn time_isolated_layout<L: ClaimLayout>(producers: usize) -> Repetition {
    let (tx, rx) =
        reserving_mpsc::bounded_as::<u64, L>(capacity_for(producers)).expect("a valid capacity");
    let gate = start_gate(producers);
    let spans = thread::scope(|scope| {
        let _release = ReleaseOnDrop(Arc::clone(&gate));
        let mut workers = Vec::with_capacity(producers);
        for producer in 0..producers {
            let tx = tx.clone();
            let gate = Arc::clone(&gate);
            workers.push(scope.spawn(move || {
                if !gate.arrive_and_wait() {
                    // Abandoned before the party completed; see `StartGate`.
                    return (Instant::now(), Instant::now());
                }
                let began = Instant::now();
                for index in 0..PUSHES_PER_PRODUCER {
                    tx.push((producer * PUSHES_PER_PRODUCER + index) as u64)
                        .expect("the run fits in the capacity");
                }
                (began, Instant::now())
            }));
        }
        let _ = gate.arrive_and_wait();
        workers
            .into_iter()
            .map(|worker| worker.join().expect("a producer must not panic"))
            .collect::<Vec<_>>()
    });
    let elapsed = measured_span(&spans);
    let refusals = tx.refused();
    while rx.pop().is_ok() {}
    (elapsed, refusals)
}

/// One claim-word layout, against a consumer looping on `pop`.
///
/// Generic for [`time_isolated_layout`]'s reason.
fn time_drained_layout<L: ClaimLayout + 'static>(producers: usize) -> Repetition {
    let (tx, rx) =
        reserving_mpsc::bounded_as::<u64, L>(DRAINED_CAPACITY).expect("a valid capacity");
    let done = Arc::new(AtomicBool::new(false));
    let consumer_done = Arc::clone(&done);
    // Set on every exit path, not just the one that returns. See StopOnDrop.
    let stop = StopOnDrop(done);
    // Closes the undrained opening the gate alone leaves. See await_consumer.
    let ready = Arc::new(AtomicBool::new(false));
    let consumer_ready = Arc::clone(&ready);
    // The consumer joins the gate for the reason its twins do: a run whose
    // opening is undrained is not the regime being measured.
    let gate = start_gate(producers + 1);
    let consumer_gate = Arc::clone(&gate);

    let consumer = thread::spawn(move || {
        if !consumer_gate.arrive_and_wait() {
            return rx.refused();
        }
        // **One drain attempt BEFORE announcing, not merely reaching the loop.**
        // Publishing first proves only that the consumer is about to drain: it
        // can be descheduled between the store and its first `pop`, which is the
        // same undrained opening in a narrower form. Popping first makes the
        // announcement mean `this consumer has executed the pop path`, which is a
        // fact rather than an intention. The queue is empty here, so it costs one
        // failed pop, and it happens before any producer has started timing.
        drain_then_announce(
            || {
                let _ = rx.pop();
            },
            &consumer_ready,
        );
        while !consumer_done.load(Ordering::Relaxed) {
            while rx.pop().is_ok() {}
            std::hint::spin_loop();
        }
        while rx.pop().is_ok() {}
        rx.refused()
    });

    let spans = thread::scope(|scope| {
        let _release = ReleaseOnDrop(Arc::clone(&gate));
        let mut workers = Vec::with_capacity(producers);
        for producer in 0..producers {
            let tx = tx.clone();
            let gate = Arc::clone(&gate);
            let ready = Arc::clone(&ready);
            workers.push(scope.spawn(move || {
                if !gate.arrive_and_wait() {
                    // Abandoned before the party completed; see `StartGate`.
                    return (Instant::now(), Instant::now());
                }
                await_consumer(&ready);
                let began = Instant::now();
                for index in 0..PUSHES_PER_PRODUCER {
                    let mut item = (producer * PUSHES_PER_PRODUCER + index) as u64;
                    while let Err(error) = tx.push(item) {
                        // Only a FULL queue is retryable; see the note on the
                        // first of these loops.
                        assert!(
                            error.is_retryable(),
                            "the consumer is gone, so this push can never \
                             succeed: {error}"
                        );
                        item = error.into_inner();
                        std::hint::spin_loop();
                    }
                }
                (began, Instant::now())
            }));
        }
        let _ = gate.arrive_and_wait();
        workers
            .into_iter()
            .map(|worker| worker.join().expect("a producer must not panic"))
            .collect::<Vec<_>>()
    });
    let elapsed = measured_span(&spans);
    drop(stop);
    drop(tx);
    let refusals = consumer.join().expect("the consumer must not panic");
    (elapsed, refusals)
}
