// Copyright (c) Mike Grier.

//! Measures the long-path opt-in **with** `longPathAware` in the manifest.
//!
//! **An experiment, not a component.** These probes measure platform behaviour
//! and are not for production use. See this crate's DESIGN-NOTES.md.
//!
//! Its twin, `probe-long-path-unaware`, is the same code without the manifest.
//! Run both: one row of results proves nothing, because the difference between
//! them is the whole measurement.

use windows_platform_probes::long_path_report;
use windows_platform_probes::report::emit_report;

fn main() {
    // The probe's whole output policy, and it is one line: hand the renderer to
    // the sink. Nothing here or below names a stream -- that is chosen once, in
    // `report`, so retargeting a probe is not a rewrite.
    //
    // The manifest is this binary's whole difference from its twin, and it is a
    // claim about the build rather than something measured at runtime: the two
    // halves of the opt-in are a machine-wide registry value and a per-executable
    // manifest, neither of which is a switch a process can read off itself.
    //
    // So the claim is *derived* from the flag `build.rs` sets in the same branch
    // that embeds the manifest, never hardcoded. Hardcoding `true` here would let
    // a target whose linker the script skips -- anything non-MSVC -- report
    // `manifest longPathAware : yes` for an executable carrying no manifest,
    // while both halves quietly measured the same un-opted-in case.
    let manifest_aware = cfg!(long_path_manifest_embedded);
    emit_report(|out| long_path_report::render(out, manifest_aware));
}
