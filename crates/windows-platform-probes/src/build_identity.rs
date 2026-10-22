// Copyright (c) Mike Grier.

//! Which build of these probes produced a report (`M4.8`).
//!
//! The identity is `windows-placement-probe`'s type, stamped by the same
//! build-script code under this crate's own names, so the two crates' reports
//! name their builds the same way. A capture's runs are only comparable when the
//! same build produced them, and nothing but the report itself can say which
//! build that was.

pub use windows_placement_probe::build_identity::{BuildIdentity, BuildSource};

/// This binary's identity, as this crate's build script stamped it.
///
/// **A dirty build names its commit, not its source.** Two dirty builds at one
/// commit render alike while their trees may differ, so a capture taken from one
/// still has to say in its README what was built.
#[must_use]
pub fn this_build() -> BuildIdentity {
    BuildIdentity::from_stamps(
        env!("CARGO_PKG_VERSION"),
        env!("PLATFORM_PROBES_COMMIT_OUT"),
        env!("PLATFORM_PROBES_DIRTY_OUT"),
        env!("PLATFORM_PROBES_SOURCE_OUT"),
    )
}

#[cfg(test)]
mod tests;
