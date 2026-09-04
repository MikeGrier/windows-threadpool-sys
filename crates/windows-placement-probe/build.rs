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

use std::fs;
use std::path::Path;
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

/// Whether this build can see a git repository at all.
///
/// Deliberately independent of whether a commit was obtained: it answers "was
/// there something to ask", so a test can distinguish a build script that
/// failed to read an available repository from one that correctly reported an
/// unavailable one.
///
/// `git rev-parse` rather than looking for a `.git` entry, because that entry
/// may be a file redirecting elsewhere, may name a gitdir that no longer
/// exists, and says nothing about whether `git` is on `PATH` -- which is the
/// other way the commit legitimately comes back unknown.
fn repository_present() -> bool {
    Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .is_ok_and(|output| output.status.success())
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
                // **Resolved against the redirect file's own directory.** A
                // `gitdir:` path may be relative, and it is relative to where
                // the `.git` file sits -- not to wherever cargo happens to run
                // this script. Taking it literally produced watch paths under
                // the crate directory, so in a worktree or submodule checkout
                // commits would stop refreshing the stamp: the exact failure
                // this function was added to fix, surviving in the one layout
                // it was added for. `join` still yields an absolute redirect
                // unchanged, so both forms work.
                Some(path) => git_dir.parent().unwrap_or(Path::new(".")).join(path.trim()),
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

    // **Refs may live somewhere other than the gitdir that holds `HEAD`.** A
    // linked worktree keeps a private `HEAD` -- correctly, since each worktree
    // is on its own branch -- but shares every ref with the repository it was
    // created from, naming that shared directory in a `commondir` file beside
    // the private `HEAD`. Looking for the ref in the private gitdir finds
    // nothing, so watching would stop at `HEAD`: on a branch, precisely the file
    // that does not change when you commit, which is the silent staleness this
    // whole function exists to prevent, surviving in a layout it was meant to
    // cover.
    //
    // Measured before fixing rather than reasoned about: in a throwaway
    // `git worktree add`, a commit left the worktree's `HEAD` untouched while
    // the ref under `commondir` was rewritten eight seconds later.
    //
    // A submodule has no `commondir` and keeps its refs in the gitdir its
    // `gitdir:` redirect names, so it falls through to `git_dir` unchanged.
    let refs_dir = match fs::read_to_string(git_dir.join("commondir")) {
        // Relative to the gitdir the file sits in (git writes `../..`).
        Ok(common) => git_dir.join(common.trim()),
        Err(_) => git_dir.clone(),
    };

    // A loose ref is rewritten on every commit, so watching it is exact.
    let loose = refs_dir.join(reference.trim());
    if loose.exists() {
        watch(&loose);
        return;
    }

    // Otherwise the ref is packed: it exists only as a line in `packed-refs`,
    // and that file is what a `git pack-refs` or `git gc` rewrites.
    let packed = refs_dir.join("packed-refs");
    if packed.exists() {
        watch(&packed);
    }

    // **Packed is a state, not a fate, and this is the half that was missing.**
    // The next commit on this branch writes a *loose* ref and leaves
    // `packed-refs` completely untouched -- the stale packed line simply loses
    // to the new file. So watching `packed-refs` alone sees nothing, cargo does
    // not re-run, and the binary keeps the previous commit's stamp.
    //
    // Measured on a clone of this repository rather than reasoned about: after
    // `git pack-refs --all`, a commit moved HEAD from 28f0486205df to
    // 64259d9d8320 while the built binary went on reporting 28f0486205df.
    //
    // Cargo takes a watched directory's newest contained mtime, so naming the
    // nearest existing ancestor catches the loose ref being created. It is
    // deliberately coarse: another branch's commit re-runs this script too,
    // which costs one cheap script run, while missing our own commit costs a
    // wrong answer in a shipped record.
    if let Some(ancestor) = nearest_existing_dir(&loose) {
        watch(&ancestor);
    }
}

/// The closest ancestor of `path` that exists as a directory.
///
/// A branch name may contain slashes, so the loose ref for `feature/x` sits at
/// `refs/heads/feature/x` and *neither* the file nor its `feature` directory
/// need exist while the ref is packed. Walking up finds the deepest directory
/// that does, which is the one that will observe the ref appearing.
fn nearest_existing_dir(path: &Path) -> Option<std::path::PathBuf> {
    let mut candidate = path.parent()?;
    loop {
        if candidate.is_dir() {
            return Some(candidate.to_path_buf());
        }
        candidate = candidate.parent()?;
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
