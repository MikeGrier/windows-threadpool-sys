// Copyright (c) 2026 Mike Grier
//! Tests for [`BuildIdentity`](super::BuildIdentity).

use super::{BuildIdentity, BuildSource};

/// An official build: CI, known commit, clean tree.
fn official() -> BuildIdentity {
    BuildIdentity {
        crate_version: "2026.902.0",
        commit: Some("abcdef123456"),
        dirty: Some(false),
        source: BuildSource::Ci,
    }
}

#[test]
fn the_default_source_is_the_untrusted_one() {
    // The load-bearing property, matching Provenance one layer down: a value
    // that was never established must not read as trustworthy.
    assert_eq!(BuildSource::default(), BuildSource::Unknown);
}

#[test]
fn the_source_ordering_is_the_trust_order() {
    assert!(BuildSource::Unknown < BuildSource::Local);
    assert!(BuildSource::Local < BuildSource::Ci);
}

#[test]
fn an_official_build_is_ci_with_a_known_commit_and_a_clean_tree() {
    assert!(official().is_official());
}

#[test]
fn every_single_missing_condition_makes_a_build_unofficial() {
    // Exhaustive rather than illustrative: each condition is dropped on its own
    // so no one of them can quietly stop mattering.
    let dirty = BuildIdentity {
        dirty: Some(true),
        ..official()
    };
    let unknown_dirt = BuildIdentity {
        dirty: None,
        ..official()
    };
    let no_commit = BuildIdentity {
        commit: None,
        ..official()
    };
    let local = BuildIdentity {
        source: BuildSource::Local,
        ..official()
    };
    let unknown = BuildIdentity {
        source: BuildSource::Unknown,
        ..official()
    };

    for build in [dirty, unknown_dirt, no_commit, local, unknown] {
        assert!(
            !build.is_official(),
            "{build:?} was accepted as an official build"
        );
    }
}

#[test]
fn an_unknown_tree_state_is_not_treated_as_clean() {
    // The distinction that a boolean would have lost. "We could not ask" and
    // "we asked and it was clean" are different facts, and only the second may
    // support an official build.
    let unknown_dirt = BuildIdentity {
        dirty: None,
        ..official()
    };

    assert_ne!(unknown_dirt.dirty, Some(false));
    assert!(!unknown_dirt.is_official());
}

#[test]
fn an_official_build_renders_without_a_marker() {
    let rendered = official().to_string();

    assert!(!rendered.contains("!!"), "got {rendered}");
    assert!(rendered.contains("v2026.902.0"), "got {rendered}");
    assert!(rendered.contains("abcdef123456"), "got {rendered}");
}

#[test]
fn an_unofficial_build_is_marked_at_the_front() {
    let local = BuildIdentity {
        source: BuildSource::Local,
        ..official()
    };

    assert!(
        local.to_string().starts_with("!!UNOFFICIAL!! "),
        "got {local}"
    );
}

#[test]
fn the_rendering_names_what_is_wrong_rather_than_only_that_something_is() {
    // A reader triaging a surprising submission needs to know *which* property
    // failed: a dirty tree and an unknown commit are different problems.
    let dirty = BuildIdentity {
        dirty: Some(true),
        ..official()
    };
    let no_commit = BuildIdentity {
        commit: None,
        ..official()
    };

    assert!(dirty.to_string().contains("DIRTY"), "got {dirty}");
    assert!(
        no_commit.to_string().contains("commit-unknown"),
        "got {no_commit}"
    );
}

#[test]
fn this_binarys_identity_is_readable_and_names_its_version() {
    // Exercises the build script's stamps rather than a fixture. What the
    // values *are* depends on how this test was built, so only their shape is
    // asserted -- and the crate version is knowable either way.
    let current = BuildIdentity::current();

    assert_eq!(current.crate_version, env!("CARGO_PKG_VERSION"));
    if let Some(commit) = current.commit {
        assert!(!commit.is_empty(), "an empty commit must read as None");
        assert!(commit.len() <= 12, "the commit is meant to be shortened");
    }
}

#[test]
fn the_build_script_stamped_what_this_build_could_determine() {
    // Guards against the build script silently emitting nothing: every stamp
    // would then be empty, `commit` would be `None` everywhere, and the shape
    // assertions above would all still pass while the record carried no
    // identity at all.
    //
    // # Both outcomes are asserted, because both are correct somewhere
    //
    // An earlier version of this test simply required a commit, on the reasoning
    // that the suite is built from a working copy. That is usually true and is
    // not a property of the suite: `cargo mutants` builds from a scratch copy of
    // the tree with `.git` left behind, and this test then failed in the
    // *unmutated* baseline and stopped the run before a single mutant was
    // tested. A `cargo install` from a crates.io tarball and a downloaded source
    // zip reach the same state, and for those the honest answer is `None`.
    //
    // So the build script reports whether a repository was there to ask, and
    // this asserts the right thing in each case rather than skipping. Skipping
    // would leave the silent-emit defect uncaught in exactly the environment the
    // skip fires in; requiring a commit unconditionally calls a correct build a
    // failure. Neither branch is vacuous -- what is forbidden is the build
    // script disagreeing with its own surroundings.
    let current = BuildIdentity::current();

    if built_from_a_repository() {
        let commit = current
            .commit
            .expect("a repository was available, so the commit must have been determined");
        assert!(
            commit.len() == 12 && commit.chars().all(|c| c.is_ascii_hexdigit()),
            "the stamped commit is not a shortened hex sha: {commit:?}"
        );
        assert!(
            current.dirty.is_some(),
            "the tree state must be determinable from a repository"
        );
        assert_eq!(
            current.source,
            BuildSource::Local,
            "a working-copy build must report itself as local"
        );
    } else {
        // The honest-unknown case, and it has real content: "unknown" must mean
        // unknown all the way through rather than a guess or a stale value.
        assert!(
            current.commit.is_none(),
            "no repository was available, yet a commit was stamped: {:?}",
            current.commit
        );
        assert!(
            current.dirty.is_none(),
            "no repository was available, yet a tree state was claimed"
        );
        assert_eq!(
            current.source,
            BuildSource::Unknown,
            "a build that could not identify itself must say so"
        );
    }
}

/// Whether the build script found a repository to read.
///
/// Set by `build.rs`, not recomputed here: what this test checks is that the
/// stamp agrees with the conditions the *build* ran under, and those are not
/// necessarily the conditions the test runs under -- a binary built in a
/// working copy can be executed anywhere.
fn built_from_a_repository() -> bool {
    !env!("PLACEMENT_PROBE_REPOSITORY_OUT").is_empty()
}

#[test]
fn a_local_development_build_does_not_claim_to_be_official() {
    // This suite is never built by the release workflow, so whatever else the
    // build could determine, it must not pass as official. That holds in both
    // worlds the test above distinguishes: a working copy stamps `Local`, and a
    // tree with no repository stamps `Unknown`. If this ever fails, the build
    // script is claiming something it cannot know.
    assert!(
        !BuildIdentity::current().is_official(),
        "a build from a working copy claimed to be official: {}",
        BuildIdentity::current()
    );
}
