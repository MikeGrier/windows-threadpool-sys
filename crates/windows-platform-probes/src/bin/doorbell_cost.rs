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

use std::fmt::Write as _;
use windows_platform_probes::doorbell_cost::{Observation, measure, measure_park_and_wake};

use windows_platform_probes::report::{Stdout, emit};

fn main() {
    // The only place that names the real stream. Everything above composes
    // text; nothing above knows where it goes.
    emit(
        &mut Stdout,
        &render(&measure(), measure_park_and_wake(20_000)),
    );
}

/// The probe's whole report, as text.
fn render(observation: &Observation, park: Option<f64>) -> String {
    let mut out = String::new();
    // First line of the report, and part of the returned text rather than
    // written out here: a captured report must carry the line naming the
    // machine that produced it, and the taint marker with it. Without it a
    // timing number can be pasted anywhere and compared against anything.
    let _ = writeln!(
        out,
        "{}",
        windows_placement_probe::fingerprint::banner_line()
    );
    let _ = writeln!(
        out,
        "== what does a doorbell cost, against the syscall it guards? ==\n"
    );

    let _ = writeln!(out, "{:<30} {:>12}", "operation", "ns/op");
    for timing in &observation.timings {
        let _ = writeln!(out, "{:<30} {:>12.1}", timing.label, timing.nanos_per_op);
    }

    match park {
        Some(ns) => {
            let _ = writeln!(out, "{:<30} {:>12.1}", "park_and_wake round trip", ns);
        }
        None => {
            let _ = writeln!(
                out,
                "{:<30} {:>12}",
                "park_and_wake round trip", "TIMED OUT"
            );
        }
    }

    let _ = writeln!(out, "\ninterpretation:");

    if let Some(atomic) = observation.get("atomic_fetch_add")
        && let Some(doorbell) = observation.get("set_reset_event")
        && atomic > 0.0
    {
        let _ = writeln!(
            out,
            "  a doorbell cycle costs {:.0}x an uncontended atomic ({:.0} ns vs {:.1} ns).",
            doorbell / atomic,
            doorbell,
            atomic
        );
        if let Some(park) = park {
            let _ = writeln!(
                out,
                "  an actual park-and-wake round trip costs {:.0}x that again ({:.0} ns),",
                park / doorbell,
                park
            );
            let _ = writeln!(
                out,
                "  which is what is paid when the consumer genuinely sleeps."
            );
        }
    }

    // Deliberately NOT expressed as a share of the empty submit. See below.
    if let Some(submit) = observation.submit_nanos {
        let _ = writeln!(
            out,
            "\n  CAUTION: an empty SubmitIoRing measured {submit:.0} ns, and that is"
        );
        let _ = writeln!(
            out,
            "  NOT a fair denominator: it carries no work, so any 'doorbell is N%"
        );
        let _ = writeln!(
            out,
            "  of a syscall' figure derived from it would be a confident wrong"
        );
        let _ = writeln!(
            out,
            "  answer. The honest denominator is the cost of the real work a"
        );
        let _ = writeln!(
            out,
            "  submission carries, which this probe does not measure."
        );

        // Whether it short-circuits is DECIDED HERE, not asserted. This text
        // used to state, unconditionally, that the empty submit was "far too
        // cheap for a kernel transition -- almost certainly short-circuiting in
        // user mode". That was written around a 79 ns reading on the
        // development machine and is contradicted by any host where the empty
        // submit lands among this probe's own syscalls: measured here at
        // 216 ns against 205 ns for an already-signalled `SetEvent` and 280 ns
        // for a satisfied wait, the claim is not merely unsupported, it is
        // false. The advice above holds either way, which is why it is
        // unconditional and this is not.
        let syscalls: Vec<f64> = ["set_event_already_signalled", "wait_zero_signalled"]
            .into_iter()
            .filter_map(|label| observation.get(label))
            .collect();
        if let Some(cheapest) = syscalls.iter().copied().reduce(f64::min) {
            if submit < cheapest / 2.0 {
                let _ = writeln!(
                    out,
                    "  It is also under half this probe's cheapest measured syscall"
                );
                let _ = writeln!(
                    out,
                    "  ({cheapest:.0} ns), so on this host it is very likely short-circuiting"
                );
                let _ = writeln!(out, "  in user mode when there is nothing queued.");
            } else {
                let _ = writeln!(
                    out,
                    "  On this host it is the same order as this probe's own syscalls"
                );
                let _ = writeln!(
                    out,
                    "  ({cheapest:.0} ns and up), so nothing here says it short-circuits --"
                );
                let _ = writeln!(out, "  it is simply an empty one.");
            }
        }
    }
    // What can be said without a denominator: how much batching it takes for
    // the doorbell to disappear, which is the lever the design actually has.
    if let Some(doorbell) = observation.get("set_reset_event")
        && let Some(atomic) = observation.get("atomic_fetch_add")
        && atomic > 0.0
    {
        let _ = writeln!(
            out,
            "\n  batching is the lever, and it is a strong one. One doorbell per"
        );
        let _ = writeln!(out, "  drained batch costs, per operation:");
        for batch in [1_u32, 8, 32, 128] {
            let _ = writeln!(
                out,
                "    batch of {batch:>4}: {:>7.1} ns/op ({:.1}x an atomic)",
                doorbell / f64::from(batch),
                doorbell / f64::from(batch) / atomic
            );
        }
        let break_even = (doorbell / atomic).ceil() as u32;
        let _ = writeln!(
            out,
            "  so at a batch of about {break_even}, the doorbell costs less per"
        );
        let _ = writeln!(out, "  operation than the atomic push it accompanies.");
    }

    let _ = writeln!(
        out,
        "\n  => The skip-when-busy rule is a refinement, not a prerequisite."
    );
    let _ = writeln!(
        out,
        "     Batching alone drives the doorbell below the cost of the push,"
    );
    let _ = writeln!(
        out,
        "     so a first implementation can always-signal and stay honest."
    );
    let _ = writeln!(
        out,
        "     Adopt the eventcount when a measurement against real work"
    );
    let _ = writeln!(out, "     justifies its lost-wakeup risk -- not before.");

    let atomic = observation.get("atomic_fetch_add").unwrap_or(f64::NAN);
    let already = observation
        .get("set_event_already_signalled")
        .unwrap_or(f64::NAN);
    let cycle = observation.get("set_reset_event").unwrap_or(f64::NAN);
    let wait0 = observation.get("wait_zero_signalled").unwrap_or(f64::NAN);
    // `doorbell_over_empty_submit`, not `doorbell_share_of_submit`. The prose
    // above tells a human that an empty submit is not a fair denominator and
    // that any figure derived from it is a confident wrong answer -- and this
    // line then handed a machine exactly that figure under a name meaning "the
    // doorbell's share of a submit", with no caveat a mining pass could read.
    // The name now states its own denominator, so a query that wants the ratio
    // asks for it knowingly, and one that wants a real share of a syscall does
    // not find this field by looking for that.
    let _ = writeln!(
        out,
        concat!(
            r#"{{"reason":"x-probe-doorbell-cost","arch":"{}","atomic_ns":{:.1},"#,
            r#""set_event_already_signalled_ns":{:.1},"set_reset_event_ns":{:.1},"#,
            r#""wait_zero_signalled_ns":{:.1},"park_and_wake_round_trip_ns":{},"#,
            r#""submit_io_ring_empty_ns":{},"doorbell_over_empty_submit":{}}}"#
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
    out
}
