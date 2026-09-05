// Copyright (c) Mike Grier.

//! Prints the `IoRing` registration and thread-agnosticism findings.
//!
//! **An experiment, not a component.** These probes measure platform behaviour
//! and are not for production use: that scope is what lets one do things a
//! shipping component must not. Do not call them from production code, and do
//! not lift a technique out of here. See this crate's DESIGN-NOTES.md.
//!
//! Both are environment-dependent: `IoRing` needs a recent Windows build, so
//! this reports "cannot measure" rather than a false negative on a host that
//! has no ring.

use std::fmt::Write as _;
use windows_platform_probes::ioring::{
    IoRingSupport, is_available, measure_registration, measure_thread_agnosticism,
};
use windows_platform_probes::report::{Stdout, emit};

fn main() {
    // The only place that names the real stream. Everything below composes
    // text; nothing below knows where it goes.
    emit(&mut Stdout, &render());
}

/// The probe's whole report, as text.
fn render() -> String {
    let mut out = String::new();
    // First line of the report, and part of the returned text rather than
    // written out here: a captured report must carry the line naming the
    // machine that produced it, and the taint marker with it. Without it a
    // finding can be pasted anywhere and compared against anything.
    let _ = writeln!(
        out,
        "{}",
        windows_placement_probe::fingerprint::banner_line()
    );
    let _ = writeln!(out, "== IoRing registration and thread agnosticism ==\n");

    if !is_available() {
        let _ = writeln!(
            out,
            "this host has no usable IoRing, so nothing was measured."
        );
        let _ = writeln!(
            out,
            "(Reported rather than measured: 'we could not ask' and 'the answer"
        );
        let _ = writeln!(
            out,
            "is no' are different facts, and conflating them is how a design note"
        );
        let _ = writeln!(out, "ends up citing a measurement that never ran.)");
        return out;
    }

    let _ = writeln!(out, "registration semantics:");
    match measure_registration() {
        IoRingSupport::Unavailable => {
            let _ = writeln!(out, "  (no ring)");
        }
        IoRingSupport::Measured(observed) => {
            let _ = writeln!(
                out,
                "  after re-registering ONE handle: index 0 usable {}, index 1 usable {}",
                observed.index_zero_usable_after_second, observed.index_one_usable_after_second
            );
            if observed.replaces() {
                let _ = writeln!(out, "  -> REPLACES the whole table, which is what");
                let _ = writeln!(
                    out,
                    "     windows-ioring-sys assumes and refuses a second call on."
                );
            } else if observed.appends() {
                let _ = writeln!(
                    out,
                    "  -> APPENDS. windows-ioring-sys's index bookkeeping would be"
                );
                let _ = writeln!(out, "     WRONG and its refusal a needless restriction.");
            } else {
                let _ = writeln!(
                    out,
                    "  -> neither: even index 0 stopped working, so the probe"
                );
                let _ = writeln!(out, "     broke rather than the platform answering.");
            }
        }
    }

    let _ = writeln!(out, "\nthread agnosticism:");
    match measure_thread_agnosticism() {
        IoRingSupport::Unavailable => {
            let _ = writeln!(out, "  (no ring)");
        }
        IoRingSupport::Measured(observed) => {
            let _ = writeln!(
                out,
                "  pending at submitter exit: {} | result code: {:#010x} | \
                 transferred the fill byte: {}",
                observed.pending_at_submitter_exit, observed.result_code, observed.filled
            );
            if !observed.pending_at_submitter_exit {
                let _ = writeln!(
                    out,
                    "  -> the read had ALREADY completed, so this run measured"
                );
                let _ = writeln!(out, "     nothing about thread affinity either way.");
            }
            if observed.survives_submitter_exit() {
                let _ = writeln!(
                    out,
                    "  -> an operation OUTLIVES the thread that submitted it, so a"
                );
                let _ = writeln!(
                    out,
                    "     design whose threads are transient by construction is safe."
                );
            } else {
                let _ = writeln!(
                    out,
                    "  -> the operation did NOT survive its submitter. Every thread"
                );
                let _ = writeln!(
                    out,
                    "     in the proposed design is transient, so this would fail"
                );
                let _ = writeln!(
                    out,
                    "     only under load -- the worst place to discover it."
                );
            }
        }
    }
    out
}
