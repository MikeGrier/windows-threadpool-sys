// Copyright (c) Mike Grier.

//! Renders one long-path run. Shared by the two binaries, which differ only in
//! whether their manifest declares `longPathAware`.
//!
//! **An experiment, not a component.** These probes measure platform behaviour
//! and are not for production use. See this crate's DESIGN-NOTES.md.

use std::fmt::Write as _;

use crate::long_path::{Observation, Shape, is_refusal};

/// The probe's whole report, as text.
#[must_use]
pub fn render(observation: &Observation) -> String {
    let mut out = String::new();
    // First line of the report, and part of the returned text rather than
    // written out here: a captured report must carry the line naming the
    // machine that produced it, and the taint marker with it.
    let _ = writeln!(
        out,
        "{}",
        windows_placement_probe::fingerprint::banner_line()
    );
    let _ = writeln!(
        out,
        "== does the long-path opt-in lift MAX_PATH for a relative path? ==\n"
    );

    let _ = writeln!(
        out,
        "manifest longPathAware : {}",
        if observation.manifest_aware {
            "yes"
        } else {
            "no"
        }
    );
    let _ = writeln!(
        out,
        "LongPathsEnabled       : {}",
        if observation.registry_enabled {
            "1"
        } else {
            "unset or 0"
        }
    );

    if !observation.registry_enabled {
        let _ = writeln!(
            out,
            "\n*** The machine half of the opt-in is absent, so this run measures the\n\
             *** un-opted-in case whatever the manifest says. The rows below are still\n\
             *** real, but they answer a different question than the one intended."
        );
    }

    if let Some(error) = &observation.apparatus_error {
        let _ = writeln!(
            out,
            "\n*** APPARATUS FAILED: {error}\n\
             *** Nothing below says anything about the machine."
        );
        return out;
    }

    let _ = writeln!(
        out,
        "\n{:<18} {:>8} {:>8}  {:<10} error",
        "shape", "resolved", "> MAX", "result"
    );
    for attempt in &observation.attempts {
        let _ = writeln!(
            out,
            "{:<18} {:>8} {:>8}  {:<10} {}",
            attempt.shape.label(),
            attempt.resolved_len,
            if attempt.over_max_path { "yes" } else { "no" },
            if attempt.opened { "opened" } else { "REFUSED" },
            if attempt.opened {
                String::new()
            } else {
                format!(
                    "{}{}",
                    attempt.error,
                    if is_refusal(attempt) {
                        " (not-found; the target provably exists, so this is the length refusal)"
                    } else {
                        ""
                    }
                )
            }
        );
    }

    let _ = writeln!(out, "\n{}", verdict(observation));
    out
}

/// What the rows mean, stated rather than left for the reader to infer.
fn verdict(observation: &Observation) -> String {
    let long: Vec<_> = observation
        .attempts
        .iter()
        .filter(|attempt| attempt.over_max_path)
        .collect();
    if long.is_empty() {
        return "-- no attempt exceeded MAX_PATH, so this run tested nothing.".to_string();
    }

    let plain_long_opened = long
        .iter()
        .any(|attempt| attempt.shape == Shape::Plain && attempt.opened);
    let reparsing: Vec<&str> = long
        .iter()
        .filter(|attempt| !attempt.shape.survives_verbatim_parsing() && !attempt.opened)
        .map(|attempt| attempt.shape.label())
        .collect();

    if !plain_long_opened {
        return "-- MAX_PATH was NOT lifted for a relative path: even the plain shape was\n\
                   refused past the ceiling. The documented reading is wrong for this\n\
                   configuration."
            .to_string();
    }
    if reparsing.is_empty() {
        "-- MAX_PATH was lifted for a relative path, and the path was still parsed\n\
            normally: `..` and forward slashes resolved past the ceiling exactly as\n\
            they do below it. No evidence of a prefix-then-parse implementation."
            .to_string()
    } else {
        format!(
            "-- SHARP EDGE. Length was lifted, but these shapes stopped resolving past\n\
                the ceiling while working below it: {}.\n\
                That is the signature of regularize-then-prefix: `\\\\?\\` disables exactly\n\
                these features, so a relative path changes meaning at MAX_PATH.",
            reparsing.join(", ")
        )
    }
}
