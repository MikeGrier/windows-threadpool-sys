// Copyright (c) Mike Grier.

//! Print what this machine does with the `SEM_` bits.
//!
//! **An experiment, not a component.** These probes measure platform behaviour
//! and are not for production use: that scope is what lets one do things a
//! shipping component must not. Do not call them from production code, and do
//! not lift a technique out of here. See this crate's DESIGN-NOTES.md.
//!
//! The assertable findings are pinned by tests; this binary exists to report the
//! same observations on a machine the test suite has not run on -- notably an
//! x64 host, since every measurement so far was taken on ARM64 -- and to perform
//! the one observation a test must not: the alignment bit's process-scope
//! stickiness is irreversible, so it is demonstrated here and nowhere else.

use windows_platform_probes::error_mode::{
    alignment_bit_is_sticky_at_process_scope, bits, combined_invalid_installs_nothing, probe_bit,
    settable_bits, thread_mode_independent_of_process,
};

fn name(bit: u32) -> &'static str {
    match bit {
        bits::FAIL_CRITICAL_ERRORS => "SEM_FAILCRITICALERRORS     (0x0001)",
        bits::NO_GP_FAULT_ERROR_BOX => "SEM_NOGPFAULTERRORBOX      (0x0002)",
        bits::NO_ALIGNMENT_FAULT_EXCEPT => "SEM_NOALIGNMENTFAULTEXCEPT (0x0004)",
        bits::NO_OPEN_FILE_ERROR_BOX => "SEM_NOOPENFILEERRORBOX     (0x8000)",
        _ => "unknown",
    }
}

fn main() {
    println!("--- each bit on its own, set then read back ---");
    for bit in [
        bits::FAIL_CRITICAL_ERRORS,
        bits::NO_GP_FAULT_ERROR_BOX,
        bits::NO_ALIGNMENT_FAULT_EXCEPT,
        bits::NO_OPEN_FILE_ERROR_BOX,
    ] {
        let outcome = probe_bit(bit);
        let verdict = if outcome.is_settable() {
            "SETTABLE"
        } else if outcome.is_silently_dropped() {
            "ACCEPTED BUT SILENTLY DROPPED"
        } else {
            "REJECTED"
        };
        println!(
            "{}  ok={} last_error={:<5} read_back=0x{:04X}  -> {verdict}",
            name(bit),
            outcome.set_ok,
            outcome.last_error,
            outcome.read_back
        );
    }

    println!("\nsettable mask: 0x{:04X}", settable_bits());

    let (installed_nothing, read_back) = combined_invalid_installs_nothing();
    println!("\n--- one invalid bit alongside two valid ones ---");
    println!(
        "read back 0x{read_back:04X}  -> {}",
        if installed_nothing {
            "the WHOLE call failed; none of the valid bits was installed"
        } else {
            "the valid bits survived"
        }
    );

    let observation = thread_mode_independent_of_process();
    println!("\n--- process mode versus thread mode ---");
    println!(
        "process=0x{:04X} thread=0x{:04X}  -> {}",
        observation.process_mode,
        observation.thread_mode,
        if observation.is_independent() {
            "INDEPENDENT storage"
        } else {
            "the process bit SHOWS THROUGH the thread mode"
        }
    );

    println!("\n--- irreversible: the alignment bit at process scope ---");
    println!("(this permanently alters this process, which is why no test does it)");
    let (before, after) = alignment_bit_is_sticky_at_process_scope();
    println!(
        "before=0x{before:04X} after restore attempt=0x{after:04X}  -> {}",
        if after & bits::NO_ALIGNMENT_FAULT_EXCEPT != 0 {
            "STICKY: the restore was ignored"
        } else {
            "clearable"
        }
    );
}
