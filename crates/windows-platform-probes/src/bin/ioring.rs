// Copyright (c) Mike Grier.

//! Prints the `IoRing` registration and thread-agnosticism findings.
//!
//! Both are environment-dependent: `IoRing` needs a recent Windows build, so
//! this reports "cannot measure" rather than a false negative on a host that
//! has no ring.

use windows_platform_probes::ioring::{
    IoRingSupport, is_available, measure_registration, measure_thread_agnosticism,
};

fn main() {
    println!("== IoRing registration and thread agnosticism ==\n");

    if !is_available() {
        println!("this host has no usable IoRing, so nothing was measured.");
        println!("(Reported rather than measured: 'we could not ask' and 'the answer");
        println!("is no' are different facts, and conflating them is how a design note");
        println!("ends up citing a measurement that never ran.)");
        return;
    }

    println!("registration semantics:");
    match measure_registration() {
        IoRingSupport::Unavailable => println!("  (no ring)"),
        IoRingSupport::Measured(observed) => {
            println!(
                "  after re-registering ONE handle: index 0 usable {}, index 1 usable {}",
                observed.index_zero_usable_after_second, observed.index_one_usable_after_second
            );
            if observed.replaces() {
                println!("  -> REPLACES the whole table, which is what");
                println!("     windows-ioring-sys assumes and refuses a second call on.");
            } else if observed.appends() {
                println!("  -> APPENDS. windows-ioring-sys's index bookkeeping would be");
                println!("     WRONG and its refusal a needless restriction.");
            } else {
                println!("  -> neither: even index 0 stopped working, so the probe");
                println!("     broke rather than the platform answering.");
            }
        }
    }

    println!("\nthread agnosticism:");
    match measure_thread_agnosticism() {
        IoRingSupport::Unavailable => println!("  (no ring)"),
        IoRingSupport::Measured(observed) => {
            println!(
                "  submitter exited: {} | result code: {:#010x}",
                observed.submitter_exited, observed.result_code
            );
            if observed.survives_submitter_exit() {
                println!("  -> an operation OUTLIVES the thread that submitted it, so a");
                println!("     design whose threads are transient by construction is safe.");
            } else {
                println!("  -> the operation did NOT survive its submitter. Every thread");
                println!("     in the proposed design is transient, so this would fail");
                println!("     only under load -- the worst place to discover it.");
            }
        }
    }
}
