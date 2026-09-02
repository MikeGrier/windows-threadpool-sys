// Copyright (c) Mike Grier.

//! Prints whether impersonation changes which DOS device map a thread resolves
//! drive letters in.
//!
//! **An experiment, not a component.** These probes measure platform behaviour
//! and are not for production use: that scope is what lets one do things a
//! shipping component must not. Do not call them from production code, and do
//! not lift a technique out of here. See this crate's DESIGN-NOTES.md.
//!
//! This is the measurement behind the session-relative drive-letter hazard that
//! `windows-namespace-request-sys` documents and deliberately does not close: a
//! path resolved on a submitting thread and opened on a worker under a captured
//! token can name a different device.

use std::fmt::Write as _;
use windows_platform_probes::device_map::{SubstDrive, measure_with_subst};
use windows_platform_probes::report::{Stdout, emit};

fn main() {
    // The only place that names the real stream. Everything below composes
    // text; nothing below knows where it goes.
    emit(&mut Stdout, &render());
}

/// The probe's whole report, as text.
fn render() -> String {
    let mut out = String::new();
    let _ = writeln!(out, "== does impersonation change the DOS device map? ==\n");

    let Some(drive) = SubstDrive::claim("binary") else {
        let _ = writeln!(
            out,
            "no free drive letter on this host, so the probe cannot run."
        );
        let _ = writeln!(
            out,
            "(Reported rather than measured: a probe that cannot set up its"
        );
        let _ = writeln!(
            out,
            "fixture must say so instead of producing a misleading negative.)"
        );
        return out;
    };

    let _ = writeln!(
        out,
        "using {} as a subst-style link to {}\n",
        drive.letter(),
        drive.target()
    );
    let finding = measure_with_subst(&drive);

    // Takes the buffer as an argument rather than capturing it, so that `out`
    // stays available to the lines below. A closure capturing it mutably would
    // hold the borrow across every later write.
    fn describe(
        out: &mut String,
        label: &str,
        observation: &windows_platform_probes::device_map::MapObservation,
    ) {
        let _ = writeln!(out, "{label}");
        let _ = writeln!(
            out,
            "  {} -> {}",
            observation.letter,
            observation
                .target
                .as_deref()
                .unwrap_or("(not found in this map)")
        );
        match observation.logon_session {
            Some((low, high)) => {
                let _ = writeln!(out, "  logon session LUID: {high:08x}:{low:08x}");
            }
            None => {
                let _ = writeln!(out, "  logon session LUID: (not impersonating)");
            }
        }
    }

    describe(&mut out, "our own session:", &finding.own_session);
    let _ = writeln!(out);
    describe(
        &mut out,
        "impersonating the anonymous session:",
        &finding.anonymous_session,
    );

    let _ = writeln!(out, "\ncontrol:");
    let _ = writeln!(
        out,
        "  the two contexts are different logon sessions: {}",
        finding.sessions_differ()
    );

    let _ = writeln!(out, "\nconclusion:");
    if finding.impersonation_changes_the_map() {
        let _ = writeln!(
            out,
            "  the SAME drive letter, on the SAME thread, resolves differently"
        );
        let _ = writeln!(
            out,
            "  depending on the token in effect. A path resolved on a submitter"
        );
        let _ = writeln!(
            out,
            "  and opened on a worker under a captured token can name a"
        );
        let _ = writeln!(
            out,
            "  different device -- which is why lexical resolution does not"
        );
        let _ = writeln!(out, "  close that hazard.");
    } else {
        let _ = writeln!(
            out,
            "  the letter resolved the same way in both contexts. Either the"
        );
        let _ = writeln!(
            out,
            "  finding has changed, or the impersonation did not take effect --"
        );
        let _ = writeln!(
            out,
            "  check the control above before believing the former."
        );
    }
    out
}
