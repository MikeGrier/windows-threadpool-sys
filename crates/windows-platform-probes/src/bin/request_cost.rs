// Copyright (c) Mike Grier.

//! Prints what a namespace request costs to build, against the queue that would
//! carry it.
//!
//! **An experiment, not a component.** These probes measure platform behaviour
//! and are not for production use. Do not call them from production code, and
//! do not lift a technique out of here. See this crate's DESIGN-NOTES.md.
//!
//! Read alongside `probe-doorbell-cost`: together they say whether the queue's
//! mechanics or the request's allocation model deserves the attention.

use windows_platform_probes::request_cost::measure;

/// Measured by `probe-doorbell-cost` on the same machine. Restated here only to
/// render a ratio; the authoritative number is whatever that probe prints on
/// the host this runs on.
const DOORBELL_NS_REFERENCE: f64 = 164.9;
const ATOMIC_NS_REFERENCE: f64 = 7.2;

fn main() {
    println!("== what does a namespace request cost to build? ==\n");

    let observation = measure();

    println!(
        "{:<26} {:>10} {:>14} {:>16}",
        "operation", "ns/op", "x an atomic", "x a doorbell"
    );
    for timing in &observation.timings {
        println!(
            "{:<26} {:>10.1} {:>14.1} {:>16.2}",
            timing.label,
            timing.nanos_per_op,
            timing.nanos_per_op / ATOMIC_NS_REFERENCE,
            timing.nanos_per_op / DOORBELL_NS_REFERENCE,
        );
    }
    println!(
        "\n(ratios use the reference doorbell {DOORBELL_NS_REFERENCE:.1} ns and atomic \
         {ATOMIC_NS_REFERENCE:.1} ns measured\n by probe-doorbell-cost on the development \
         machine; re-read that probe on this host\n before trusting them)"
    );

    println!("\ninterpretation:");

    let build = observation.get("build_open_request");
    let capture = observation.get("capture_handle");

    if let Some(build) = build {
        println!(
            "  building a pathed request costs {build:.0} ns, which is {:.1}x one",
            build / DOORBELL_NS_REFERENCE
        );
        println!("  doorbell.");
        println!();
        println!("  SCOPE, because this is easy to over-read: that is a statement about");
        println!("  ONE OPERATION TYPE, not about the queue. A namespace open is the");
        println!("  heaviest payload the queue carries -- it resolves a path through");
        println!("  Win32 and may duplicate a handle -- and it ends in a CreateFileW");
        println!("  costing microseconds regardless. A registered-buffer read, which is");
        println!("  the hot path, carries no path and no handle: its descriptor is a slot");
        println!("  index and an offset, and there the queue's own mechanics are the");
        println!("  whole cost.");
        println!();
        println!("  Nor is per-operation overhead the same thing as queue efficiency.");
        println!("  Throughput under contention, cache behaviour, batching amortization");
        println!("  and backpressure decide that, and a single uncontended construction");
        println!("  time measures none of them.");
        println!();
        println!("  What it does support: for an open-heavy workload, doorbell tuning");
        println!("  would be optimizing the small half. That is a finding about");
        println!("  OPERATION MIX, and it says nothing about the read path.");
    }

    if let Some(capture) = capture {
        println!("\n  duplicating a handle costs {capture:.0} ns -- a kernel transition, not");
        println!("  a memory copy, and easy to under-count when thinking about what an");
        println!("  SQE holds.");
        if let Some(build) = build {
            if capture > build {
                println!(
                    "  It is {:.1}x the cost of building the pathed request itself, so a",
                    capture / build
                );
                println!("  request carrying a handle is dominated by the duplication, and");
                println!("  any allocation tuning on the path would be optimizing the wrong");
                println!("  half.");
            } else {
                println!(
                    "  It is {:.2}x the pathed request, so the two are comparable and",
                    capture / build
                );
                println!("  neither dominates.");
            }
        }
    }

    // The split that decides whether an allocator change can help at all.
    if let Some(build) = build
        && let Some(clone) = observation.get("clone_prepared_units")
        && build > clone
    {
        println!("\n  WHERE THE TIME ACTUALLY GOES, and it is not the allocator:");
        println!("  `prepare` calls GetFullPathNameW to resolve the path against the");
        println!("  process working directory -- a Win32 call, because the CWD is mutable");
        println!("  by any thread and resolving later would be racy. So most of the cost");
        println!("  above is a syscall that no allocation scheme can remove.");
        println!("  Cloning already-prepared units is {clone:.0} ns, which bounds what an");
        println!(
            "  inline-storage or recycling scheme could recover at {:.0} ns per request",
            build - clone
        );
        println!("  AT MOST -- and only for a caller that can reuse a resolved path.");
        println!("  A caller with a fresh path each time pays the resolution regardless.");
    }

    let get = |label: &str| {
        observation
            .get(label)
            .map_or("null".to_string(), |n| format!("{n:.1}"))
    };
    println!(
        concat!(
            r#"{{"reason":"x-probe-request-cost","arch":"{}","prepare_short_ns":{},"#,
            r#""prepare_long_ns":{},"build_open_request_ns":{},"#,
            r#""clone_prepared_units_ns":{},"capture_handle_ns":{}}}"#
        ),
        std::env::consts::ARCH,
        get("prepare_short_path"),
        get("prepare_long_path"),
        get("build_open_request"),
        get("clone_prepared_units"),
        get("capture_handle"),
    );
}
