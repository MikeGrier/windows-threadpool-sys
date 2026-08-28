// Copyright (c) Mike Grier.

//! Prints whether impersonation changes which DOS device map a thread resolves
//! drive letters in.
//!
//! This is the measurement behind the session-relative drive-letter hazard that
//! `windows-namespace-request-sys` documents and deliberately does not close: a
//! path resolved on a submitting thread and opened on a worker under a captured
//! token can name a different device.

use windows_platform_probes::device_map::{SubstDrive, measure_with_subst};

fn main() {
    println!("== does impersonation change the DOS device map? ==\n");

    let Some(drive) = SubstDrive::claim("binary") else {
        println!("no free drive letter on this host, so the probe cannot run.");
        println!("(Reported rather than measured: a probe that cannot set up its");
        println!("fixture must say so instead of producing a misleading negative.)");
        return;
    };

    println!(
        "using {} as a subst-style link to {}\n",
        drive.letter(),
        drive.target()
    );
    let finding = measure_with_subst(&drive);

    let describe =
        |label: &str, observation: &windows_platform_probes::device_map::MapObservation| {
            println!("{label}");
            println!(
                "  {} -> {}",
                observation.letter,
                observation
                    .target
                    .as_deref()
                    .unwrap_or("(not found in this map)")
            );
            match observation.logon_session {
                Some((low, high)) => println!("  logon session LUID: {high:08x}:{low:08x}"),
                None => println!("  logon session LUID: (not impersonating)"),
            }
        };

    describe("our own session:", &finding.own_session);
    println!();
    describe(
        "impersonating the anonymous session:",
        &finding.anonymous_session,
    );

    println!("\ncontrol:");
    println!(
        "  the two contexts are different logon sessions: {}",
        finding.sessions_differ()
    );

    println!("\nconclusion:");
    if finding.impersonation_changes_the_map() {
        println!("  the SAME drive letter, on the SAME thread, resolves differently");
        println!("  depending on the token in effect. A path resolved on a submitter");
        println!("  and opened on a worker under a captured token can name a");
        println!("  different device -- which is why lexical resolution does not");
        println!("  close that hazard.");
    } else {
        println!("  the letter resolved the same way in both contexts. Either the");
        println!("  finding has changed, or the impersonation did not take effect --");
        println!("  check the control above before believing the former.");
    }
}
