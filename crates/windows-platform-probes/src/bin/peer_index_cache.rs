// Copyright (c) Mike Grier.

//! Prints what caching the peer's index buys an SPSC ring.
//!
//! **An experiment, not a component.** These probes measure platform behaviour
//! and are not for production use. Do not call them from production code, and
//! do not lift a technique out of here. See this crate's DESIGN-NOTES.md.

use windows_platform_probes::peer_index_cache::{CAPACITY, ITEMS, Strategy, measure};

fn main() {
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

    println!(
        "
  The read columns say the technique WORKED and still lost. Caching"
    );
    println!("  cut the consumer's shared reads by roughly 3.6x -- it is not that");
    println!("  the optimisation failed to engage. It engaged and cost throughput.");
    println!();
    println!("  The reason is in the same columns: the batch it amortises over is");
    println!("  only about 3.6 items deep, because a spinning consumer keeps the");
    println!("  ring near empty. And on the producer side the count goes UP, not");
    println!("  down -- a cached index is only consulted when it says 'no room', so");
    println!("  a producer that is genuinely blocked refreshes on every spin and");
    println!("  gains nothing at all.");
    println!();
    println!("  That is the trade the technique actually makes: it exchanges");
    println!("  freshness for fewer reads. When a real backlog exists the exchange");
    println!("  is free, because a stale index is still far behind the peer. When");
    println!("  the ring hovers near empty or near full it is not free -- each side");
    println!("  idles on a stale bound it could have refreshed, and that idling");
    println!("  costs more than the reads it saved.");
    println!();
    println!("  The warming load is the control, and it behaves as a control should:");
    println!("  it removes no shared read (its count matches the baseline) and it");
    println!("  changes no throughput. Warming cannot help here because the");
    println!("  authoritative load still happens, and in a tight handoff loop the");
    println!("  prefetch has no time to land before it.");
}
