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

use windows_platform_probes::worker_context::{
    observe_on_worker, observe_on_worker_while_impersonating,
};

fn main() {
    println!("== what a thread-pool worker is handed ==\n");

    let plain = observe_on_worker();
    println!("submitted with no impersonation:");
    println!("  has thread token   : {}", plain.has_thread_token);
    println!("  OpenThreadToken err: {}", plain.open_token_error);
    println!("  thread error mode  : {:#06x}", plain.error_mode);
    println!("  -> unimpersonated  : {}", plain.is_unimpersonated());
    println!(
        "  -> critical-error handler enabled: {}",
        plain.critical_error_handler_enabled()
    );

    let impersonating = observe_on_worker_while_impersonating();
    let worker = impersonating.worker;
    println!(
        "
submitted WHILE the submitter impersonates:"
    );
    println!(
        "  submitter has token: {}",
        impersonating.submitter.has_thread_token
    );
    println!("  worker has token   : {}", worker.has_thread_token);
    println!("  OpenThreadToken err: {}", worker.open_token_error);
    println!("  thread error mode  : {:#06x}", worker.error_mode);
    println!("  -> unimpersonated  : {}", worker.is_unimpersonated());
    println!("  -> they disagree   : {}", impersonating.disagree());

    println!(
        "
conclusion:"
    );
    if impersonating.disagree() {
        println!("  a worker does NOT inherit the submitter's token, so identity");
        println!("  must be captured and applied explicitly.");
    } else {
        println!("  UNEXPECTED: the worker inherited a token. The ambient crate's");
        println!("  premise no longer holds and its design needs revisiting.");
    }
    if plain.critical_error_handler_enabled() {
        println!("  a worker's critical-error handler is ENABLED, so a hard device");
        println!("  error can put a modal dialog on shared infrastructure.");
    } else {
        println!("  UNEXPECTED: a worker starts with the handler suppressed.");
    }
}
