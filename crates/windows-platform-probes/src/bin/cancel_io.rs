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

use windows_platform_probes::cancel_io::{
    CancelOutcome, WATCHDOG, cancel_against_busy_thread, cancel_against_idle_thread,
};

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
    println!("== is CancelSynchronousIo safe to point at a shared thread? ==\n");
    println!("watchdog: {WATCHDOG:?} per attempt\n");

    println!("case 1: cancel against a thread with no I/O outstanding");
    let idle = cancel_against_idle_thread();
    println!("  {}", describe(idle));
    println!(
        "  -> a point-in-time cancel finds nothing here; a standing request\n     against the thread would not."
    );

    println!("\ncase 2: cancel against a thread hammering synchronous reads");
    let busy = cancel_against_busy_thread(4);
    for (attempt, outcome) in busy.iter().enumerate() {
        println!("  attempt {}: {}", attempt + 1, describe(*outcome));
    }

    let wedged = busy.iter().any(|outcome| !outcome.returned());

    println!("\nconclusion:");
    if wedged {
        println!("  CancelSynchronousIo DID NOT RETURN against a thread that keeps");
        println!("  re-entering synchronous I/O. In the proposed mid-flight");
        println!("  cancellation design the canceller is a control-plane thread and");
        println!("  the target is a shared pool worker -- so a control plane can be");
        println!("  wedged by the very thing it is trying to rescue.");
        println!("  This is why mid-flight cancellation stays deferred.");
    } else {
        println!("  every attempt returned on this host and this Windows build.");
        println!("  That does NOT clear the design: the original measurement wedged");
        println!("  with four identical noninvasive samples over twelve seconds, so");
        println!("  a non-wedge here is a timing difference rather than a refutation.");
        println!("  Re-run, and vary the hammer loop, before concluding otherwise.");
    }
}
