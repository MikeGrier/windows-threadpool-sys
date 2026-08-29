// Copyright (c) Mike Grier.

//! Prints how a private thread pool grows when its workers are blocked.
//!
//! **An experiment, not a component.** These probes measure platform behaviour
//! and are not for production use: that scope is what lets one do things a
//! shipping component must not. Do not call them from production code, and do
//! not lift a technique out of here. See this crate's DESIGN-NOTES.md.
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

    let raise = measure_raise_while_saturated(2, 6, 8);
    println!("P3 raise while saturated (2 -> 6):");
    if raise.saturated_before_raise() {
        println!(
            "  extra work started {:?} after the maximum was raised",
            raise.delay
        );
    } else {
        println!(
            "  NOT saturated before the raise ({} of {} started), so the delay",
            raise.started_before_raise, raise.base_max
        );
        println!("  below times growth toward the base maximum, not the raise.");
        println!("  elapsed: {:?}", raise.delay);
    }
    if !raise.took_effect {
        println!("  the settle window expired with no extra callback started.");
    }

    println!("\nnote: every number here is from this host and this Windows build.");
    println!("Re-run on another architecture rather than assuming they carry over.");
}
