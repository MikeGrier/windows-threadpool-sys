// Copyright (c) Mike Grier.

//! Measures the long-path opt-in **with** `longPathAware` in the manifest.
//!
//! **An experiment, not a component.** These probes measure platform behaviour
//! and are not for production use. See this crate's DESIGN-NOTES.md.
//!
//! Its twin, `probe-long-path-unaware`, is the same code without the manifest.
//! Run both: one row of results proves nothing, because the difference between
//! them is the whole measurement.

use windows_platform_probes::report::{Stdout, emit};
use windows_platform_probes::{long_path, long_path_report};

fn main() {
    // The only place that names the real stream. Everything below composes
    // text; nothing below knows where it goes.
    emit(
        &mut Stdout,
        &long_path_report::render(&long_path::measure(true)),
    );
}
