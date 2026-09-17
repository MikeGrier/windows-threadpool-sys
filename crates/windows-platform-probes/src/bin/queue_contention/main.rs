// Copyright (c) Mike Grier.

//! Prints how the array queue's tail claim behaves as producers are added.
//!
//! **An experiment, not a component.** These probes measure platform behaviour
//! and are not for production use. Do not call them from production code, and
//! do not lift a technique out of here. See this crate's
//! [DESIGN-NOTES.md](../../DESIGN-NOTES.md).
//!
//! This reports observations that bear on two questions otherwise settled by
//! taste: whether the linked and sharded MPSC shapes are ever needed, and
//! whether `slotwise_mpsc` and `reserving_mpsc` should merge. It does not settle
//! either -- see `queue_contention`'s module docs for what the regimes can and
//! cannot separate.

use windows_platform_probes::queue_contention::{
    DRAINED_CAPACITY, Observation, PRODUCER_COUNTS, PUSHES_PER_PRODUCER, REPETITIONS, format_nanos,
    format_ratio_bounded, format_scaling_bounded, measure, ratio_column_width, render_table,
    shapes,
};
use windows_platform_probes::report::emit_report;

fn main() {
    // The probe's whole output policy, and it is one line: hand the renderer to
    // the sink. Nothing here or below names a stream -- that is chosen once, in
    // `report`, so retargeting a probe is not a rewrite.
    emit_report(render);
}

#[cfg(test)]
mod tests;

/// Measure, then render what was measured.
///
/// **The two halves are separate so the second one can be tested.** Rendering
/// used to measure inside itself, which made the only way to exercise it a
/// ~65-second host-dependent run -- so the table assembly, the derived column
/// widths, the `cfg`-gated rows and the prose between them were reached by
/// nothing in the suite. Two defects shipped through that gap on this branch:
/// two tables hard-coded a column width for formatters whose output has no
/// fixed maximum, and the drained footer printed a seven-run result beneath a
/// table produced by one invocation. See `M4.7`.
fn render(out: &mut dyn std::fmt::Write) {
    // The banner is the one line that is not a function of the observation --
    // it is a fresh topology read -- so it is written here and
    // `render_observation` stays a pure function of what was measured. That
    // purity is the whole point of the split: it is what lets a fixture drive
    // the entire report.
    //
    // First line of the report, and part of the returned text rather than
    // written out here: a captured report must carry the line naming the
    // machine that produced it, and the taint marker with it.
    let _ = writeln!(
        out,
        "{}",
        windows_placement_probe::fingerprint::banner_line()
    );
    render_observation(out, &measure());
}

