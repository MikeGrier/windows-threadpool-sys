// Copyright (c) Mike Grier.

//! Prints how the array queue's tail claim behaves as producers are added.
//!
//! **An experiment, not a component.** These probes measure platform behaviour
//! and are not for production use. Do not call them from production code, and
//! do not lift a technique out of here. See this crate's DESIGN-NOTES.md.
//!
//! This reports observations that bear on two questions otherwise settled by
//! taste: whether the linked and sharded MPSC shapes are ever needed, and
//! whether `slotwise_mpsc` and `reserving_mpsc` should merge. It does not settle
//! either -- see `queue_contention`'s module docs for what the regimes can and
//! cannot separate.

use windows_platform_probes::queue_contention::{
    DRAINED_CAPACITY, PRODUCER_COUNTS, PUSHES_PER_PRODUCER, REPETITIONS, Run, measure, shapes,
};
use windows_platform_probes::report::emit_report;

fn main() {
    // The probe's whole output policy, and it is one line: hand the renderer to
    // the sink. Nothing here or below names a stream -- that is chosen once, in
    // `report`, so retargeting a probe is not a rewrite.
    emit_report(render);
}

/// The probe's whole report, as text.
fn render(out: &mut dyn std::fmt::Write) {
    // First line of the report, and part of the returned text rather than
    // written out here: a captured report must carry the line naming the
    // machine that produced it, and the taint marker with it.
    let _ = writeln!(
        out,
        "{}",
        windows_placement_probe::fingerprint::banner_line()
    );
    let _ = writeln!(
        out,
        "== how does the array queue's push path scale with producer count? ==\n"
    );

    let observation = measure();
    // `available_parallelism`, not the host count -- an affinity mask or job
    // object narrows it, and saying "host reports" under either would contradict
    // the banner three lines up. The host's shape is already there; this is what
    // decides whether a producer count oversubscribes THIS run.
    let _ = match observation.available_parallelism {
        Some(count) => writeln!(out, "processors available to this process: {count}\n"),
        None => writeln!(
            out,
            "processors available to this process: unknown (the query failed)\n"
        ),
    };
    // The sampling parameters are capture parameters, and a figure is only
    // interpretable with them -- see D-observations-not-verdicts. The build
    // profile is one of them too: a captured report has to be able to show it
    // was produced by a build that can measure, not merely stay silent when it
    // was. The dispersion belongs here as well and is not yet carried; M4.2
    // covers it.
    let _ = writeln!(
        out,
        "profile: {}",
        if cfg!(debug_assertions) {
            "debug -- NOT A MEASUREMENT, see below"
        } else {
            "release"
        }
    );
    let _ = writeln!(
        out,
        "sampling: {} pushes per producer, median of {} repetitions, one untimed \
         warmup pass\n",
        PUSHES_PER_PRODUCER, REPETITIONS
    );
    // A debug build does not merely lose precision here: the un-inlined overhead
    // swamps the cache-coherence effects that ARE the finding, and the two MPSC
    // shapes come out indistinguishable -- a confident wrong answer. The banner
    // goes in the report rather than in a refusal to run, because it has to
    // travel with a captured report: whoever pastes these numbers somewhere is
    // the person who needs to see it, and a binary that refused would tell only
    // the person who already had the terminal open.
    if cfg!(debug_assertions) {
        let _ = writeln!(
            out,
            "!! DEBUG BUILD -- THESE NUMBERS ARE NOT A MEASUREMENT !!"
        );
        let _ = writeln!(
            out,
            "!! Un-inlined overhead swamps the effect being measured, and the"
        );
        let _ = writeln!(
            out,
            "!! MPSC shapes report as equivalent when they are not. Rebuild"
        );
        let _ = writeln!(out, "!! with --release before reading anything below.\n");
    }

    let _ = writeln!(
        out,
        "-- isolated: producers only, capacity large enough that nothing is refused --"
    );
    render_table(out, &observation.isolated);

    let _ = writeln!(
        out,
        "\n-- drained: a consumer popping continuously, capacity {DRAINED_CAPACITY} --"
    );
    render_table(out, &observation.drained);

    let _ = writeln!(out, "\ninterpretation:\n");

    // Question 1: does the claim collapse as producers are added?
    let _ = writeln!(
        out,
        "  1. push-path scaling with producer count (isolated regime)\n"
    );
    let _ = writeln!(
        out,
        "     {:<18} {:>12} {:>12} {:>12} {:>14}",
        "producers", "slotwise", "reserving", "permit", "atomic floor"
    );
    for &producers in PRODUCER_COUNTS {
        let mpsc = observation.scaling(&observation.isolated, shapes::SLOTWISE_MPSC, producers);
        let reserving =
            observation.scaling(&observation.isolated, shapes::RESERVING_MPSC, producers);
        let permit = observation.scaling(&observation.isolated, shapes::PERMIT_MPSC, producers);
        let floor =
            observation.scaling(&observation.isolated, shapes::BASELINE_FETCH_ADD, producers);
        let _ = writeln!(
            out,
            "     {producers:<18} {:>12} {:>12} {:>12} {:>14}",
            format_scaling(mpsc),
            format_scaling(reserving),
            format_scaling(permit),
            format_scaling(floor)
        );
    }
    let _ = writeln!(
        out,
        "\n     Read as: throughput at N producers divided by throughput at one."
    );
    let _ = writeln!(
        out,
        "     1.00 means N threads together push no faster than one did."
    );
    let _ = writeln!(
        out,
        "     The atomic floor is the cheapest possible contended operation,"
    );
    let _ = writeln!(
        out,
        "     so it says how much of any curve is the queue and how much is"
    );
    let _ = writeln!(
        out,
        "     simply what this processor does to a fought-over cache line."
    );

    // Question 2: what does reserving_mpsc's read of `head` actually cost?
    let _ = writeln!(
        out,
        "\n  2. reserving vs slotwise, drained (where `head` is written)\n"
    );
    let _ = writeln!(
        out,
        "     The ratio is the WHOLE push path of two different shapes, not the"
    );
    let _ = writeln!(
        out,
        "     price of reserving's extra `head` load on its own: they use"
    );
    let _ = writeln!(
        out,
        "     different claim protocols, slot metadata and retry behaviour. This"
    );
    let _ = writeln!(
        out,
        "     regime is where that load is at its most expensive -- but the"
    );
    let _ = writeln!(
        out,
        "     ratio still does not isolate it, or bound it either way.\n"
    );
    let _ = writeln!(
        out,
        "     {:<18} {:>14} {:>14} {:>10} {:>14} {:>16}",
        "producers", "slotwise ns/pu", "reserving", "ratio", "permit", "permit/reserving"
    );
    for &producers in PRODUCER_COUNTS {
        let plain = observation.find(&observation.drained, shapes::SLOTWISE_MPSC, producers);
        let reserving = observation.find(&observation.drained, shapes::RESERVING_MPSC, producers);
        let permit = observation.find(&observation.drained, shapes::PERMIT_MPSC, producers);
        let ratio = format_ratio(reserving, plain);
        // The column SH-15.5 exists to fill: the experimental claim against the
        // shipping shape it would replace. Below 1.00 means the permit claim is
        // cheaper; above means removing the room-decision race costs throughput.
        let permit_ratio = format_ratio(permit, reserving);
        let _ = writeln!(
            out,
            "     {producers:<18} {:>14} {:>14} {:>10} {:>14} {:>16}",
            format_nanos(plain),
            format_nanos(reserving),
            ratio,
            format_nanos(permit),
            permit_ratio
        );
    }
    let _ = writeln!(
        out,
        "\n     `reserving_mpsc` reads the consumer's position on every push and"
    );
    let _ = writeln!(
        out,
        "     `slotwise_mpsc` does not. This regime is where that read is at its"
    );
    let _ = writeln!(
        out,
        "     most expensive, because a consumer is writing the line being read"
    );
    let _ = writeln!(
        out,
        "     -- but the ratio does not decompose. It is an END-TO-END"
    );
    let _ = writeln!(
        out,
        "     comparison of two shapes: they also differ in claim protocol,"
    );
    let _ = writeln!(
        out,
        "     slot metadata and retry behaviour, and those differences are not"
    );
    let _ = writeln!(
        out,
        "     ordered. So this ratio neither isolates the read nor bounds it."
    );
    let _ = writeln!(
        out,
        "\n     `permit_mpsc` is experimental and is the candidate replacement"
    );
    let _ = writeln!(
        out,
        "     for `reserving_mpsc`: it removes that read entirely, and with it"
    );
    let _ = writeln!(
        out,
        "     the stale room decision behind SH-14.1, by making admission a"
    );
    let _ = writeln!(
        out,
        "     read-modify-write on a permit count instead. The last column is"
    );
    let _ = writeln!(
        out,
        "     the trade -- below 1.00 and the safer claim is also the cheaper"
    );
    let _ = writeln!(
        out,
        "     one; above 1.00 and closing the hole costs throughput."
    );

    // Question 3: what does the claim word's apportionment and width cost?
    let _ = writeln!(out, "\n  3. claim-word layout\n");
    // Counted from what was actually measured rather than written as a literal:
    // the 64/64 rows are cfg-elided on a target with no native 128-bit exchange,
    // and a hardcoded "four" would be false there.
    let layouts_measured = [
        shapes::CLAIM_NARROW,
        shapes::CLAIM_DEEP,
        shapes::CLAIM_PERPETUAL,
        shapes::CLAIM_WIDE,
    ]
    .iter()
    .filter(|shape| {
        observation
            .find(&observation.isolated, shape, PRODUCER_COUNTS[0])
            .is_some()
    })
    .count();
    let _ = writeln!(
        out,
        "     {layouts_measured} apportionments of reserving_mpsc's claim word, measured on"
    );
    let _ = writeln!(
        out,
        "     the shipping type itself rather than on a stand-in. 32/32 is the"
    );
    let _ = writeln!(
        out,
        "     default; 16/48 and 8/56 are the same u64 exchange with the bits"
    );
    let _ = writeln!(
        out,
        "     apportioned differently; 64/64 is a u128 exchange (cmpxchg16b on"
    );
    let _ = writeln!(
        out,
        "     x86-64, ldxp/stxp on aarch64), measured only where that is native."
    );
    let _ = writeln!(
        out,
        "     The three u64 rows issue the same instruction and differ only in"
    );
    let _ = writeln!(
        out,
        "     shift and mask constants, so there is no structural reason for one"
    );
    let _ = writeln!(
        out,
        "     to be slower -- but these rows time the WHOLE push path, so a"
    );
    let _ = writeln!(
        out,
        "     difference between them is not thereby noise. Read it against a"
    );
    let _ = writeln!(
        out,
        "     control before calling it either way: the reserving_mpsc row and"
    );
    let _ = writeln!(
        out,
        "     the 32/32 row above are the same code, so the gap between them is"
    );
    let _ = writeln!(
        out,
        "     what 'no difference' looks like on this host -- which across seven"
    );
    let _ = writeln!(
        out,
        "     runs was not zero, and was wide enough to swallow the layout rows."
    );
    let _ = writeln!(
        out,
        "     64/64 vs 32/32 is the double-width layout's effect on the whole"
    );
    let _ = writeln!(
        out,
        "     push path -- what moving the recurrence to 2^64 costs, against"
    );
    let _ = writeln!(
        out,
        "     8/56 merely deferring it. Not the exchange in isolation.\n"
    );
    for (label, regime) in [
        ("isolated", &observation.isolated),
        ("drained", &observation.drained),
    ] {
        let _ = writeln!(out, "     -- {label} --");
        let _ = writeln!(
            out,
            "     {:<10} {:>11} {:>11} {:>11} {:>11} {:>10} {:>10} {:>10}",
            "producers",
            "32/32 ns",
            "16/48 ns",
            "8/56 ns",
            "64/64 ns",
            "16/48 vs",
            "8/56 vs",
            "64/64 vs"
        );
        for &producers in PRODUCER_COUNTS {
            let narrow = observation.find(regime, shapes::CLAIM_NARROW, producers);
            let deep = observation.find(regime, shapes::CLAIM_DEEP, producers);
            let perpetual = observation.find(regime, shapes::CLAIM_PERPETUAL, producers);
            let wide = observation.find(regime, shapes::CLAIM_WIDE, producers);
            let _ = writeln!(
                out,
                "     {:<10} {:>11} {:>11} {:>11} {:>11} {:>10} {:>10} {:>10}",
                producers,
                format_nanos(narrow),
                format_nanos(deep),
                format_nanos(perpetual),
                format_nanos(wide),
                format_ratio(deep, narrow),
                format_ratio(perpetual, narrow),
                format_ratio(wide, narrow)
            );
        }
        let _ = writeln!(out);
    }
    let _ = writeln!(
        out,
        "     the 32/32 row and the reserving_mpsc row above are the same"
    );
    let _ = writeln!(
        out,
        "     configuration run twice, so the gap between them is this host's"
    );
    let _ = writeln!(
        out,
        "     same-code control: whatever it shows is dispersion, not a"
    );
    let _ = writeln!(
        out,
        "     difference between shapes. Do not read it as noise that can be"
    );
    let _ = writeln!(
        out,
        "     discounted -- its width is an open question about this"
    );
    let _ = writeln!(out, "     instrument. They");
    let _ = writeln!(
        out,
        "     are no longer a control against a duplicated implementation: the"
    );
    let _ = writeln!(
        out,
        "     shipping type takes the layout as a parameter, so there is nothing"
    );
    let _ = writeln!(
        out,
        "     left that could drift away from what callers actually run."
    );
    let _ = writeln!(
        out,
        "\n  CAUTION: the drained regime has ONE consumer, because that is what"
    );
    let _ = writeln!(
        out,
        "  MPSC means. At high producer counts it is expected to become"
    );
    let _ = writeln!(
        out,
        "  consumer-bound, and a plateau there says nothing about the claim."
    );
    let _ = writeln!(
        out,
        "  The refusal counts above are what make that visible: a run with"
    );
    let _ = writeln!(
        out,
        "  many refusals was waiting for the consumer, not for the tail."
    );
}

