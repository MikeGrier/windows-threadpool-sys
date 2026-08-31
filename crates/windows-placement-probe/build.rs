// Copyright (c) 2026 Mike Grier
//! Stamps the build's identity into the binary.
//!
//! A measurement that cannot say which build produced it is an unlabelled
//! number. Results from this tool arrive over months, from machines nobody here
//! owns, built from whatever commit was current -- so the record carries the
//! commit, whether the tree was dirty, and whether the build came from CI.
//!
//! # The default is untrusted
//!
//! Every value here can fail to be determined: a `cargo install` from a
//! crates.io tarball has no repository, a downloaded source zip has no `.git`,
//! and `git` may not be on `PATH`. **In every one of those cases the answer is
//! "unknown", never a guess**, and unknown is not official. That mirrors
//! `Provenance::Synthetic` being `Default` one layer down: forgetting, or being
//! unable to tell, must be the safe direction.

use std::process::Command;

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
    println!("cargo::rerun-if-changed=../../.git/HEAD");

    let (commit, dirty) = match std::env::var(COMMIT_ENV) {
        // CI knows the commit it checked out, and a CI checkout is clean by
        // construction, so no `git` call is needed or wanted there.
        Ok(sha) if !sha.trim().is_empty() => (Some(shorten(sha.trim())), Some(false)),
        _ => (git_commit(), git_dirty()),
    };

    let source = match std::env::var(SOURCE_ENV) {
        Ok(value) if value.trim().eq_ignore_ascii_case("ci") => "ci",
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
}

/// The first twelve characters, which is unambiguous in practice and short
/// enough to sit in a printed line.
fn shorten(sha: &str) -> String {
    sha.chars().take(12).collect()
}

fn git_commit() -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let sha = String::from_utf8(output.stdout).ok()?;
    let sha = sha.trim();
    if sha.is_empty() {
        return None;
    }
    Some(shorten(sha))
}

/// Whether the working tree had uncommitted changes.
///
/// `None` when the question could not be asked at all, which is a different
/// fact from "clean" and is reported as such rather than assumed either way.
fn git_dirty() -> Option<bool> {
    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(!output.stdout.is_empty())
}
