// Copyright (c) 2026 Mike Grier
//! Tests for [`BuildIdentity`](super::BuildIdentity).

use super::{BuildIdentity, BuildSource};

/// An official build: CI, known commit, clean tree.
fn official() -> BuildIdentity {
    BuildIdentity {
        crate_version: "0.1.0",
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
    assert!(rendered.contains("v0.1.0"), "got {rendered}");
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
fn the_build_script_stamped_a_real_commit_here() {
    // Guards against the build script silently emitting nothing: every stamp
    // would then be empty, `commit` would be `None` everywhere, and the shape
    // assertions above would all still pass while the record carried no
    // identity at all.
    //
    // This suite is built from a git working copy, so the commit *is*
    // determinable and must have been determined. On a machine where it is
    // genuinely unavailable -- a crates.io tarball, a source zip -- this test
    // is not the one that runs, because that is not where the suite runs.
    let current = BuildIdentity::current();

    let commit = current
        .commit
        .expect("the build script must find a commit when built from a repository");
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
}

#[test]
fn a_local_development_build_does_not_claim_to_be_official() {
    // This suite runs from a working copy, never from CI, so the binary under
    // test must not pass as official. If this ever fails, the build script is
    // claiming something it cannot know.
    assert!(
        !BuildIdentity::current().is_official(),
        "a build from a working copy claimed to be official: {}",
        BuildIdentity::current()
    );
}
