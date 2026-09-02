// Copyright (c) Mike Grier.

//! Print what this machine does with duplicated handles and enumeration state.
//!
//! **An experiment, not a component.** These probes measure platform behaviour
//! and are not for production use: that scope is what lets one do things a
//! shipping component must not. Do not call them from production code, and do
//! not lift a technique out of here. See this crate's DESIGN-NOTES.md.
//!
//! Every finding here is also pinned by a test. The binary exists to show the
//! observations themselves -- the actual file names each handle returned --
//! which is what makes a surprising result diagnosable rather than merely red.

use std::fmt::Write as _;
use windows_platform_probes::handle_state::{
    Fixture, SingleShot, closing_duplicate_preserves_source, duplicate_shares_cursor, ground_truth,
    query_disturbs_cursor, separate_opens_are_independent,
};
use windows_platform_probes::report::{Stdout, emit};

fn main() {
    // The only place that names the real stream. Everything below composes
    // text; nothing below knows where it goes.
    emit(&mut Stdout, &render());
}

/// The probe's whole report, as text.
fn render() -> String {
    let mut out = String::new();
    let fixture = Fixture::new("bin");
    let truth = ground_truth(&fixture);
    let _ = writeln!(out, "--- ground truth (one handle, start to finish) ---");
    let _ = writeln!(out, "{} entries: {truth:?}\n", truth.len());

    let observation = duplicate_shares_cursor(&fixture);
    let _ = writeln!(out, "--- does a duplicate share the cursor? ---");
    let _ = writeln!(out, "source (restart)   -> {:?}", observation.source_first);
    let _ = writeln!(out, "duplicate (continue) -> {:?}", observation.other_next);
    let _ = writeln!(
        out,
        "  -> {}\n",
        if observation.continued(&truth) {
            "SHARED: the duplicate continued where the source stopped"
        } else if observation.restarted() {
            "INDEPENDENT: the duplicate restarted"
        } else {
            "UNCLEAR: neither a clean continuation nor a clean restart"
        }
    );

    let control = separate_opens_are_independent(&fixture);
    let _ = writeln!(out, "--- control: two separate opens ---");
    let _ = writeln!(out, "open #1 (restart)  -> {:?}", control.source_first);
    let _ = writeln!(out, "open #2 (continue) -> {:?}", control.other_next);
    let _ = writeln!(
        out,
        "  -> {}\n",
        if control.restarted() {
            "INDEPENDENT, as expected -- so the result above is attributable to duplication"
        } else {
            "UNEXPECTED: a separate open did not start from the beginning"
        }
    );

    let _ = writeln!(out, "--- does closing the duplicate break the source? ---");
    let _ = writeln!(
        out,
        "  -> {}\n",
        if closing_duplicate_preserves_source(&fixture) {
            "no: the source kept enumerating"
        } else {
            "YES: the source was broken, so owning a duplicate is unsafe to drop"
        }
    );

    let _ = writeln!(
        out,
        "--- does an interleaved single-shot query move the cursor? ---"
    );
    for (query, on_duplicate) in [
        (SingleShot::BasicInfo, false),
        (SingleShot::IdInfo, false),
        (SingleShot::NonEx, false),
        (SingleShot::BasicInfo, true),
    ] {
        let (succeeded, disturbed) = query_disturbs_cursor(&fixture, query, on_duplicate, &truth);
        let _ = writeln!(
            out,
            "{query:?}{}  succeeded={succeeded}  -> {}",
            if on_duplicate {
                " (on the duplicate)"
            } else {
                ""
            },
            if disturbed {
                "DISTURBED"
            } else {
                "undisturbed"
            }
        );
    }
    out
}
