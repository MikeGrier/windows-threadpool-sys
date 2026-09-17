// Copyright (c) Mike Grier.

//! Prints what caching the peer's index buys an SPSC ring.
//!
//! **An experiment, not a component.** These probes measure platform behaviour
//! and are not for production use. Do not call them from production code, and
//! do not lift a technique out of here. See this crate's DESIGN-NOTES.md.

use windows_placement_probe::peer_index_cache::{CAPACITY, ITEMS, Strategy, measure};
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
        "== what does caching the peer's index buy an SPSC ring? ==\n"
    );

    // Measured after the banner and heading are already out, so a host that
    // refuses still gives a reader the line naming which probe declined and on
    // what machine.
    //
    // **This arm is required by the type, not reachable today.**
    // `peer_index_cache::measure` starts only unpinned runs, so nothing it does
    // can currently return `Err`; the handler exists because the signature is
    // `Result` and deleting it would mean discarding the error instead. Said
    // here because an earlier version of this comment implied this binary could
    // decline, which it cannot -- `probe-core-affinity` is the one that pins.
    let observation = match measure() {
        Ok(observation) => observation,
        Err(error) => {
            let _ = writeln!(
                out,
                "this host declined to be measured:\n{}",
                error.to_string().trim_end()
            );
            let _ = writeln!(
                out,
                "Nothing below could be measured, so nothing below is reported. This is\n\
                 a refusal to measure the host, not a finding about it."
            );
            return;
        }
    };

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
        return;
    };

    // The model has to reproduce the shipping queue before anything it says
    // about variants is worth reading.
    //
    // `signed` and `drift` are kept apart deliberately. The magnitude decides
    // whether to caution at all; the SIGN decides what may then be said, and
    // conflating them printed a conclusion backwards. The floor argument below
    // reads "the shipping queue spends much more per item than this model, so
    // whatever the shared read costs it is a minority of that" -- which holds
    // only when the model is the FASTER of the two. Taken on `abs()` alone it
    // fired just as readily when the model was slower, where the same words
    // assert a floor the measurement does not support.
    let signed = (baseline.nanos_per_item - observation.calibration.nanos_per_item)
        / observation.calibration.nanos_per_item;
    let drift = signed.abs();
    let _ = writeln!(
        out,
        "  calibration: the model's baseline differs from the shipping spsc by
  \
         {:.0}% ({:.1} vs {:.1} ns/item).",
        drift * 100.0,
        baseline.nanos_per_item,
        observation.calibration.nanos_per_item
    );
    if drift > 0.25 && signed < 0.0 {
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
        // **The bound is the model's SHARE of shipping cost, not a verdict about
        // it.** This said the shared read "is a minority of what the shipping
        // queue spends per item, so removing it cannot be the large win", which
        // does not follow from the gap being wide: the shared read sits inside
        // the model baseline, so what the calibration bounds is the share the
        // whole model occupies. At a 39% gap that share is 61% -- a majority,
        // under a sentence asserting a minority. Correcting the SIGN in the
        // previous round left the arithmetic wrong, because the sign was only
        // half of what made the claim unsupported.
        //
        // So report the share and stop. A reader with a threshold in mind can
        // apply it; this run does not have one.
        let _ = writeln!(
            out,
            "  it. What the gap does bound is a share: the shared read sits"
        );
        let _ = writeln!(
            out,
            "  inside the model's baseline, so whatever it costs, it is at most"
        );
        let _ = writeln!(
            out,
            "  {:.0}% of what the shipping queue spends per item.",
            (baseline.nanos_per_item / observation.calibration.nanos_per_item) * 100.0
        );
    } else if drift > 0.25 {
        let _ = writeln!(
            out,
            "  CAUTION: that is a wide gap, so the rows below describe the MODEL"
        );
        let _ = writeln!(
            out,
            "  and not the shipping queue -- and the model is the SLOWER of the"
        );
        let _ = writeln!(
            out,
            "  two, which is the direction that carries no floor argument. The"
        );
        let _ = writeln!(
            out,
            "  model is the shipping queue with work stripped out, so costing"
        );
        let _ = writeln!(
            out,
            "  MORE per item than the thing it strips from means it is not"
        );
        let _ = writeln!(
            out,
            "  measuring what it was built to measure. Nothing below can be read"
        );
        let _ = writeln!(out, "  as a statement about the shipping queue.");
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
            "  Peer-index caching trades freshness for fewer reads. The depths"
        );
        let _ = writeln!(
            out,
            "  above say how far each side's reads were amortised; which side's"
        );
        let _ = writeln!(
            out,
            "  depth carries the win is not separated by this measurement."
        );
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
        return;
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
}
