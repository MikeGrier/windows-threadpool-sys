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

use crate::probe_args::{self, ArgError, Flag};

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
mod sampling_tests;
#[cfg(test)]
mod tests;

mod measure;
mod raw_row;
mod render;

pub use self::measure::*;
pub use self::raw_row::*;
pub use self::render::*;

/// How many pushes each producer thread performs in one timed run, unless
/// [`Sampling`] says otherwise.
pub const PUSHES_PER_PRODUCER: usize = 50_000;

/// How many times each configuration is repeated, unless [`Sampling`] says
/// otherwise; the median is reported.
///
/// Odd, so the median is an observed value rather than an average of two. Five
/// because these probes run on a virtual machine, where a single run can be
/// perturbed by something entirely outside the process.
pub const REPETITIONS: usize = 5;

/// The producer counts measured, in order, unless [`Sampling`] says otherwise.
///
/// Fixed rather than derived from the host's processor count, so two runs on
/// different machines produce comparable rows. The host's own count is reported
/// alongside, since the interesting region is around and beyond it.
pub const PRODUCER_COUNTS: &[usize] = &[1, 2, 4, 8, 16, 32];

/// How a run samples: how long each timed pass is, how many are taken, and at
/// which producer counts (`M4.2`).
///
/// Settable so that the first step the design notes prescribe on a wide control
/// -- lengthen the span and raise the repetitions on the unchanged configuration
/// -- is a command-line argument and not a source edit. The
/// [default](Sampling::default) is the three constants above, so a run given no
/// settings takes the published counts. What a run used travels with its
/// [`Observation`] and is printed in its report.
///
/// **The order passes are taken in is a setting too, since `M4.4`**, and its
/// default changed there: see [`Order`] for what the figures measured before it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sampling {
    /// Pushes each producer performs in one timed pass: the length of the span.
    pub pushes_per_producer: usize,
    /// Timed passes per configuration, of which the median is reported. With an
    /// even count the median taken is the upper of the two middle passes, so it
    /// is still one that was observed.
    pub repetitions: usize,
    /// The producer counts measured, in the order given. Scaling is reported
    /// against the one-producer row, so a list without 1 reports no scaling.
    pub producer_counts: Vec<usize>,
    /// The order the timed passes are taken in.
    pub order: Order,
}

impl Default for Sampling {
    fn default() -> Self {
        Self {
            pushes_per_producer: PUSHES_PER_PRODUCER,
            repetitions: REPETITIONS,
            producer_counts: PRODUCER_COUNTS.to_vec(),
            order: Order::Interleaved,
        }
    }
}

impl Sampling {
    /// The settings `probe-queue-contention` takes, which are this type's
    /// fields.
    pub const FLAGS: [Flag; 4] = [
        Flag {
            name: "pushes-per-producer",
            value: "N",
            help: "pushes each producer performs in one timed pass; 50000 when not given",
        },
        Flag {
            name: "repetitions",
            value: "N",
            help: "timed passes per configuration, the median reported; 5 when not given",
        },
        Flag {
            name: "producers",
            value: "N,N,...",
            help: "producer counts to measure, in order; 1,2,4,8,16,32 when not given",
        },
        Flag {
            name: "order",
            value: "interleaved|sequential",
            help: "how passes are ordered; interleaved when not given, sequential being the \
                   order of the figures published before M4.4",
        },
    ];

