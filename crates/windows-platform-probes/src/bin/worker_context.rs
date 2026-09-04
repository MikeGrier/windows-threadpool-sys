// Copyright (c) Mike Grier.

//! Prints what a thread-pool worker starts with, and what it does *not*
//! inherit from an impersonating submitter.
//!
//! **An experiment, not a component.** These probes measure platform behaviour
//! and are not for production use: that scope is what lets one do things a
//! shipping component must not. Do not call them from production code, and do
//! not lift a technique out of here. See this crate's DESIGN-NOTES.md.
//!
//! The asserted tier covers these facts; this binary exists so the same
//! measurement can be eyeballed on a new host or a new Windows build without
//! reading a test's output.

use std::fmt::Write as _;
use windows_platform_probes::report::{Stdout, emit};
use windows_platform_probes::worker_context::{
    observe_on_worker, observe_on_worker_while_impersonating,
};

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
    let _ = writeln!(out, "== what a thread-pool worker is handed ==\n");

    let plain = observe_on_worker();
    let _ = writeln!(out, "submitted with no impersonation:");
    let _ = writeln!(out, "  has thread token   : {}", plain.has_thread_token);
    let _ = writeln!(out, "  OpenThreadToken err: {}", plain.open_token_error);
    let _ = writeln!(out, "  thread error mode  : {:#06x}", plain.error_mode);
    let _ = writeln!(out, "  -> unimpersonated  : {}", plain.is_unimpersonated());
    let _ = writeln!(
        out,
        "  -> critical-error handler enabled: {}",
        plain.critical_error_handler_enabled()
    );

    let impersonating = observe_on_worker_while_impersonating();
    let worker = impersonating.worker;
    let _ = writeln!(
        out,
        "
submitted WHILE the submitter impersonates:"
    );
    let _ = writeln!(
        out,
        "  submitter has token: {}",
        impersonating.submitter.has_thread_token
    );
    let _ = writeln!(out, "  worker has token   : {}", worker.has_thread_token);
    let _ = writeln!(out, "  OpenThreadToken err: {}", worker.open_token_error);
    let _ = writeln!(out, "  thread error mode  : {:#06x}", worker.error_mode);
    let _ = writeln!(out, "  -> unimpersonated  : {}", worker.is_unimpersonated());
    let _ = writeln!(out, "  -> they disagree   : {}", impersonating.disagree());

    let _ = writeln!(
        out,
        "
conclusion:"
    );
    if impersonating.disagree() {
        let _ = writeln!(
            out,
            "  a worker does NOT inherit the submitter's token, so identity"
        );
        let _ = writeln!(out, "  must be captured and applied explicitly.");
    } else {
        let _ = writeln!(
            out,
            "  UNEXPECTED: the worker inherited a token. The ambient crate's"
        );
        let _ = writeln!(
            out,
            "  premise no longer holds and its design needs revisiting."
        );
    }
    if plain.critical_error_handler_enabled() {
        let _ = writeln!(
            out,
            "  a worker's critical-error handler is ENABLED, so a hard device"
        );
        let _ = writeln!(
            out,
            "  error can put a modal dialog on shared infrastructure."
        );
    } else {
        let _ = writeln!(
            out,
            "  UNEXPECTED: a worker starts with the handler suppressed."
        );
    }
    out
}
