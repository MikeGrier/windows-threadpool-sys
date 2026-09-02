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

use std::fmt::Write as _;
use windows_platform_probes::pool_growth::{measure_growth, measure_raise_while_saturated};
use windows_platform_probes::report::{Stdout, emit};

/// Measure one configuration and append its block to `out`.
///
/// Takes the buffer rather than printing: a helper that wrote to stdout while
/// its caller composed a string would emit its lines *before* the caller's,
/// reordering the report even though every line still appeared.
fn report(out: &mut String, label: &str, maximum: u32, submissions: usize, runs_long: bool) {
    let observed = measure_growth(maximum, submissions, runs_long);

    let _ = writeln!(out, "{label}");
    let _ = writeln!(
        out,
        "  max {} / submitted {} -> started while blocked {}, distinct threads {}",
        observed.maximum,
        observed.submitted,
        observed.started_while_blocked,
        observed.distinct_threads
    );
    let _ = writeln!(
        out,
        "  saturated {} | one thread each {} | slowest arrival {:?}",
        observed.saturated(),
        observed.one_thread_each(),
        observed.slowest_arrival()
    );
    let _ = writeln!(out, "  arrivals (us): {:?}", observed.arrivals_us);
}

fn main() {
    // The only place that names the real stream. Everything below composes
    // text; nothing below knows where it goes.
    emit(&mut Stdout, &render());
}

/// The probe's whole report, as text.
fn render() -> String {
    let mut out = String::new();
    let _ = writeln!(out, "== how a blocked pool grows ==\n");

    report(&mut out, "P1 growth curve, max 4:", 4, 8, false);
    let _ = writeln!(out);
    report(&mut out, "P1 growth curve, max 8:", 8, 16, false);
    let _ = writeln!(out);
    report(&mut out, "P2 the same, runs-long:", 4, 8, true);
    let _ = writeln!(out);

    let raise = measure_raise_while_saturated(2, 6, 8);
    let _ = writeln!(out, "P3 raise while saturated (2 -> 6):");
    if raise.saturated_before_raise() {
        let _ = writeln!(
            out,
            "  extra work started {:?} after the maximum was raised",
            raise.delay
        );
    } else {
        let _ = writeln!(
            out,
            "  NOT saturated before the raise ({} of {} started), so the delay",
            raise.started_before_raise, raise.base_max
        );
        let _ = writeln!(
            out,
            "  below times growth toward the base maximum, not the raise."
        );
        let _ = writeln!(out, "  elapsed: {:?}", raise.delay);
    }
    if !raise.took_effect {
        let _ = writeln!(
            out,
            "  the settle window expired with no extra callback started."
        );
    }

    let _ = writeln!(
        out,
        "\nnote: every number here is from this host and this Windows build."
    );
    let _ = writeln!(
        out,
        "Re-run on another architecture rather than assuming they carry over."
    );
    out
}