    /// The sampling a command line asks for: the defaults, with each setting it
    /// gives in place of its default.
    ///
    /// # Errors
    ///
    /// What [`probe_args::parse`] refuses, a value that is not a whole number of
    /// at least one or a producer list [`probe_args::positive_list`] refuses, and
    /// a combination whose largest run would not fit the queue's capacity
    /// arithmetic: the isolated regime sizes its queue to the next power of two
    /// at or above producers times pushes, and a total with none is refused
    /// rather than overflowing.
    pub fn from_args(args: impl IntoIterator<Item = String>) -> Result<Self, ArgError> {
        let given = probe_args::parse(args, &Self::FLAGS)?;
        let mut sampling = Self::default();
        if let Some(value) = given.get("pushes-per-producer") {
            sampling.pushes_per_producer = probe_args::positive("pushes-per-producer", value)?;
        }
        if let Some(value) = given.get("repetitions") {
            sampling.repetitions = probe_args::positive("repetitions", value)?;
        }
        if let Some(value) = given.get("producers") {
            sampling.producer_counts = probe_args::positive_list("producers", value)?;
        }
        if let Some(value) = given.get("order") {
            sampling.order = Order::from_name(value).ok_or_else(|| ArgError::Invalid {
                flag: "order",
                value: value.to_owned(),
                reason: "is neither `interleaved` nor `sequential`".to_owned(),
            })?;
        }
        let largest = sampling
            .producer_counts
            .iter()
            .max()
            .copied()
            .expect("a parsed producer list is never empty");
        if largest
            .checked_mul(sampling.pushes_per_producer)
            .and_then(usize::checked_next_power_of_two)
            .is_none()
        {
            return Err(ArgError::Invalid {
                flag: "pushes-per-producer",
                value: sampling.pushes_per_producer.to_string(),
                reason: format!(
                    "{largest} producers times this many pushes is too large to size a queue for"
                ),
            });
        }
        Ok(sampling)
    }

    /// Whether this is the sampling a run given no settings uses.
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// The order a run takes its timed passes in (`M4.4`).
///
/// **Interleaved by default, because the same-code control is two rows.** The
/// control is `reserving_mpsc` against `reserving(32/32)`: the same code measured
/// twice in one run. Taken back to back, each configuration's passes fill their
/// own stretch of the run, so the control's two halves were measured several
/// configurations apart, and drift across that interval -- frequency, thermal,
/// the scheduler -- landed inside the control and inside every comparison judged
/// against it. Interleaved, every configuration at a producer count takes one pass
/// per round, so each row's passes are spread across the same rounds.
///
/// **Sequential is kept, not replaced.** It is the order every figure published
/// before `M4.4` was measured in, so it is what retakes them, and running both on
/// one host is the measurement that says whether interleaving narrowed the
/// control there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    /// One pass of every configuration per round, the first position moving on
    /// by one each round so that no configuration is always measured first.
    Interleaved,
    /// Every pass of one configuration before the next configuration starts.
    Sequential,
}

impl Order {
    /// The name the command line and the test corpus give this order.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Interleaved => "interleaved",
            Self::Sequential => "sequential",
        }
    }

    /// The order a [`name`](Self::name) names, if it names one.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        [Self::Interleaved, Self::Sequential]
            .into_iter()
            .find(|order| order.name() == name)
    }

    /// How the report's sampling line describes this order.
    #[must_use]
    pub const fn described(self) -> &'static str {
        match self {
            Self::Interleaved => "passes interleaved across configurations",
            Self::Sequential => "each configuration's passes back to back",
        }
    }
}

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
    /// Fastest of the run's timed repetitions ([`Sampling::repetitions`]), in
    /// nanoseconds per operation.
    ///
    /// Carried because [`d-observations-not-verdicts`] obliges every published
    /// figure to arrive with its run count *and its dispersion*: a median alone
    /// is an anecdote a reader cannot compare against their own hardware. The
    /// repetitions the median discards are the only evidence this probe has
    /// about its own stability within a run, and discarding them silently was
    /// the crate publishing a figure its own contract forbids.
    ///
    /// [`d-observations-not-verdicts`]: ../DESIGN-NOTES.md#d-observations-not-verdicts
    pub fastest_nanos_per_op: f64,
    /// Slowest of the run's timed repetitions, in nanoseconds per operation. See
    /// [`Run::fastest_nanos_per_op`].
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
            // Positive too, not merely finite: a zero slowest endpoint divides
            // cleanly to a spread of zero -- the most reassuring value this
            // column can hold -- and a negative one yields a negative spread,
            // which is not a dispersion at all. `is_measured` keeps both out of
            // the report, but `spread` is public and answers callers directly.
            && self.slowest_nanos_per_op > 0.0
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
    /// How the run sampled, so its report states what produced its figures.
    pub sampling: Sampling,
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

/// Raw result of one timed repetition: elapsed nanoseconds and refusals.
type Repetition = (f64, u64);

