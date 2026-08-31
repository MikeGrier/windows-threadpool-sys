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

use windows_platform_probes::queue_contention::{PRODUCER_COUNTS, Run, measure};

fn main() {
    println!("== does the array queue's tail claim contend? ==\n");

    let observation = measure();
    println!(
        "host reports {} logical processors\n",
        observation.logical_processors
    );

    println!("-- isolated: producers only, capacity large enough that nothing is refused --");
    print_table(&observation.isolated);

    println!("\n-- drained: a consumer popping continuously, capacity 1024 --");
    print_table(&observation.drained);

    println!("\ninterpretation:\n");

    // Question 1: does the claim collapse as producers are added?
    println!("  1. tail-claim contention (isolated regime)\n");
    println!(
        "     {:<18} {:>12} {:>12} {:>14}",
        "producers", "mpsc x1thr", "reserving", "atomic floor"
    );
    for &producers in PRODUCER_COUNTS {
        let mpsc = observation.scaling(&observation.isolated, "mpsc", producers);
        let reserving = observation.scaling(&observation.isolated, "reserving_mpsc", producers);
        let floor = observation.scaling(&observation.isolated, "baseline_fetch_add", producers);
        println!(
            "     {producers:<18} {:>12} {:>12} {:>14}",
            format_scaling(mpsc),
            format_scaling(reserving),
            format_scaling(floor)
        );
    }
    println!("\n     Read as: throughput at N producers divided by throughput at one.");
    println!("     1.00 means N threads together push no faster than one did.");
    println!("     The atomic floor is the cheapest possible contended operation,");
    println!("     so it says how much of any curve is the queue and how much is");
    println!("     simply what this processor does to a fought-over cache line.");

    // Question 2: what does reserving_mpsc's read of `head` actually cost?
    println!("\n  2. the price of reservation (drained regime, where `head` is written)\n");
    println!(
        "     {:<18} {:>14} {:>14} {:>10}",
        "producers", "mpsc ns/push", "reserving", "ratio"
    );
    for &producers in PRODUCER_COUNTS {
        let plain = observation.find(&observation.drained, "mpsc", producers);
        let reserving = observation.find(&observation.drained, "reserving_mpsc", producers);
        let ratio = match (plain, reserving) {
            (Some(plain), Some(reserving)) if plain.nanos_per_push > 0.0 => {
                format!("{:.2}x", reserving.nanos_per_push / plain.nanos_per_push)
            }
            _ => "--".to_owned(),
        };
        println!(
            "     {producers:<18} {:>14} {:>14} {:>10}",
            format_nanos(plain),
            format_nanos(reserving),
            ratio
        );
    }
    println!("\n     `reserving_mpsc` reads the consumer's position on every push and");
    println!("     `mpsc` does not, which is the entire reason they ship as two");
    println!("     shapes. This regime is the one that can price that read, because");
    println!("     a consumer is writing the line being read.");

    println!("\n  CAUTION: the drained regime has ONE consumer, because that is what");
    println!("  MPSC means. At high producer counts it is expected to become");
    println!("  consumer-bound, and a plateau there says nothing about the claim.");
    println!("  The refusal counts above are what make that visible: a run with");
    println!("  many refusals was waiting for the consumer, not for the tail.");
}

fn print_table(runs: &[Run]) {
    println!(
        "{:<18} {:>10} {:>14} {:>16} {:>14}",
        "shape", "producers", "ns/push", "pushes/sec", "refusals"
    );
    for run in runs {
        println!(
            "{:<18} {:>10} {:>14.1} {:>16.0} {:>14}",
            run.shape, run.producers, run.nanos_per_push, run.pushes_per_second, run.refusals
        );
    }
}

fn format_scaling(scaling: Option<f64>) -> String {
    scaling.map_or_else(|| "--".to_owned(), |value| format!("{value:.2}x"))
}

fn format_nanos(run: Option<Run>) -> String {
    run.map_or_else(
        || "--".to_owned(),
        |run| format!("{:.1}", run.nanos_per_push),
    )
}
