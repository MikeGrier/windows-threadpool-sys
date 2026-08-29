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

use windows_platform_probes::completion_port::{CompletionPortFinding, ReadAttempt, measure};
use windows_platform_probes::ioring::IoRingSupport;

fn describe(label: &str, attempt: ReadAttempt) {
    println!(
        "  {label:<46} result={:#010x} bytes={} first={:#04x}  [{}]",
        attempt.result_code,
        attempt.bytes,
        attempt.first_byte,
        if attempt.succeeded() { "PASS" } else { "FAIL" }
    );
}

fn report(finding: CompletionPortFinding) {
    println!("a PASS needs all three: success code, full byte count, and the fill byte.\n");

    describe(
        "case 1 CONTROL: no association at all",
        finding.control_unassociated,
    );
    describe(
        "case 2 TEST: associated, then IoRing read",
        finding.after_iocp_association,
    );
    describe(
        "case 3a: IoRing read BEFORE association",
        finding.before_late_association,
    );
    describe(
        "case 3b: IoRing read AFTER association",
        finding.after_late_association,
    );
    println!(
        "  {:<46} [{}]",
        "case 4 CONTROL: overlapped read via the port",
        if finding.port_still_works {
            "PASS"
        } else {
            "FAIL"
        }
    );
    describe(
        "case 5a: IoRing BEFORE CreateThreadpoolIo",
        finding.before_threadpool_io,
    );
    describe(
        "case 5b: IoRing AFTER CreateThreadpoolIo",
        finding.after_threadpool_io,
    );

    println!("\n--- verdict ---");
    if !finding.is_valid() {
        println!("  INVALID: a negative control failed, so nothing can be concluded.");
        println!("  The probe is broken rather than the platform answering.");
        return;
    }

    if finding.association_forecloses_ioring() {
        println!("  IOCP association FORECLOSES IoRing use of the same handle.");
        if finding.port_still_works {
            println!("  The handle is still healthy -- it completes through the port -- so it");
            println!("  is the IoRing path specifically that is refused, not the handle.");
        } else {
            println!("  NOTE: the port control also failed, so the handle may be broken");
            println!("  outright rather than only the IoRing path being refused.");
        }
    } else {
        println!("  COEXIST: association does NOT prevent IoRing use of the handle.");
        println!("  This contradicts the reading windows-namespace-request-sys rests on;");
        println!("  the handle-destination fork would need revisiting.");
    }

    if finding.threadpool_io_forecloses_ioring() {
        println!("\n  CreateThreadpoolIo forecloses it the SAME way. That is the path this");
        println!("  workspace actually uses, so the consequence lands on");
        println!("  windows-threadpool-sys's own users.");
    } else {
        println!("\n  CreateThreadpoolIo does NOT foreclose it, which differs from raw IOCP.");
        println!("  Worth knowing: the two are not interchangeable for this purpose.");
    }
}

fn main() {
    println!("== IOCP association vs IoRing, on one handle ==\n");

    match measure() {
        IoRingSupport::Unavailable => {
            println!("this host has no usable IoRing, so nothing was measured.");
            println!("('cannot ask' is not 'the answer is no'.)");
        }
        IoRingSupport::Measured(finding) => report(finding),
    }
}
