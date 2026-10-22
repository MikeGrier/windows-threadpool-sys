// Copyright (c) Mike Grier.
//! Tests for this crate's build identity.

use super::*;

#[test]
fn this_build_names_this_crates_version() {
    assert_eq!(this_build().crate_version, env!("CARGO_PKG_VERSION"));
}

#[test]
fn the_build_script_stamped_a_commit_exactly_when_a_repository_was_there() {
    // Both directions, because each failure is real: a repository with no
    // commit stamped is the build script failing silently, and a commit with no
    // repository is a stamp invented from nothing. A scratch copy without `.git`
    // -- which is how cargo-mutants builds -- is the honest second case, so this
    // asserts what the build could determine rather than that a commit exists.
    let repository = !env!("PLATFORM_PROBES_REPOSITORY_OUT").is_empty();
    let stamped_by_ci = !option_env!("PLATFORM_PROBES_COMMIT")
        .unwrap_or_default()
        .trim()
        .is_empty();
    let identity = this_build();
    if repository || stamped_by_ci {
        assert!(
            identity.commit.is_some(),
            "a repository was there (or CI named the commit), and no commit was stamped: \
             {identity}"
        );
    } else {
        assert_eq!(identity.commit, None, "{identity}");
        assert_eq!(identity.source, BuildSource::Unknown, "{identity}");
    }
}

/// Whose stamps a build identity read, as far as the crates' versions can say.
#[derive(Debug, PartialEq, Eq)]
enum Reading {
    /// The identity carries this crate's version.
    Ours,
    /// The identity carries the placement probe's version, so it read the wrong crate's stamps.
    Theirs,
    /// The two crates' versions coincide, so the version cannot say whose stamps were read.
    CannotTell,
}

/// What `identity` says about whose stamps it is, given this crate's version and the placement
/// probe's. Both crates are calendar-versioned, so released on one day they carry the same version,
/// and then nothing in a `BuildIdentity` tells them apart.
fn whose_stamps(identity: &str, ours: &str, theirs: &str) -> Reading {
    if ours == theirs {
        Reading::CannotTell
    } else if identity == ours {
        Reading::Ours
    } else if identity == theirs {
        Reading::Theirs
    } else {
        panic!("an identity carrying neither crate's version: {identity}")
    }
}

#[test]
fn a_reading_is_ours_theirs_or_unknowable_by_the_two_versions() {
    assert_eq!(
        whose_stamps("2026.1022.0", "2026.1022.0", "2026.1021.0"),
        Reading::Ours
    );
    assert_eq!(
        whose_stamps("2026.1021.0", "2026.1022.0", "2026.1021.0"),
        Reading::Theirs
    );
    // The two directions of the same fact: equal versions cannot be told apart whichever way the
    // identity was read, so neither answer is given.
    for identity in ["2026.1022.0", "2026.1021.0"] {
        assert_eq!(
            whose_stamps(identity, "2026.1022.0", "2026.1022.0"),
            Reading::CannotTell,
            "{identity}"
        );
    }
}

#[test]
#[should_panic(expected = "carrying neither crate's version")]
fn an_identity_carrying_neither_version_is_an_error_not_a_reading() {
    whose_stamps("2026.1020.0", "2026.1022.0", "2026.1021.0");
}

#[test]
fn this_crate_reads_its_own_stamps_not_the_placement_probes() {
    // The two crates are stamped by the same code under different names, so
    // reading the wrong crate's stamp would compile and report a plausible
    // identity. The version is what tells them apart -- and only while they differ:
    // both crates are calendar-versioned, so a release of both on one day gives
    // them one version, and then this test cannot tell, which is a fact about the
    // versions and not a failure of the build. It says so rather than failing, and
    // the design notes record what that leaves unchecked.
    let placement = windows_placement_probe::build_identity::BuildIdentity::current();
    let reading = whose_stamps(
        this_build().crate_version,
        env!("CARGO_PKG_VERSION"),
        placement.crate_version,
    );
    match reading {
        Reading::Ours => {}
        Reading::Theirs => panic!(
            "this crate's build identity carries the placement probe's version, so it read the \
             placement probe's stamps"
        ),
        Reading::CannotTell => eprintln!(
            "the two probe crates share the version {}, so this test cannot tell whose stamps \
             this_build() read",
            env!("CARGO_PKG_VERSION")
        ),
    }
    assert_eq!(this_build().crate_version, env!("CARGO_PKG_VERSION"));
}
