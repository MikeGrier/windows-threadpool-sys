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

use std::fmt::Write as _;

use windows_platform_probes::report::emit_report;
use windows_platform_probes::request_cost::{EVERY_LABEL, json_key, measure};

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
    // Says what the rows include, because their names do not. Each is a
    // construct-and-DESTROY cycle: `time_loop` drops the value it is handed at
    // the end of the timed statement, so the free is inside the figure, and for
    // the two `perform` rows the drop is the `CloseHandle` of the handle the
    // open returned. `duplicate_borrowed_handle` and `close_duplicate` are the
    // exception and are timed apart, because dropping the `OwnedHandle` a
    // duplication returns calls `CloseHandle` and that is a second kernel
    // transition rather than a free.
    let _ = writeln!(
        out,
        "  (every row but the last two is a construct-and-drop cycle, and the perform\n   \
         rows are an open-and-close cycle; a duplicate and its close are timed separately)"
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

    // Every label read here is recorded unconditionally by `measure`, and the
    // census at the end of this function asserts it, so a lookup that misses
    // means a label was renamed on one side and not the other.
    let string_clone = observation.get("clone_path_string");
    let build = observation.get("build_request_kept");
    let grafted = observation.get("build_request_grafted");
    let clone_kept = observation.get("clone_request_kept");
    let clone_grafted = observation.get("clone_request_grafted");
    let perform = observation.get("perform_open");
    let build_and_perform = observation.get("build_and_perform_open");
    let share = observation.get("share_handle");
    let with_handle = observation.get("build_request_with_handle");
    let duplicate = observation.get("duplicate_borrowed_handle");

    if let Some(build) = build {
        // The ratio names its own reference, because the two numbers do not
        // come from the same machine. `build` was measured on this host just
        // now; the doorbell figure is a constant captured once on the
        // Snapdragon X2 (ARM64) development machine. This probe runs on hosted
        // CI runners, which are a heterogeneous fleet, so a ratio printed as
        // though both halves were local can be wrong even when the measurement
        // is sound.
        let _ = writeln!(
            out,
            "  building a request for an absolute path costs {build:.0} ns, which is {:.1}x one",
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
            "  heaviest payload the queue carries -- it carries a path and may"
        );
        let _ = writeln!(
            out,
            "  carry a handle -- and it ends in a CreateFileW costing"
        );
        let _ = writeln!(
            out,
            "  microseconds regardless. A registered-buffer read, which is"
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
        // does not measure a doorbell. Which half is larger inverts between
        // hosts: the doorbell is the small half on the Snapdragon X2 (ARM64)
        // development machine and the large one on the x86_64 host measured
        // during review, so a sentence naming either would be contradicted a
        // few lines further down the same CI log.
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

    // What the graft adds, as figures and the mechanism, and no verdict on
    // whether it matters. The two builds are the same request but for the path:
    // an absolute path is kept as given and no directory is read, and a relative
    // one is grafted onto the current directory the build reads.
    if let Some(build) = build
        && let Some(grafted) = grafted
    {
        let _ = writeln!(
            out,
            "\n  A path Win32 would resolve against the current directory is fixed to it"
        );
        let _ = writeln!(
            out,
            "  when the request is built. Building one costs {grafted:.0} ns, against {build:.0} ns for"
        );
        let _ = writeln!(
            out,
            "  an absolute path, which is kept as given. The difference spans the"
        );
        let _ = writeln!(
            out,
            "  GetCurrentDirectoryW call, the concatenation, and the allocations they"
        );
        let _ = writeln!(
            out,
            "  need; this run does not separate them, and does not establish whether"
        );
        let _ = writeln!(out, "  the call enters the kernel.");
        if let Some(string_clone) = string_clone {
            let _ = writeln!(
                out,
                "  Both builds include {string_clone:.0} ns to clone the sample string that hands the"
            );
            let _ = writeln!(out, "  request an owned path.");
        }
    }

    // Recycling a request, as the two costs side by side. A clone keeps the
    // directory its original captured, so it pays neither the read nor the
    // concatenation again.
    if let Some(build) = build
        && let Some(grafted) = grafted
        && let Some(clone_kept) = clone_kept
        && let Some(clone_grafted) = clone_grafted
    {
        let _ = writeln!(
            out,
            "\n  Cloning a built request instead costs {clone_kept:.0} ns for an absolute path"
        );
        let _ = writeln!(
            out,
            "  and {clone_grafted:.0} ns for a relative one, against {build:.0} and {grafted:.0} ns to build them."
        );
        let _ = writeln!(
            out,
            "  A clone keeps the directory its original captured. A request that is"
        );
        let _ = writeln!(
            out,
            "  grafted holds two strings, the path as given and the path handed to"
        );
        let _ = writeln!(out, "  Win32, where one that is kept as given holds one.");
    }

    // The call the request carries, so the build can be read beside it.
    if let Some(perform) = perform {
        let _ = writeln!(
            out,
            "\n  Performing the open of a real file (kernel32.dll) and closing the handle it"
        );
        let _ = writeln!(out, "  returns costs {perform:.0} ns.");
        if let Some(both) = build_and_perform {
            let _ = writeln!(
                out,
                "  Building the request and performing it costs {both:.0} ns together."
            );
        }
    }

    // A handle in a request, as figures and the mechanism. A request holds a
    // `SharedHandle`, so giving it one is a clone of a handle the caller already
    // shares; nothing is duplicated.
    if let Some(share) = share {
        let _ = writeln!(
            out,
            "\n  A request holds a handle it carries shared: giving it one is a clone of a"
        );
        let _ = writeln!(
            out,
            "  handle the caller already shares, which counts one more holder. A clone and"
        );
        let _ = writeln!(out, "  its drop cost {share:.0} ns.");
        if let Some(build) = build
            && let Some(with_handle) = with_handle
        {
            let _ = writeln!(
                out,
                "  Building the absolute-path request with a template handle costs {with_handle:.0} ns,"
            );
            let _ = writeln!(out, "  against {build:.0} ns without one.");
        }
    }

    // Outside the request crate: what a caller holding only a borrowed handle
    // pays for one it can share. Labelled as that and not as a request's cost,
    // because a request no longer duplicates anything.
    if let Some(duplicate) = duplicate {
        let _ = writeln!(
            out,
            "\n  A caller holding only a BORROWED handle pays, outside the request crate,"
        );
        let _ = writeln!(
            out,
            "  for one it can share: duplicating it costs {duplicate:.0} ns, a kernel transition."
        );
        // Reported beside it because the duplication figure excludes it by
        // construction, and every duplicate is eventually closed, so the
        // lifecycle is the pair. Keeping them separate is what stops either
        // being quoted as the other.
        if let Some(close) = observation.get("close_duplicate") {
            let _ = writeln!(
                out,
                "  Closing the duplicate costs a further {close:.0} ns, measured separately."
            );
        }
        let _ = writeln!(out, "  Neither is part of building a request.");
    }

    // Built by walking the SAME `timings` the prose table walked, with
    // `json_key` deciding only what each entry is called here.
    //
    // This replaces hand-named fields. They were two independent restatements
    // of one measurement, so nothing stopped them disagreeing or one omitting a
    // figure the other showed.
    //
    // Iterating what was measured cannot ask for something absent, so this loop
    // cannot fail on a missing label; it just emits one key fewer, shrinking the
    // schema with nothing saying so. The census below is the guard, in one
    // place.
    let mut fields = String::new();
    for timing in &observation.timings {
        let _ = write!(
            fields,
            r#""{}":{:.1},"#,
            json_key(timing.label),
            timing.nanos_per_op
        );
    }

    for label in EVERY_LABEL {
        assert!(
            observation
                .timings
                .iter()
                .any(|timing| timing.label == label),
            "`{label}` is recorded on every host, so its absence means it was \
             renamed or dropped in one place and not the other; the NDJSON line \
             must not quietly ship one field fewer"
        );
    }
    // The trailing comma the loop leaves is trimmed rather than avoided with a
    // separator dance, because every field here is unconditional.
    let fields = fields.trim_end_matches(',');

    let _ = writeln!(
        out,
        r#"{{"reason":"x-probe-request-cost","arch":"{}",{fields}}}"#,
        std::env::consts::ARCH,
    );
}