/// Run one configuration `sampling.repetitions` times and keep the median.
///
/// The timer is handed the producer count and `sampling.pushes_per_producer`,
/// and the per-operation figures divide by that same value, so the pushes a
/// pass performed and the pushes its time is divided by cannot come from two
/// places.
///
/// The median rather than the mean, because on a virtual machine the failure
/// mode is one run being hugely slower rather than a spread around a centre,
/// and a mean would carry that outlier into the reported number.
fn median_run(
    shape: &'static str,
    producers: usize,
    sampling: &Sampling,
    mut timer: impl FnMut(usize, usize) -> Repetition,
) -> Run {
    let pushes_per_producer = sampling.pushes_per_producer;
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
    let _ = timer(producers, pushes_per_producer);

    let results: Vec<Repetition> = (0..sampling.repetitions)
        .map(|_| timer(producers, pushes_per_producer))
        .collect();
    summarise(shape, producers, pushes_per_producer, results)
}

/// A configuration's timer, boxed so a run's configurations can sit in one list.
type Timer<'a> = Box<dyn FnMut(usize, usize) -> Repetition + 'a>;

/// Time every configuration in `timers` at `producers`, in the order
/// `sampling.order` says, and return one [`Run`] per timer in the order given.
///
/// [`Order::Sequential`] is [`median_run`] once per timer. [`Order::Interleaved`]
/// gives every timer its untimed pass first -- the same pass, for the reason
/// `median_run` records -- and then takes `sampling.repetitions` rounds of one
/// pass each, starting each round one position later than the last. So all the
/// timed passes of every configuration fall within the same rounds, and the
/// median of each is taken over passes spread across the same interval.
fn runs(
    producers: usize,
    sampling: &Sampling,
    timers: &mut [(&'static str, Timer<'_>)],
) -> Vec<Run> {
    let pushes_per_producer = sampling.pushes_per_producer;
    if sampling.order == Order::Sequential {
        return timers
            .iter_mut()
            .map(|(shape, timer)| median_run(shape, producers, sampling, &mut **timer))
            .collect();
    }

    for (_, timer) in timers.iter_mut() {
        let _ = timer(producers, pushes_per_producer);
    }
    let count = timers.len();
    let mut results: Vec<Vec<Repetition>> = vec![Vec::with_capacity(sampling.repetitions); count];
    for round in 0..sampling.repetitions {
        for position in 0..count {
            let index = (round + position) % count;
            results[index].push((timers[index].1)(producers, pushes_per_producer));
        }
    }
    timers
        .iter()
        .zip(results)
        .map(|((shape, _), results)| summarise(shape, producers, pushes_per_producer, results))
        .collect()
}

/// The [`Run`] a configuration's timed passes make: the median, with the extremes
/// beside it, each divided by the pushes every pass performed.
fn summarise(
    shape: &'static str,
    producers: usize,
    pushes_per_producer: usize,
    mut results: Vec<Repetition>,
) -> Run {
    let repetitions = results.len();
    results.sort_by(|left, right| left.0.total_cmp(&right.0));
    let (elapsed_nanos, refusals) = results[repetitions / 2];
    // The sort is ascending by elapsed time, so the extremes are the ends. They
    // are carried rather than discarded because a median without its dispersion
    // is what `d-observations-not-verdicts` forbids publishing.
    let (fastest_nanos, _) = results[0];
    let (slowest_nanos, _) = results[repetitions - 1];

    let pushes = (producers * pushes_per_producer) as f64;
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

/// The smallest capacity every timed queue accepts, which `M4.2`'s settable push count can fall below: one
/// producer making one push needs room for one item, and the slotwise queue refuses a capacity under two.
/// Larger than the run needs is harmless, since nothing is refused either way, so the minimum raises the
/// capacity and leaves the workload as it was asked for.
const MINIMUM_QUEUE_CAPACITY: usize = 2;

/// Capacity big enough that a whole run fits, so nothing is ever refused, and no smaller than a timed queue
/// accepts.
fn capacity_for(producers: usize, pushes_per_producer: usize) -> usize {
    (producers * pushes_per_producer)
        .next_power_of_two()
        .max(MINIMUM_QUEUE_CAPACITY)
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
/// [D-38](../../windows-waitable-queues/DESIGN-NOTES.md#d-38) -- an acquire that
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
