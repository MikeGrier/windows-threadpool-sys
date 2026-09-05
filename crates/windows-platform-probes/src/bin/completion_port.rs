// Copyright (c) Mike Grier.

//! Prints whether associating a handle with a completion port forecloses
//! `IoRing` use of it.
//!
//! **An experiment, not a component.** These probes measure platform behaviour
//! and are not for production use: that scope is what lets one do things a
//! shipping component must not. Do not call them from production code, and do
//! not lift a technique out of here. See this crate's DESIGN-NOTES.md.
//!
//! Every read is judged on its result code, its byte count, **and** the bytes
//! it actually landed. The first version of this probe checked only where the
//! completion arrived, and so read a clean failure -- result code
//! `ERROR_INVALID_PARAMETER`, zero bytes -- as a success.

use std::fmt::Write as _;
use windows_platform_probes::completion_port::{CompletionPortFinding, ReadAttempt, measure};
use windows_platform_probes::ioring::IoRingSupport;
use windows_platform_probes::report::{Stdout, emit};

fn describe(out: &mut String, label: &str, attempt: ReadAttempt) {
    let _ = writeln!(
        out,
        "  {label:<46} result={:#010x} bytes={} first={:#04x}  [{}]",
        attempt.result_code,
        attempt.bytes,
        attempt.first_byte,
        if attempt.succeeded() { "PASS" } else { "FAIL" }
    );
}

fn report(out: &mut String, finding: CompletionPortFinding) {
    let _ = writeln!(
        out,
        "a PASS needs all three: success code, full byte count, and the fill byte.\n"
    );

    describe(
        out,
        "case 1 CONTROL: no association at all",
        finding.control_unassociated,
    );
    describe(
        out,
        "case 2 TEST: associated, then IoRing read",
        finding.after_iocp_association,
    );
    describe(
        out,
        "case 3a: IoRing read BEFORE association",
        finding.before_late_association,
    );
    describe(
        out,
        "case 3b: IoRing read AFTER association",
        finding.after_late_association,
    );
    let _ = writeln!(
        out,
        "  {:<46} [{}]",
        "case 4 CONTROL: overlapped read via the port",
        if finding.port_still_works {
            "PASS"
        } else {
            "FAIL"
        }
    );
    describe(
        out,
        "case 5a: IoRing BEFORE CreateThreadpoolIo",
        finding.before_threadpool_io,
    );
    describe(
        out,
        "case 5b: IoRing AFTER CreateThreadpoolIo",
        finding.after_threadpool_io,
    );

    let _ = writeln!(out, "\n--- verdict ---");
    if !finding.is_valid() {
        let _ = writeln!(
            out,
            "  INVALID: a negative control failed, so nothing can be concluded."
        );
        let _ = writeln!(
            out,
            "  The probe is broken rather than the platform answering."
        );
        return;
    }

    if finding.association_forecloses_ioring() {
        let _ = writeln!(
            out,
            "  IOCP association FORECLOSES IoRing use of the same handle."
        );
        if finding.port_still_works {
            let _ = writeln!(
                out,
                "  The handle is still healthy -- it completes through the port -- so it"
            );
            let _ = writeln!(
                out,
                "  is the IoRing path specifically that is refused, not the handle."
            );
        } else {
            let _ = writeln!(
                out,
                "  NOTE: the port control also failed, so the handle may be broken"
            );
            let _ = writeln!(
                out,
                "  outright rather than only the IoRing path being refused."
            );
        }
    } else {
        let _ = writeln!(
            out,
            "  COEXIST: association does NOT prevent IoRing use of the handle."
        );
        let _ = writeln!(
            out,
            "  This contradicts the reading windows-namespace-request-sys rests on;"
        );
        let _ = writeln!(out, "  the handle-destination fork would need revisiting.");
    }

    if finding.threadpool_io_forecloses_ioring() {
        let _ = writeln!(
            out,
            "\n  CreateThreadpoolIo forecloses it the SAME way. That is the path this"
        );
        let _ = writeln!(
            out,
            "  workspace actually uses, so the consequence lands on"
        );
        let _ = writeln!(out, "  windows-threadpool-sys's own users.");
    } else {
        let _ = writeln!(
            out,
            "\n  CreateThreadpoolIo does NOT foreclose it, which differs from raw IOCP."
        );
        let _ = writeln!(
            out,
            "  Worth knowing: the two are not interchangeable for this purpose."
        );
    }
}

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
    let _ = writeln!(out, "== IOCP association vs IoRing, on one handle ==\n");

    match measure() {
        IoRingSupport::Unavailable => {
            let _ = writeln!(
                out,
                "this host has no usable IoRing, so nothing was measured."
            );
            let _ = writeln!(out, "('cannot ask' is not 'the answer is no'.)");
        }
        IoRingSupport::Measured(finding) => report(&mut out, finding),
    }
    out
}
