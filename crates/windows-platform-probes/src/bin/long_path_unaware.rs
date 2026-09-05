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

use windows_platform_probes::report::{Stdout, emit};
use windows_platform_probes::{long_path, long_path_report};

fn main() {
    // The only place that names the real stream. Everything below composes
    // text; nothing below knows where it goes.
    emit(
        &mut Stdout,
        &long_path_report::render(&long_path::measure(false)),
    );
}
