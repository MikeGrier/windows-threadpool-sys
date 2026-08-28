// Copyright (c) Mike Grier.

//! Prints how a private thread pool grows when its workers are blocked.
//!
//! The tests assert the shape of this (the pool reaches its maximum, does not
//! exceed it, and gets there promptly); this binary prints the numbers, which
//! is what a new architecture actually needs to be re-measured against.

use windows_platform_probes::pool_growth::{measure_growth, measure_raise_while_saturated};

fn report(label: &str, maximum: u32, submissions: usize, runs_long: bool) {
    let observed = measure_growth(maximum, submissions, runs_long);

    println!("{label}");
    println!(
        "  max {} / submitted {} -> started while blocked {}, distinct threads {}",
        observed.maximum,
        observed.submitted,
        observed.started_while_blocked,
        observed.distinct_threads
    );
    println!(
        "  saturated {} | one thread each {} | slowest arrival {:?}",
        observed.saturated(),
        observed.one_thread_each(),
        observed.slowest_arrival()
    );
    println!("  arrivals (us): {:?}", observed.arrivals_us);
}

fn main() {
    println!("== how a blocked pool grows ==\n");

    report("P1 growth curve, max 4:", 4, 8, false);
    println!();
    report("P1 growth curve, max 8:", 8, 16, false);
    println!();
    report("P2 the same, runs-long:", 4, 8, true);
    println!();

    let delay = measure_raise_while_saturated(2, 6, 8);
    println!("P3 raise while saturated (2 -> 6):");
    println!("  extra work started {delay:?} after the maximum was raised");

    println!("\nnote: every number here is from this host and this Windows build.");
    println!("Re-run on another architecture rather than assuming they carry over.");
}
