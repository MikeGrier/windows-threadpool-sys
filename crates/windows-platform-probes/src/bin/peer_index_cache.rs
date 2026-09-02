// Copyright (c) Mike Grier.

//! Prints what caching the peer's index buys an SPSC ring.
//!
//! **An experiment, not a component.** These probes measure platform behaviour
//! and are not for production use. Do not call them from production code, and
//! do not lift a technique out of here. See this crate's DESIGN-NOTES.md.

use std::fmt::Write as _;
use windows_placement_probe::peer_index_cache::{CAPACITY, ITEMS, Strategy, measure};
use windows_platform_probes::report::{Stdout, emit};

fn main() {
    // The only place that names the real stream. Everything below composes
    // text; nothing below knows where it goes.
    emit(&mut Stdout, &render());
}

/// The probe's whole report, as text.
fn render() -> String {
    let mut out = String::new();
    windows_placement_probe::fingerprint::print_banner();
    let _ = writeln!(
        out,
        "== what does caching the peer's index buy an SPSC ring? ==\n"
    );

    let observation = measure();

    let _ = writeln!(
        out,
        "{:<24} {:>10} {:>14} {:>14} {:>14}",
        "configuration", "ns/item", "items/sec", "cons. reads", "prod. reads"
    );
    for run in std::iter::once(&observation.calibration).chain(&observation.strategies) {
        let _ = writeln!(
            out,
            "{:<24} {:>10.1} {:>14.0} {:>14} {:>14}",
            run.label,
            run.nanos_per_item,
            run.items_per_second,
            run.consumer_refreshes,
            run.producer_refreshes
        );
    }
    let _ = writeln!(
        out,
        "
  ({ITEMS} items, capacity {CAPACITY}. The two read columns count how often each
   \
         side actually loaded the *other* side's position -- the shared line the
   \
         technique exists to avoid touching.)"
    );

    let _ = writeln!(out, "\ninterpretation:\n");

    let Some(baseline) = observation.get(Strategy::Baseline) else {
        return out;
    };

    // The model has to reproduce the shipping queue before anything it says
    // about variants is worth reading.
    let drift = (baseline.nanos_per_item - observation.calibration.nanos_per_item).abs()
        / observation.calibration.nanos_per_item;
    let _ = writeln!(
        out,
        "  calibration: the model's baseline differs from the shipping spsc by
  \
         {:.0}% ({:.1} vs {:.1} ns/item).",
        drift * 100.0,
        baseline.nanos_per_item,
        observation.calibration.nanos_per_item
    );
    if drift > 0.25 {
        let _ = writeln!(
            out,
            "  CAUTION: that is a wide gap, so the rows below describe the MODEL"
        );
        let _ = writeln!(
            out,
            "  and not the shipping queue. The model has only the ring mechanics;"
        );
        let _ = writeln!(
            out,
            "  the shipping push also consults the reservation count, updates the"
        );
        let _ = writeln!(
            out,
            "  depth metric and rings the doorbell. This probe does NOT attribute"
        );
        let _ = writeln!(
            out,
            "  the gap between those, and no such attribution should be read into"
        );
        let _ = writeln!(
            out,
            "  it. What the gap does establish is a floor: whatever the shared"
        );
        let _ = writeln!(
            out,
            "  read costs, it is a minority of what the shipping queue spends per"
        );
        let _ = writeln!(out, "  item, so removing it cannot be the large win.");
    } else {
        let _ = writeln!(
            out,
            "  Close enough to treat the model as a stand-in for the real ring."
        );
    }

    for strategy in [Strategy::Cached, Strategy::Warmed] {
        let Some(run) = observation.get(strategy) else {
            continue;
        };
        let speedup = baseline.nanos_per_item / run.nanos_per_item;
        let _ = writeln!(
            out,
            "\n  {:<22} {:.2}x the baseline ({:.1} -> {:.1} ns/item)",
            match strategy {
                Strategy::Cached => "peer-index caching:",
                Strategy::Warmed => "warming load only:",
                Strategy::Baseline => unreachable!(),
            },
            speedup,
            baseline.nanos_per_item,
            run.nanos_per_item
        );
    }

    // Everything below is DERIVED from this run's numbers, and none of it may
    // go back to being prose.
    //
    // It used to be a fixed paragraph concluding that the technique "WORKED and
    // still lost", that consumer reads fell "roughly 3.6x", and that producer
    // reads "go UP". Those were true of the x64 host it was written on. Run on
    // an ARM64 host they were all three false -- caching was 17x FASTER, and
    // producer reads fell by ~580x -- and the probe printed the old conclusion
    // anyway, contradicting the table directly above it. An instrument that
    // states its finding regardless of what it measured is worse than no
    // instrument, because it is believed.
    let Some(cached) = observation.get(Strategy::Cached) else {
        return out;
    };

    // The batch depth is the mechanism, so compute it rather than assert it: it
    // is how many items each shared read is amortised over, and it is what
    // decides whether trading freshness for fewer reads pays.
    let consumer_batch = ITEMS as f64 / cached.consumer_refreshes.max(1) as f64;
    let producer_batch = ITEMS as f64 / cached.producer_refreshes.max(1) as f64;
    let consumer_reduction =
        baseline.consumer_refreshes as f64 / cached.consumer_refreshes.max(1) as f64;
    let producer_reduction =
        baseline.producer_refreshes as f64 / cached.producer_refreshes.max(1) as f64;
    let speedup = baseline.nanos_per_item / cached.nanos_per_item;

    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "  how far each shared read was amortised, with caching on:"
    );
    let _ = writeln!(
        out,
        "    consumer: {consumer_batch:.1} items per read ({consumer_reduction:.1}x fewer reads than baseline)"
    );
    let _ = writeln!(
        out,
        "    producer: {producer_batch:.1} items per read ({producer_reduction:.1}x fewer reads than baseline)"
    );
    let _ = writeln!(out);

    let engaged = consumer_reduction > 1.5;
    if !engaged {
        let _ = writeln!(
            out,
            "  The technique did NOT engage: the consumer's shared reads barely"
        );
        let _ = writeln!(
            out,
            "  moved. Any throughput difference below is noise about something"
        );
        let _ = writeln!(out, "  else, and says nothing about peer-index caching.");
    } else if speedup >= 1.1 {
        let _ = writeln!(out, "  The technique engaged AND won, by {speedup:.2}x.");
        let _ = writeln!(
            out,
            "  Peer-index caching trades freshness for fewer reads, and that"
        );
        let _ = writeln!(
            out,
            "  trade pays when the batch it amortises over is deep. At the"
        );
        let _ = writeln!(out, "  depths above it is paying.");
    } else if speedup <= 0.9 {
        let _ = writeln!(
            out,
            "  The technique engaged and still LOST, at {speedup:.2}x the baseline."
        );
        let _ = writeln!(
            out,
            "  This is a real result about the shape rather than a failed"
        );
        let _ = writeln!(
            out,
            "  implementation. Caching trades freshness for fewer reads; at the"
        );
        let _ = writeln!(
            out,
            "  batch depths above, each side idles on a stale bound it could"
        );
        let _ = writeln!(
            out,
            "  have refreshed, and that idling costs more than the reads saved."
        );
        if producer_reduction < 1.0 {
            let _ = writeln!(
                out,
                "  Note the producer count went UP: a cached index is consulted"
            );
            let _ = writeln!(
                out,
                "  only when it says 'no room', so a blocked producer refreshes on"
            );
            let _ = writeln!(out, "  every spin and gains nothing.");
        }
    } else {
        let _ = writeln!(
            out,
            "  The technique engaged and changed throughput by {speedup:.2}x, which"
        );
        let _ = writeln!(
            out,
            "  is inside the noise of this probe. Treat it as no effect."
        );
    }

    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "  BATCH DEPTH IS THE VARIABLE, AND IT IS NOT A CONSTANT OF THE CODE."
    );
    let _ = writeln!(
        out,
        "  It depends on how the producer and consumer interleave, which"
    );
    let _ = writeln!(
        out,
        "  depends on the host: core count, whether siblings share a core,"
    );
    let _ = writeln!(
        out,
        "  and how the scheduler places the two threads. The same binary has"
    );
    let _ = writeln!(
        out,
        "  measured a depth near 1 on one machine and in the hundreds on"
    );
    let _ = writeln!(
        out,
        "  another, and the verdict inverted with it. Do not carry a"
    );
    let _ = writeln!(
        out,
        "  conclusion from one host to another -- run it on the host you"
    );
    let _ = writeln!(out, "  intend to make the decision for.");

    let Some(warmed) = observation.get(Strategy::Warmed) else {
        return out;
    };
    let warm_reduction =
        baseline.consumer_refreshes as f64 / warmed.consumer_refreshes.max(1) as f64;
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "  control (warming load): {:.2}x throughput, {:.2}x fewer consumer reads.",
        baseline.nanos_per_item / warmed.nanos_per_item,
        warm_reduction
    );
    if warm_reduction < 1.5 {
        let _ = writeln!(
            out,
            "  It removed no shared read, which is what a control should do. A"
        );
        let _ = writeln!(
            out,
            "  discarded load cannot help: the authoritative load still happens,"
        );
        let _ = writeln!(
            out,
            "  and in a tight handoff loop the prefetch has no time to land."
        );
        let _ = writeln!(
            out,
            "  So the technique works by REMOVING the load, not by warming it."
        );
    } else {
        let _ = writeln!(
            out,
            "  UNEXPECTED: the control removed shared reads, so it is not acting"
        );
        let _ = writeln!(
            out,
            "  as a control. Distrust the comparison above until that is explained."
        );
    }
    out
}