/// Append one regime's table to `out`.
///
/// Takes the buffer rather than printing, for the reason `pool_growth`'s twin
/// records: a helper writing to stdout while its caller composes a string emits
/// its lines first, reordering the report without losing any of it.
fn render_table(out: &mut dyn std::fmt::Write, runs: &[Run]) {
    let _ = writeln!(
        out,
        "{:<18} {:>10} {:>14} {:>16} {:>14}",
        "shape", "producers", "ns/push", "pushes/sec", "refusals"
    );
    for run in runs {
        let _ = writeln!(
            out,
            "{:<18} {:>10} {:>14.1} {:>16.0} {:>14}",
            run.shape, run.producers, run.nanos_per_push, run.pushes_per_second, run.refusals
        );
    }
}

/// A scaling factor, or `--` when it is missing or not a number.
///
/// Guards non-finite values for the same reason [`format_ratio`] guards its
/// denominator, and the guard belongs here rather than in `scaling`: a shape
/// whose one-producer row reports zero makes the quotient infinite, and
/// `infx` in a column of measurements reads as a measurement. `scaling` is
/// deliberately allowed to return the non-finite value -- it is arithmetic, not
/// a renderer -- so the display layer is where it has to be caught.
fn format_scaling(scaling: Option<f64>) -> String {
    match scaling {
        Some(value) if value.is_finite() => format!("{value:.2}x"),
        _ => "--".to_owned(),
    }
}

/// `numerator / denominator` as a cost ratio, or `--` when either is missing.
///
/// Guards the denominator rather than trusting it: a shape that failed to run
/// reports zero, and a division by it would print `inf` or `NaN` in a column a
/// reader would otherwise take for a measurement.
fn format_ratio(numerator: Option<Run>, denominator: Option<Run>) -> String {
    match (numerator, denominator) {
        (Some(numerator), Some(denominator)) if denominator.nanos_per_push > 0.0 => {
            format!(
                "{:.2}x",
                numerator.nanos_per_push / denominator.nanos_per_push
            )
        }
        _ => "--".to_owned(),
    }
}

fn format_nanos(run: Option<Run>) -> String {
    run.map_or_else(
        || "--".to_owned(),
        |run| format!("{:.1}", run.nanos_per_push),
    )
}