/// Everything the report says about an observation.
fn render_observation(out: &mut dyn std::fmt::Write, observation: &Observation) {
    let _ = writeln!(
        out,
        "== how does the array queue's push path scale with producer count? ==\n"
    );

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
    // was. The dispersion belongs here as well, and now is: each row carries the
    // range across its repetitions and the resulting spread. What M4.2 still
    // covers is making the sampling parameters settable rather than fixed.
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
        "\n-- drained: a consumer looping on pop, capacity {DRAINED_CAPACITY} --"
    );
    render_table(out, &observation.drained);

    let _ = writeln!(out, "\ninterpretation:\n");

    // Question 1: does the claim collapse as producers are added?
    let _ = writeln!(
        out,
        "  1. push-path scaling with producer count (isolated regime)\n"
    );
    // Same two-pass shape as the claim-layout table below, and for the same
    // reason: `format_scaling_bounded` renders a point plus a measured interval,
    // whose width follows the data and has no fixed maximum. See
    // `ratio_column_width`.
    let scaling_rows: Vec<(usize, [String; 4])> = PRODUCER_COUNTS
        .iter()
        .map(|&producers| {
            let cell = |shape: &str| {
                format_scaling_bounded(
                    observation.scaling(&observation.isolated, shape, producers),
                    observation.scaling_bounds(&observation.isolated, shape, producers),
                )
            };
            (
                producers,
                [
                    cell(shapes::SLOTWISE_MPSC),
                    cell(shapes::RESERVING_MPSC),
                    cell(shapes::PERMIT_MPSC),
                    cell(shapes::BASELINE_FETCH_ADD),
                ],
            )
        })
        .collect();
    let w = ratio_column_width(
        scaling_rows
            .iter()
            .flat_map(|(_, cells)| cells)
            .map(String::as_str),
    );
    let _ = writeln!(
        out,
        "     {:<12} {:>w$} {:>w$} {:>w$} {:>w$}",
        "producers", "slotwise", "reserving", "permit", "atomic floor"
    );
    for (producers, cells) in &scaling_rows {
        let _ = writeln!(
            out,
            "     {producers:<12} {:>w$} {:>w$} {:>w$} {:>w$}",
            cells[0], cells[1], cells[2], cells[3],
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
    // Two-pass again: both ratio columns hold `format_ratio_bounded` output,
    // whose width follows the measured span. The header labels join the
    // derivation rather than being assumed to fit, so the column is correct by
    // construction instead of by the floor happening to exceed them.
    let drained_rows: Vec<(usize, String, String, String, String, String)> = PRODUCER_COUNTS
        .iter()
        .map(|&producers| {
            let plain = observation.find(&observation.drained, shapes::SLOTWISE_MPSC, producers);
            let reserving =
                observation.find(&observation.drained, shapes::RESERVING_MPSC, producers);
            let permit = observation.find(&observation.drained, shapes::PERMIT_MPSC, producers);
            (
                producers,
                format_nanos(plain),
                format_nanos(reserving),
                format_ratio_bounded(reserving, plain),
                format_nanos(permit),
                // The column SH-15.5 exists to fill: the experimental claim
                // against the shipping shape it would replace. Below 1.00 means
                // the permit claim is cheaper; above means removing the
                // room-decision race costs throughput.
                format_ratio_bounded(permit, reserving),
            )
        })
        .collect();
    let r = ratio_column_width(
        drained_rows
            .iter()
            .flat_map(|(_, _, _, ratio, _, permit_ratio)| [ratio.as_str(), permit_ratio.as_str()])
            .chain(["reserving/slotwise", "permit/reserving", "ratio [bound]"]),
    );
    let _ = writeln!(
        out,
        "     {:<10} {:>12} {:>12} {:>r$} {:>12} {:>r$}",
        "producers", "slotwise", "reserving", "reserving/slotwise", "permit", "permit/reserving"
    );
    let _ = writeln!(
        out,
        "     {:<10} {:>12} {:>12} {:>r$} {:>12} {:>r$}",
        "", "ns/op", "ns/op", "ratio [bound]", "ns/op", "ratio [bound]"
    );
    for (producers, plain, reserving, ratio, permit, permit_ratio) in &drained_rows {
        let _ = writeln!(
            out,
            "     {producers:<10} {:>12} {:>12} {:>r$} {:>12} {:>r$}",
            plain, reserving, ratio, permit, permit_ratio,
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
    // Counted from what was actually MEASURED rather than from what is present:
    // the 64/64 rows are cfg-elided on a target with no native 128-bit exchange,
    // and a hardcoded "four" would be false there -- but a row can also be
    // present while carrying the did-not-run sentinel, which `render_table`
    // marks `--` and this sentence would otherwise still count. See
    // `Observation::count_measured`.
    let layouts_measured = observation.count_measured(
        &observation.isolated,
        &[
            shapes::CLAIM_NARROW,
            shapes::CLAIM_DEEP,
            shapes::CLAIM_PERPETUAL,
            shapes::CLAIM_WIDE,
        ],
        PRODUCER_COUNTS[0],
    );
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
        "     what 'no difference' looks like on this host -- read it against"
    );
    let _ = writeln!(
        out,
        "     the layout rows before calling any of them apart."
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
        "     8/56 moving it to 2^56. Both defer the recurrence rather than"
    );
    let _ = writeln!(out, "     removing it. Not the exchange in isolation.\n");
    for (label, regime) in [
        ("isolated", &observation.isolated),
        ("drained", &observation.drained),
    ] {
        let _ = writeln!(out, "     -- {label} --");
        // Every cell is rendered before the header is emitted, because the ratio
        // column's width is derived from the widest value it must hold. A Rust
        // width is a minimum, so sizing the header first and discovering a wider
        // cell later does not truncate that cell -- it silently pushes the two
        // columns after it out of line. See `ratio_column_width`.
        let rows: Vec<(usize, [String; 4], [String; 3])> = PRODUCER_COUNTS
            .iter()
            .map(|&producers| {
                let narrow = observation.find(regime, shapes::CLAIM_NARROW, producers);
                let deep = observation.find(regime, shapes::CLAIM_DEEP, producers);
                let perpetual = observation.find(regime, shapes::CLAIM_PERPETUAL, producers);
                let wide = observation.find(regime, shapes::CLAIM_WIDE, producers);
                (
                    producers,
                    [
                        format_nanos(narrow),
                        format_nanos(deep),
                        format_nanos(perpetual),
                        format_nanos(wide),
                    ],
                    [
                        format_ratio_bounded(deep, narrow),
                        format_ratio_bounded(perpetual, narrow),
                        format_ratio_bounded(wide, narrow),
                    ],
                )
            })
            .collect();
        let w = ratio_column_width(
            rows.iter()
                .flat_map(|(_, _, ratios)| ratios)
                .map(String::as_str)
                .chain(["16/48 vs", "8/56 vs", "64/64 vs"]),
        );
        let _ = writeln!(
            out,
            "     {:<10} {:>11} {:>11} {:>11} {:>11} {:>w$} {:>w$} {:>w$}",
            "producers",
            "32/32 ns/op",
            "16/48 ns/op",
            "8/56 ns/op",
            "64/64 ns/op",
            "16/48 vs",
            "8/56 vs",
            "64/64 vs",
        );
        for (producers, nanos, ratios) in &rows {
            let _ = writeln!(
                out,
                "     {:<10} {:>11} {:>11} {:>11} {:>11} {:>w$} {:>w$} {:>w$}",
                producers, nanos[0], nanos[1], nanos[2], nanos[3], ratios[0], ratios[1], ratios[2],
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
        "  many refusals met a full queue often, so the consumer is one term"
    );
    let _ = writeln!(
        out,
        "  in what it measured. That does not rule the tail out -- both can"
    );
    let _ = writeln!(
        out,
        "  bind at once, and these counts do not separate them."
    );
}
