// Copyright (c) Mike Grier.

//! Prints whether `CancelSynchronousIo` is safe to point at a shared thread.
//!
//! **An experiment, not a component.** These probes measure platform behaviour
//! and are not for production use: that scope is what lets one do things a
//! shipping component must not. Do not call them from production code, and do
//! not lift a technique out of here. See this crate's DESIGN-NOTES.md.
//!
//! **Binary only, and deliberately not a test.** The subject of the measurement
//! is a call that can fail to return, so every case here runs behind a
//! watchdog; a wedged `#[test]` would take the whole suite with it.

use std::fmt::Write as _;
use windows_platform_probes::cancel_io::{
    CancelOutcome, WATCHDOG, cancel_against_busy_thread, cancel_against_idle_thread,
};
use windows_platform_probes::report::{Stdout, emit};

fn describe(outcome: CancelOutcome) -> String {
    match outcome {
        CancelOutcome::Cancelled => "returned: cancelled an operation".to_owned(),
        CancelOutcome::NotFound { code } => {
            format!("returned: found nothing (error {code})")
        }
        CancelOutcome::Wedged => format!("WEDGED: did not return within {WATCHDOG:?}"),
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
    let _ = writeln!(
        out,
        "== is CancelSynchronousIo safe to point at a shared thread? ==\n"
    );
    let _ = writeln!(out, "watchdog: {WATCHDOG:?} per attempt\n");

    let _ = writeln!(
        out,
        "case 1: cancel against a thread with no I/O outstanding"
    );
    let idle = cancel_against_idle_thread();
    let _ = writeln!(out, "  {}", describe(idle));
    let _ = writeln!(
        out,
        "  -> a point-in-time cancel finds nothing here; a standing request\n     against the thread would not."
    );

    let _ = writeln!(
        out,
        "\ncase 2: cancel against a thread hammering synchronous reads"
    );
    let busy = cancel_against_busy_thread(4);
    for (attempt, outcome) in busy.iter().enumerate() {
        let _ = writeln!(out, "  attempt {}: {}", attempt + 1, describe(*outcome));
    }

    let wedged = busy.iter().any(|outcome| !outcome.returned());

    let _ = writeln!(out, "\nconclusion:");
    if wedged {
        let _ = writeln!(
            out,
            "  CancelSynchronousIo DID NOT RETURN against a thread that keeps"
        );
        let _ = writeln!(
            out,
            "  re-entering synchronous I/O. In the proposed mid-flight"
        );
        let _ = writeln!(
            out,
            "  cancellation design the canceller is a control-plane thread and"
        );
        let _ = writeln!(
            out,
            "  the target is a shared pool worker -- so a control plane can be"
        );
        let _ = writeln!(out, "  wedged by the very thing it is trying to rescue.");
        let _ = writeln!(out, "  This is why mid-flight cancellation stays deferred.");
    } else {
        let _ = writeln!(
            out,
            "  every attempt returned on this host and this Windows build."
        );
        let _ = writeln!(
            out,
            "  That does NOT clear the design: the original measurement wedged"
        );
        let _ = writeln!(
            out,
            "  with four identical noninvasive samples over twelve seconds, so"
        );
        let _ = writeln!(
            out,
            "  a non-wedge here is a timing difference rather than a refutation."
        );
        let _ = writeln!(
            out,
            "  Re-run, and vary the hammer loop, before concluding otherwise."
        );
    }
    out
}
