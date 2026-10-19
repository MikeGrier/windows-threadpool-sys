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

#[test]
fn this_crate_reads_its_own_stamps_not_the_placement_probes() {
    // The two crates are stamped by the same code under different names, so
    // reading the wrong crate's stamp would compile and report a plausible
    // identity. The version is what tells them apart.
    let placement = windows_placement_probe::build_identity::BuildIdentity::current();
    assert_ne!(
        this_build().crate_version,
        placement.crate_version,
        "the two crates' versions coincide, so this test cannot tell them apart"
    );
    assert_eq!(this_build().crate_version, env!("CARGO_PKG_VERSION"));
}
