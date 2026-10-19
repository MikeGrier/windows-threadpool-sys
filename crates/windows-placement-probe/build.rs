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

/// What CI sets so the build does not have to shell out to `git`.
const COMMIT_ENV: &str = "PLACEMENT_PROBE_COMMIT";

/// What CI sets to declare the build official.
const SOURCE_ENV: &str = "PLACEMENT_PROBE_SOURCE";

fn main() {
    // Without these, a rebuild after a commit would keep the stale stamp --
    // which is the failure this file exists to prevent, arriving by a different
    // route.
    println!("cargo::rerun-if-env-changed={COMMIT_ENV}");
    println!("cargo::rerun-if-env-changed={SOURCE_ENV}");
    watch_git_head(Path::new("../../.git"));

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

    // CI knows the commit it checked out, and a CI checkout is clean by
    // construction, so no `git` call is needed or wanted there.
    let stamped_commit = std::env::var(COMMIT_ENV)
        .ok()
        .map(|sha| sha.trim().to_owned())
        .filter(|sha| !sha.is_empty());

    let (commit, dirty) = match &stamped_commit {
        Some(sha) => (Some(shorten(sha)), Some(false)),
        None => (git_commit(), git_dirty()),
    };

    // **The two stamps are honoured as a unit, never singly.** `ci` is a claim
    // about provenance, and the commit stamp is the evidence for it; accepting
    // the claim alone lets a local build inherit the rest of its identity from
    // the working tree and still pass `BuildIdentity::is_official`, because a
    // clean checkout supplies a commit and `dirty = false` on its own.
    //
    // Measured before fixing: with only `PLACEMENT_PROBE_SOURCE=ci` set, a
    // local build reported `v0.1.0 79b9c4666a1b [ci]` -- no `!!UNOFFICIAL!!`
    // marker, so a record from it would have pooled with real CI results.
    // That inverts this file's stated default, which is that being unable to
    // tell must resolve to unofficial.
    let source = match std::env::var(SOURCE_ENV) {
        Ok(value) if value.trim().eq_ignore_ascii_case("ci") && stamped_commit.is_some() => "ci",
        _ if commit.is_some() => "local",
        _ => "unknown",
    };

    println!(
        "cargo::rustc-env=PLACEMENT_PROBE_COMMIT_OUT={}",
        commit.as_deref().unwrap_or("")
    );
    println!(
        "cargo::rustc-env=PLACEMENT_PROBE_DIRTY_OUT={}",
        match dirty {
            Some(true) => "1",
            Some(false) => "0",
            None => "",
        }
    );
    println!("cargo::rustc-env=PLACEMENT_PROBE_SOURCE_OUT={source}");

    // **Whether a repository was there to ask, which is not the same as whether
    // a commit was found.** The two differ exactly when the build script is
    // broken: a repository present but no commit produced is the silent-failure
    // case the tests exist to catch, and without this flag a test cannot tell
    // that apart from the honest "there was no repository".
    //
    // It exists because the honest case is reachable in ordinary use. A
    // `cargo install` from a crates.io tarball has no repository, a downloaded
    // source zip has none, and `cargo mutants` builds from a scratch copy of
    // the tree with `.git` left behind -- which is what surfaced this: a suite
    // asserting "a commit must have been found" failed in an unmutated tree and
    // stopped the run before a single mutant was tested.
    //
    // Not a `cfg`: this is data about the build, and a test that reads it can
    // assert the *right* thing in each case rather than being skipped.
    println!(
        "cargo::rustc-env=PLACEMENT_PROBE_REPOSITORY_OUT={}",
        if repository_present() { "1" } else { "" }
    );
}
