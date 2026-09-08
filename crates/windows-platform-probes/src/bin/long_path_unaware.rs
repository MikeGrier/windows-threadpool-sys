// Copyright (c) Mike Grier.

//! Measures the long-path opt-in **without** `longPathAware` in the manifest.
//!
//! **An experiment, not a component.** These probes measure platform behaviour
//! and are not for production use. See this crate's DESIGN-NOTES.md.
//!
//! This is the case most consumers of this workspace actually have, which is
//! why it is measured rather than assumed: a library cannot add a manifest to
//! someone else's executable, so whatever this reports is what a caller who has
//! not opted in will meet.

use windows_platform_probes::long_path_report;
use windows_platform_probes::report::emit_report;

fn main() {
    // The probe's whole output policy, and it is one line: hand the renderer to
    // the sink. Nothing here or below names a stream -- that is chosen once, in
    // `report`, so retargeting a probe is not a rewrite.
    //
    // `false` is this binary's whole difference from its twin: `build.rs`
    // embeds the manifest into `probe-long-path-aware` alone, so this one is
    // the same code compiled without the opt-in.
    emit_report(|out| long_path_report::render(out, false));
}
