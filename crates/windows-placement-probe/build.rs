// Copyright (c) 2026 Mike Grier
//! Stamps the build's identity into the binary.
//!
//! A measurement that cannot say which build produced it is an unlabelled
//! number. Results from this tool arrive over months, from machines nobody here
//! owns, built from whatever commit was current -- so the record carries the
//! commit, whether the tree was dirty, and whether the build came from CI.
//!
//! # The default is unofficial
//!
//! Every value here can fail to be determined: a `cargo install` from a
//! crates.io tarball has no repository, a downloaded source zip has no `.git`,
//! and `git` may not be on `PATH`. **In every one of those cases the answer is
//! "unknown", never a guess**, and unknown is not official. That mirrors
//! `Provenance::Synthetic` being `Default` one layer down: forgetting, or being
//! unable to tell, must be the safe direction.

include!("build/stamp.rs");

fn main() {
    // The identity, under this crate's names. CI sets `PLACEMENT_PROBE_COMMIT`
    // and `PLACEMENT_PROBE_SOURCE`; the binary reads the four `*_OUT` stamps.
    stamp_build_identity("PLACEMENT_PROBE");

    // **The dirty flag has a second way to go stale, and these close the common
    // case rather than all of it.** Emitting any `rerun-if-changed` replaces
    // cargo's default of watching the whole package, so without naming them,
    // editing this crate and rebuilding would recompile the binary while
    // leaving the previous run's clean/dirty answer stamped into it.
    //
    // What remains: an uncommitted edit in *another* crate of the workspace
    // rebuilds this one without re-running this script, so the flag can still
    // report a tree cleaner than it is. That is disclosed on the record's
    // `dirty` field rather than papered over, and it only ever affects builds
    // already marked `!!UNOFFICIAL!!` and `[LOCAL]` -- a CI build takes its
    // answer from the environment instead.
    println!("cargo::rerun-if-changed=src");
    println!("cargo::rerun-if-changed=Cargo.toml");
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=build/stamp.rs");
}
