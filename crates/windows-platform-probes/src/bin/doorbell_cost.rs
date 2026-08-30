// Copyright (c) Mike Grier.

//! Prints how expensive a doorbell is relative to the syscall it would guard.
//!
//! **An experiment, not a component.** These probes measure platform behaviour
//! and are not for production use. Do not call them from production code, and
//! do not lift a technique out of here. See this crate's DESIGN-NOTES.md.
//!
//! This decides whether the two-layer ring design needs an eventcount at all.
//! If a doorbell is a meaningful fraction of `SubmitIoRing`, the skip-when-busy
//! rules are load-bearing. If it is noise, a simple always-signal queue is
//! adequate and the more delicate protocol -- publish intent, re-check, park --
//! can wait for evidence that it is worth its lost-wakeup risk.

use windows_platform_probes::doorbell_cost::{measure, measure_park_and_wake};

fn main() {
    println!("== what does a doorbell cost, against the syscall it guards? ==\n");

    let observation = measure();

    println!("{:<30} {:>12}", "operation", "ns/op");
    for timing in &observation.timings {
        println!("{:<30} {:>12.1}", timing.label, timing.nanos_per_op);
    }

    let park = measure_park_and_wake(20_000);
    match park {
        Some(ns) => println!("{:<30} {:>12.1}", "park_and_wake round trip", ns),
        None => println!("{:<30} {:>12}", "park_and_wake round trip", "TIMED OUT"),
    }

    println!("\ninterpretation:");

    if let Some(atomic) = observation.get("atomic_fetch_add")
        && let Some(doorbell) = observation.get("set_reset_event")
        && atomic > 0.0
    {
        println!(
            "  a doorbell cycle costs {:.0}x an uncontended atomic ({:.0} ns vs {:.1} ns).",
            doorbell / atomic,
            doorbell,
            atomic
        );
        if let Some(park) = park {
            println!(
                "  an actual park-and-wake round trip costs {:.0}x that again ({:.0} ns),",
                park / doorbell,
                park
            );
            println!("  which is what is paid when the consumer genuinely sleeps.");
        }
    }

    // Deliberately NOT expressed as a share of the empty submit. See below.
    if let Some(submit) = observation.submit_nanos {
        println!("\n  CAUTION: an empty SubmitIoRing measured {submit:.0} ns, which is far too");
        println!("  cheap for a kernel transition -- it is almost certainly short-");
        println!("  circuiting in user mode when there is nothing queued. It is");
        println!("  therefore NOT a fair denominator, and any 'doorbell is N% of a");
        println!("  syscall' figure derived from it would be a confident wrong answer.");
        println!("  The honest denominator is the cost of the real work a submission");
        println!("  carries, which this probe does not measure.");
    }

    // What can be said without a denominator: how much batching it takes for
    // the doorbell to disappear, which is the lever the design actually has.
    if let Some(doorbell) = observation.get("set_reset_event")
        && let Some(atomic) = observation.get("atomic_fetch_add")
        && atomic > 0.0
    {
        println!("\n  batching is the lever, and it is a strong one. One doorbell per");
        println!("  drained batch costs, per operation:");
        for batch in [1_u32, 8, 32, 128] {
            println!(
                "    batch of {batch:>4}: {:>7.1} ns/op ({:.1}x an atomic)",
                doorbell / f64::from(batch),
                doorbell / f64::from(batch) / atomic
            );
        }
        let break_even = (doorbell / atomic).ceil() as u32;
        println!("  so at a batch of about {break_even}, the doorbell costs less per");
        println!("  operation than the atomic push it accompanies.");
    }

    println!("\n  => The skip-when-busy rule is a refinement, not a prerequisite.");
    println!("     Batching alone drives the doorbell below the cost of the push,");
    println!("     so a first implementation can always-signal and stay honest.");
    println!("     Adopt the eventcount when a measurement against real work");
    println!("     justifies its lost-wakeup risk -- not before.");

    let atomic = observation.get("atomic_fetch_add").unwrap_or(f64::NAN);
    let already = observation
        .get("set_event_already_signalled")
        .unwrap_or(f64::NAN);
    let cycle = observation.get("set_reset_event").unwrap_or(f64::NAN);
    let wait0 = observation.get("wait_zero_signalled").unwrap_or(f64::NAN);
    println!(
        concat!(
            r#"{{"reason":"x-probe-doorbell-cost","arch":"{}","atomic_ns":{:.1},"#,
            r#""set_event_already_signalled_ns":{:.1},"set_reset_event_ns":{:.1},"#,
            r#""wait_zero_signalled_ns":{:.1},"park_and_wake_round_trip_ns":{},"#,
            r#""submit_io_ring_empty_ns":{},"doorbell_share_of_submit":{}}}"#
        ),
        std::env::consts::ARCH,
        atomic,
        already,
        cycle,
        wait0,
        park.map_or("null".to_string(), |n| format!("{n:.1}")),
        observation
            .submit_nanos
            .map_or("null".to_string(), |n| format!("{n:.1}")),
        observation
            .doorbell_share_of_submit()
            .map_or("null".to_string(), |s| format!("{s:.4}")),
    );
}
