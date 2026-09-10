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

use windows_platform_probes::report::emit_report;
use windows_platform_probes::request_cost::measure;

/// Measured by `probe-doorbell-cost` on a **Snapdragon X2 (ARM64)** machine,
/// and recorded in [the 2026-08-30 design session]. Restated here only to
/// render a ratio; the authoritative number is whatever that probe prints on
/// the host this runs on.
///
/// **The platform is named because a nanosecond figure without one is not a
/// measurement, it is an anecdote.** These constants said "the development
/// machine", which is a label only its author can resolve, and everything
/// derived from them was then read as though it described machines in general.
/// It does not: an x86_64 host measured during review put the doorbell cycle at
/// ~531 ns against ~208 ns for a redundant `SetEvent`, where the ARM64 figures
/// here are 164.9 and 7.2. Two observations, two architectures, and the
/// conclusions drawn from them differ in sign -- which is the whole argument
/// against generalising from either.
///
/// **The build profile behind these is not recorded**, which is why the report
/// below calls the ratios indicative rather than quoting them as results. They
/// are not re-baselined here: the figure is the one that session recorded, and
/// silently replacing it would leave the session describing a number that no
/// longer exists anywhere. CI runs both this probe and `probe-doorbell-cost`
/// under `--release` in the same job, so the like-for-like comparison a reader
/// actually wants is those two outputs, not this constant.
///
/// [the 2026-08-30 design session]: ../../../../design-sessions/DESIGN-SESSION-2026-08-30-numa-sharded-io-execution-domains.md
const DOORBELL_NS_REFERENCE: f64 = 164.9;
const ATOMIC_NS_REFERENCE: f64 = 7.2;

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
    // machine that produced it, and the taint marker with it. Without it a
    // timing number can be pasted anywhere and compared against anything.
    let _ = writeln!(
        out,
        "{}",
        windows_placement_probe::fingerprint::banner_line()
    );
    let _ = writeln!(out, "== what does a namespace request cost to build? ==\n");

    let observation = measure();

    let _ = writeln!(
        out,
        "{:<26} {:>10} {:>14} {:>16}",
        "operation", "ns/op", "x an atomic", "x a doorbell"
    );
    // Says what the first four rows include, because their names do not. Each
    // is a construct-and-DESTROY cycle: `time_loop` drops the value it is
    // handed at the end of the timed statement, so the free is inside the
    // figure. `capture_handle` and `close_handle` are the exception and are
    // timed apart, because dropping a `CapturedHandle` calls `CloseHandle` and
    // that is a second kernel transition rather than a free.
    let _ = writeln!(
        out,
        "  (the first four are construct-and-drop cycles; capture and close are\n   \
         timed separately)"
    );
    for timing in &observation.timings {
        let _ = writeln!(
            out,
            "{:<26} {:>10.1} {:>14.1} {:>16.2}",
            timing.label,
            timing.nanos_per_op,
            timing.nanos_per_op / ATOMIC_NS_REFERENCE,
            timing.nanos_per_op / DOORBELL_NS_REFERENCE,
        );
    }
    let _ = writeln!(
        out,
        "\n(ratios use the reference doorbell {DOORBELL_NS_REFERENCE:.1} ns and atomic \
         {ATOMIC_NS_REFERENCE:.1} ns measured\n by probe-doorbell-cost on the development \
         machine; re-read that probe on this host\n before trusting them)"
    );

    let _ = writeln!(out, "\ninterpretation:");

    let build = observation.get("build_open_request");
    let capture = observation.get("capture_handle");

    if let Some(build) = build {
        // The ratio names its own reference, because the two numbers do not
        // come from the same machine. `build` was measured on this host just
        // now; the doorbell figure is a constant captured once on the
        // Snapdragon X2 (ARM64) development machine. This probe now runs on
        // hosted CI runners, which are a heterogeneous fleet, so a ratio printed
        // as though both halves were local can be wrong even when the
        // measurement is sound.
        let _ = writeln!(
            out,
            "  building a pathed request costs {build:.0} ns, which is {:.1}x one",
            build / DOORBELL_NS_REFERENCE
        );
        let _ = writeln!(
            out,
            "  doorbell AS MEASURED ON THE ARM64 DEVELOPMENT MACHINE ({DOORBELL_NS_REFERENCE:.1} ns),"
        );
        let _ = writeln!(
            out,
            "  not on this one. Run probe-doorbell-cost here to make the ratio local."
        );
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "  SCOPE, because this is easy to over-read: that is a statement about"
        );
        let _ = writeln!(
            out,
            "  ONE OPERATION TYPE, not about the queue. A namespace open is the"
        );
        let _ = writeln!(
            out,
            "  heaviest payload the queue carries -- it resolves a path through"
        );
        let _ = writeln!(
            out,
            "  Win32 and may duplicate a handle -- and it ends in a CreateFileW"
        );
        let _ = writeln!(
            out,
            "  costing microseconds regardless. A registered-buffer read, which is"
        );
        let _ = writeln!(
            out,
            "  the hot path, carries no path and no handle: its descriptor is a slot"
        );
        let _ = writeln!(
            out,
            "  index and an offset, and there the queue's own mechanics are the"
        );
        let _ = writeln!(out, "  whole cost.");
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "  Nor is per-operation overhead the same thing as queue efficiency."
        );
        let _ = writeln!(
            out,
            "  Throughput under contention, cache behaviour, batching amortization"
        );
        let _ = writeln!(
            out,
            "  and backpressure decide that, and a single uncontended construction"
        );
        let _ = writeln!(out, "  time measures none of them.");
        let _ = writeln!(out);
        // States the COMPARISON and refuses the verdict, because this probe
        // does not measure a doorbell.
        //
        // This read "What it does support: for an open-heavy workload, doorbell
        // tuning would be optimizing the small half" -- a design conclusion
        // whose truth depends entirely on which of the two is larger, decided
        // against a constant measured on the Snapdragon X2 (ARM64) development
        // machine. It INVERTS on the x86_64 host measured during review, where
        // `probe-doorbell-cost` reported a ~531 ns cycle against ~210 ns to
        // build a request, so the doorbell is the large half there and tuning
        // it would optimize the large one. Both probes run in the same
        // CI job, so the sentence was contradicted a few lines further down the
        // same log.
        //
        // The caveats above cover the printed ratio and the operation-type
        // scope; neither guarded this, because it was phrased as what the run
        // supports rather than as a ratio.
        let _ = writeln!(
            out,
            "  WHICH HALF IS LARGER IS NOT ESTABLISHED HERE. Whether doorbell"
        );
        let _ = writeln!(
            out,
            "  tuning would optimize the large or the small half depends on the"
        );
        let _ = writeln!(
            out,
            "  doorbell measured ON THIS HOST, which this probe does not measure:"
        );
        let _ = writeln!(
            out,
            "  read probe-doorbell-cost's set_reset_event from the same run and"
        );
        let _ = writeln!(
            out,
            "  compare it against the {build:.0} ns above. The comparison can invert"
        );
        let _ = writeln!(
            out,
            "  between hosts, so a conclusion about OPERATION MIX belongs to a"
        );
        let _ = writeln!(out, "  reader holding both figures from one machine.");
    }

    if let Some(capture) = capture {
        let _ = writeln!(
            out,
            "\n  duplicating a handle costs {capture:.0} ns -- a kernel transition, not"
        );
        let _ = writeln!(
            out,
            "  a memory copy, and easy to under-count when thinking about what an"
        );
        let _ = writeln!(out, "  SQE holds.");
        // Reported beside it because the duplication figure above excludes it by
        // construction, and a reader sizing a request's real cost needs both:
        // every captured handle is eventually closed, so the lifecycle is the
        // pair. Keeping them separate is what stops either being quoted as the
        // other.
        if let Some(close) = observation.get("close_handle") {
            let _ = writeln!(
                out,
                "  Closing one costs a further {close:.0} ns, measured separately, so a"
            );
            let _ = writeln!(
                out,
                "  captured handle's whole lifecycle is {:.0} ns. The duplication figure",
                capture + close
            );
            let _ = writeln!(out, "  above is the duplication alone.");
        }
        if let Some(build) = build {
            if capture > build {
                let _ = writeln!(
                    out,
                    "  It is {:.1}x the cost of building the pathed request itself, so a",
                    capture / build
                );
                let _ = writeln!(
                    out,
                    "  request carrying a handle is dominated by the duplication, and"
                );
                let _ = writeln!(
                    out,
                    "  any allocation tuning on the path would be optimizing the wrong"
                );
                let _ = writeln!(out, "  half.");
            } else {
                // The ratio without a verdict. This branch fires on `capture <=
                // build`, which is every ratio from 0.99 down to 0.01, and it
                // said "the two are comparable and neither dominates" for all
                // of them -- a claim about closeness drawn from a test for
                // order. Whether two figures are comparable needs a range, and
                // this probe was given one only by accident of which arm it
                // landed in.
                let _ = writeln!(
                    out,
                    "  It is {:.2}x the pathed request. Which of the two dominates, if",
                    capture / build
                );
                let _ = writeln!(
                    out,
                    "  either does, is for a reader with a threshold in mind; this run"
                );
                let _ = writeln!(out, "  establishes only the two costs and their ratio.");
            }
        }
    }

    // The split that decides whether an allocator change can help at all.
    if let Some(build) = build
        && let Some(clone) = observation.get("clone_prepared_units")
        && build > clone
    {
        let _ = writeln!(
            out,
            "\n  WHERE THE TIME ACTUALLY GOES, and it is not only the allocator:"
        );
        // "resolves against process state" -- not "a syscall", and not
        // "lexical" either.
        //
        // The first was wrong because a timing loop cannot establish a kernel
        // transition. The second, which replaced it, is wrong for a symmetric
        // reason: `GetFullPathNameW` consults the process current directory,
        // and for a drive-relative path the per-drive current directory held in
        // the `=C:` environment variables, so it is not pure string work. A
        // genuinely lexical canonicalizer is a different call
        // (`PathCchCanonicalizeEx`), and it is deliberately NOT the one
        // `prepare` wants -- resolving against the CWD at submission is the
        // property the namespace design is buying.
        //
        // What this run established is the cost. The mechanism it did not, and
        // two successive attempts to name one were each wrong in the same way.
        let _ = writeln!(
            out,
            "  `prepare` calls GetFullPathNameW, which roots MOST paths that are not fully"
        );
        let _ = writeln!(
            out,
            "  qualified against process state -- most, because a legacy device name such"
        );
        let _ = writeln!(
            out,
            "  as CON short-circuits rooting entirely. The CWD is mutable by any thread, so"
        );
        let _ = writeln!(
            out,
            "  resolving later would be racy. BOTH SAMPLES HERE ARE FULLY QUALIFIED, so"
        );
        let _ = writeln!(
            out,
            "  that rooting is the motivation for resolving at submission and is not"
        );
        let _ = writeln!(out, "  what these numbers measure.");
        let _ = writeln!(
            out,
            "  The gap between building and cloning bounds the resolution step from"
        );
        let _ = writeln!(
            out,
            "  above. It is not the call's own cost: it also spans ONE NET allocation"
        );
        let _ = writeln!(
            out,
            "  of this crate's own -- prepare allocates twice against the clone's once,"
        );
        let _ = writeln!(
            out,
            "  so the subtraction cancels one -- and the builder chain."
        );
        let _ = writeln!(
            out,
            "  Whether any of it enters the kernel is not something this run measured."
        );
        let _ = writeln!(
            out,
            "  Two different schemes recover different things, and this said"
        );
        let _ = writeln!(
            out,
            "  one number for both. RECYCLING a resolved path pays {clone:.0} ns instead"
        );
        let _ = writeln!(
            out,
            "  of {build:.0} ns, so it recovers {:.0} ns -- but that saving is the",
            build - clone
        );
        let _ = writeln!(
            out,
            "  RESOLUTION STEP plus that net allocation, and only a caller that can"
        );
        let _ = writeln!(
            out,
            "  reuse a resolved path gets it. INLINE STORAGE removes the allocation and"
        );
        let _ = writeln!(
            out,
            "  copy instead, which is what the {clone:.0} ns clone measures, so it"
        );
        let _ = writeln!(
            out,
            "  recovers at most that and cannot touch the resolution at all."
        );
        let _ = writeln!(
            out,
            "  A caller with a fresh path each time pays the resolution regardless."
        );
    }

    // `expect`, not `null`. Every label below is recorded unconditionally by
    // `measure`, so a lookup that misses means a label was renamed on one side
    // and not the other -- a defect in this probe, not a condition of the host.
    //
    // `null` is the right answer for a value that can legitimately be absent,
    // and none of these can be. Letting them say "absent" would have produced a
    // partially populated record that parses cleanly and reads, to a mining
    // pass, as a host on which the measurement did not apply.
    let get = |label: &str| {
        let ns = observation
            .get(label)
            .unwrap_or_else(|| panic!("measure always records {label}"));
        format!("{ns:.1}")
    };
    let _ = writeln!(
        out,
        concat!(
            r#"{{"reason":"x-probe-request-cost","arch":"{}","prepare_short_cycle_ns":{},"#,
            r#""prepare_long_cycle_ns":{},"build_open_request_cycle_ns":{},"#,
            r#""clone_prepared_units_cycle_ns":{},"capture_handle_ns":{},"#,
            r#""close_handle_ns":{}}}"#
        ),
        std::env::consts::ARCH,
        get("prepare_short_path"),
        get("prepare_long_path"),
        get("build_open_request"),
        get("clone_prepared_units"),
        get("capture_handle"),
        get("close_handle"),
    );
}
