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

use std::fs;
use std::path::{Path, PathBuf};
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
    watch_git_head(Path::new("../../.git"));

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

/// Ask cargo to re-run this script whenever the checked-out commit changes.
///
/// # Watching `HEAD` alone does not work, and the failure is silent
///
/// On a branch, `.git/HEAD` holds `ref: refs/heads/<branch>` -- a line that
/// does not change when you commit. What git rewrites is the *ref* file the
/// line names. Since a `rerun-if-changed` directive replaces cargo's default of
/// watching the whole package, watching only `HEAD` meant the script never re-
/// ran after a commit and the binary kept whatever commit it was first built
/// with.
///
/// Measured on this repository rather than reasoned about: `.git/HEAD` had not
/// been touched in 21 hours while fourteen commits landed, and a freshly built
/// binary reported a commit six behind `HEAD`. CI hides this entirely, because
/// there the commit arrives through `PLACEMENT_PROBE_COMMIT` -- so the stamp
/// was wrong only on local builds, which are exactly the ones whose commit is
/// their only traceability.
///
/// `HEAD` is still watched, because switching branches or detaching does change
/// it.
fn watch_git_head(git_dir: &Path) {
    // A `.git` *file* rather than a directory means a worktree or submodule,
    // and names the real directory. Not watched further: the redirect is enough
    // to find the ref, and a tarball with no `.git` at all is the case this
    // whole file is built to survive.
    let git_dir = match fs::read_to_string(git_dir.join("HEAD")) {
        Ok(_) => git_dir.to_path_buf(),
        Err(_) => match fs::read_to_string(git_dir) {
            Ok(redirect) => match redirect.trim().strip_prefix("gitdir:") {
                Some(path) => PathBuf::from(path.trim()),
                None => return,
            },
            // No repository. The stamp is "unknown", which is the honest answer
            // and needs no watching.
            Err(_) => return,
        },
    };

    let head = git_dir.join("HEAD");
    let Ok(contents) = fs::read_to_string(&head) else {
        return;
    };
    watch(&head);

    // A detached HEAD holds the sha itself, so the file already changes with
    // the commit and there is nothing further to watch.
    let Some(reference) = contents.trim().strip_prefix("ref:") else {
        return;
    };

    // A loose ref is rewritten on every commit. A ref that has been packed does
    // not exist as a file, and `packed-refs` is what changes instead -- so
    // whichever of the two is present is the one to watch. Emitting a path that
    // does not exist would make cargo re-run this script on every single build.
    let loose = git_dir.join(reference.trim());
    if loose.exists() {
        watch(&loose);
    } else {
        let packed = git_dir.join("packed-refs");
        if packed.exists() {
            watch(&packed);
        }
    }
}

/// Emit one `rerun-if-changed`, if the path can be named.
fn watch(path: &Path) {
    if let Some(path) = path.to_str() {
        println!("cargo::rerun-if-changed={path}");
    }
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
