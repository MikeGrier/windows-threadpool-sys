// Copyright (c) Mike Grier.

//! Prints how the array queue's tail claim behaves as producers are added.
//!
//! **An experiment, not a component.** These probes measure platform behaviour
//! and are not for production use. Do not call them from production code, and
//! do not lift a technique out of here. See this crate's DESIGN-NOTES.md.
//!
//! This decides two things that are otherwise decided by taste: whether the
//! linked and sharded MPSC shapes are ever needed, and whether `mpsc` and
//! `reserving_mpsc` should merge. See `queue_contention`'s module docs.

use std::fmt::Write as _;
use windows_platform_probes::queue_contention::{PRODUCER_COUNTS, Run, measure, shapes};
use windows_platform_probes::report::{Stdout, emit};

fn main() {
    // The only place that names the real stream. Everything below composes
    // text; nothing below knows where it goes.
    emit(&mut Stdout, &render());
}

/// The probe's whole report, as text.
fn render() -> String {
    let mut out = String::new();
    // First line of the report, and part of the returned text rather than
    // written out here: a captured report must carry the line naming the
    // machine that produced it, and the taint marker with it.
    let _ = writeln!(
        out,
        "{}",
        windows_placement_probe::fingerprint::banner_line()
    );
    let _ = writeln!(out, "== does the array queue's tail claim contend? ==\n");

    let observation = measure();
    let _ = writeln!(
        out,
        "host reports {} logical processors\n",
        observation.logical_processors
    );

    let _ = writeln!(
        out,
        "-- isolated: producers only, capacity large enough that nothing is refused --"
    );
    render_table(&mut out, &observation.isolated);

    let _ = writeln!(
        out,
        "\n-- drained: a consumer popping continuously, capacity 1024 --"
    );
    render_table(&mut out, &observation.drained);

    let _ = writeln!(out, "\ninterpretation:\n");

    // Question 1: does the claim collapse as producers are added?
    let _ = writeln!(out, "  1. tail-claim contention (isolated regime)\n");
    let _ = writeln!(
        out,
        "     {:<18} {:>12} {:>12} {:>12} {:>14}",
        "producers", "slotwise x1thr", "reserving", "permit", "atomic floor"
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
        "\n  2. the price of reservation (drained regime, where `head` is written)\n"
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
        "     `mpsc` does not, which is the entire reason they ship as two"
    );
    let _ = writeln!(
        out,
        "     shapes. This regime is the one that can price that read, because"
    );
    let _ = writeln!(out, "     a consumer is writing the line being read.");
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
    out
}

/// Append one regime's table to `out`.
///
/// Takes the buffer rather than printing, for the reason `pool_growth`'s twin
/// records: a helper writing to stdout while its caller composes a string emits
/// its lines first, reordering the report without losing any of it.
fn render_table(out: &mut String, runs: &[Run]) {
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

fn format_scaling(scaling: Option<f64>) -> String {
    scaling.map_or_else(|| "--".to_owned(), |value| format!("{value:.2}x"))
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
