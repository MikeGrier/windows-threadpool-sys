// Copyright (c) Mike Grier.

//! Prints what caching the peer's index buys an SPSC ring.
//!
//! **An experiment, not a component.** These probes measure platform behaviour
//! and are not for production use. Do not call them from production code, and
//! do not lift a technique out of here. See this crate's DESIGN-NOTES.md.

use windows_placement_probe::peer_index_cache::{CAPACITY, ITEMS, Strategy, measure};

fn main() {
    windows_placement_probe::fingerprint::print_banner();
    println!("== what does caching the peer's index buy an SPSC ring? ==\n");

    let observation = measure();

    println!(
        "{:<24} {:>10} {:>14} {:>14} {:>14}",
        "configuration", "ns/item", "items/sec", "cons. reads", "prod. reads"
    );
    for run in std::iter::once(&observation.calibration).chain(&observation.strategies) {
        println!(
            "{:<24} {:>10.1} {:>14.0} {:>14} {:>14}",
            run.label,
            run.nanos_per_item,
            run.items_per_second,
            run.consumer_refreshes,
            run.producer_refreshes
        );
    }
    println!(
        "
  ({ITEMS} items, capacity {CAPACITY}. The two read columns count how often each
   \
         side actually loaded the *other* side's position -- the shared line the
   \
         technique exists to avoid touching.)"
    );

    println!("\ninterpretation:\n");

    let Some(baseline) = observation.get(Strategy::Baseline) else {
        return;
    };

    // The model has to reproduce the shipping queue before anything it says
    // about variants is worth reading.
    let drift = (baseline.nanos_per_item - observation.calibration.nanos_per_item).abs()
        / observation.calibration.nanos_per_item;
    println!(
        "  calibration: the model's baseline differs from the shipping spsc by
  \
         {:.0}% ({:.1} vs {:.1} ns/item).",
        drift * 100.0,
        baseline.nanos_per_item,
        observation.calibration.nanos_per_item
    );
    if drift > 0.25 {
        println!("  CAUTION: that is a wide gap, so the rows below describe the MODEL");
        println!("  and not the shipping queue. The model has only the ring mechanics;");
        println!("  the shipping push also consults the reservation count, updates the");
        println!("  depth metric and rings the doorbell. This probe does NOT attribute");
        println!("  the gap between those, and no such attribution should be read into");
        println!("  it. What the gap does establish is a floor: whatever the shared");
        println!("  read costs, it is a minority of what the shipping queue spends per");
        println!("  item, so removing it cannot be the large win.");
    } else {
        println!("  Close enough to treat the model as a stand-in for the real ring.");
    }

    for strategy in [Strategy::Cached, Strategy::Warmed] {
        let Some(run) = observation.get(strategy) else {
            continue;
        };
        let speedup = baseline.nanos_per_item / run.nanos_per_item;
        println!(
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
        return;
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

    println!();
    println!("  how far each shared read was amortised, with caching on:");
    println!(
        "    consumer: {consumer_batch:.1} items per read ({consumer_reduction:.1}x fewer reads than baseline)"
    );
    println!(
        "    producer: {producer_batch:.1} items per read ({producer_reduction:.1}x fewer reads than baseline)"
    );
    println!();

    let engaged = consumer_reduction > 1.5;
    if !engaged {
        println!("  The technique did NOT engage: the consumer's shared reads barely");
        println!("  moved. Any throughput difference below is noise about something");
        println!("  else, and says nothing about peer-index caching.");
    } else if speedup >= 1.1 {
        println!("  The technique engaged AND won, by {speedup:.2}x.");
        println!("  Peer-index caching trades freshness for fewer reads, and that");
        println!("  trade pays when the batch it amortises over is deep. At the");
        println!("  depths above it is paying.");
    } else if speedup <= 0.9 {
        println!("  The technique engaged and still LOST, at {speedup:.2}x the baseline.");
        println!("  This is a real result about the shape rather than a failed");
        println!("  implementation. Caching trades freshness for fewer reads; at the");
        println!("  batch depths above, each side idles on a stale bound it could");
        println!("  have refreshed, and that idling costs more than the reads saved.");
        if producer_reduction < 1.0 {
            println!("  Note the producer count went UP: a cached index is consulted");
            println!("  only when it says 'no room', so a blocked producer refreshes on");
            println!("  every spin and gains nothing.");
        }
    } else {
        println!("  The technique engaged and changed throughput by {speedup:.2}x, which");
        println!("  is inside the noise of this probe. Treat it as no effect.");
    }

    println!();
    println!("  BATCH DEPTH IS THE VARIABLE, AND IT IS NOT A CONSTANT OF THE CODE.");
    println!("  It depends on how the producer and consumer interleave, which");
    println!("  depends on the host: core count, whether siblings share a core,");
    println!("  and how the scheduler places the two threads. The same binary has");
    println!("  measured a depth near 1 on one machine and in the hundreds on");
    println!("  another, and the verdict inverted with it. Do not carry a");
    println!("  conclusion from one host to another -- run it on the host you");
    println!("  intend to make the decision for.");

    let Some(warmed) = observation.get(Strategy::Warmed) else {
        return;
    };
    let warm_reduction =
        baseline.consumer_refreshes as f64 / warmed.consumer_refreshes.max(1) as f64;
    println!();
    println!(
        "  control (warming load): {:.2}x throughput, {:.2}x fewer consumer reads.",
        baseline.nanos_per_item / warmed.nanos_per_item,
        warm_reduction
    );
    if warm_reduction < 1.5 {
        println!("  It removed no shared read, which is what a control should do. A");
        println!("  discarded load cannot help: the authoritative load still happens,");
        println!("  and in a tight handoff loop the prefetch has no time to land.");
        println!("  So the technique works by REMOVING the load, not by warming it.");
    } else {
        println!("  UNEXPECTED: the control removed shared reads, so it is not acting");
        println!("  as a control. Distrust the comparison above until that is explained.");
    }
}
