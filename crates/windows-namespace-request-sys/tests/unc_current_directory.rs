// Copyright (c) Mike Grier.

//! Root-relative rooting under a UNC current directory.
//!
//! An integration test, and a two-process one, because the property is not
//! expressible any other way. The current directory is **process-wide**, so a
//! test that wants to observe what `GetFullPathNameW` does under a UNC one has
//! to *be* a process that has a UNC one -- and it must not impose that on the
//! rest of the suite, which runs as threads in the same process.
//!
//! What it pins is the branch the module doc calls out and nothing else
//! reached: a root-relative path (`\foo`) is rooted at the **root of the
//! current directory**, which under a UNC current directory is the share root
//! `\\server\share\` and not a drive. The unit test for the same rule runs
//! under whatever directory launched the suite, so on ordinary drive-rooted CI
//! it only ever exercises the `C:\foo` half; it computes the UNC expectation
//! but never puts the call in a position to produce one.
//!
//! The child is this same test binary, re-executed with a marker in its
//! environment and a filter naming the one test to run. That keeps the
//! assertion beside the thing it asserts rather than in a fixture binary whose
//! drift nothing would catch.

use windows_namespace_request_sys::ResolveFullPath;
use wtf_string::Wtf16String;

/// Set on the child, absent on the parent. Presence, not value, is the signal.
const CHILD_MARKER: &str = "WNRS_UNC_CURRENT_DIRECTORY_CHILD";

/// The administrative share, which is the only UNC path a test can rely on
/// existing without creating one -- and creating one needs privileges the suite
/// must not assume it has.
const UNC_ROOT: &str = r"\\localhost\C$";

const TEST_NAME: &str = "a_root_relative_path_takes_the_share_root_under_a_unc_current_directory";

// Ignored by default and run explicitly in CI, which is this repository's
// existing "ignored tier" pattern rather than a new one.
//
// `C$` is an ADMINISTRATIVE share. It is reachable for a member of
// Administrators and not for an ordinary user, and it can be switched off
// entirely -- so running by default would turn `cargo test` red for a
// non-administrator developer on a perfectly good host, reporting an
// environment limitation as a defect in the crate. Provisioning a share of our
// own is not an escape: creating one needs the same privileges.
//
// The alternative -- skipping quietly when the share is missing -- is the one
// thing that must not happen, because this is the ONLY coverage of the UNC
// branch and a silent skip would leave it unpinned while reporting otherwise.
// `#[ignore]` keeps that visible: an ignored test is counted and named in the
// output, where a skip inside a passing test is not.
#[test]
#[ignore = "needs a reachable administrative share for a UNC current directory; run in CI with --include-ignored"]
fn a_root_relative_path_takes_the_share_root_under_a_unc_current_directory() {
    if std::env::var_os(CHILD_MARKER).is_some() {
        assert_root_relative_takes_the_share_root();
        return;
    }

    // The precondition is asserted rather than silently skipped. A test that
    // quietly passes when it could not run is the vacuity this crate keeps
    // finding, and it is worse here than elsewhere: this is the only coverage
    // of the branch, so a silent skip would leave the doc unpinned while
    // reporting that it is pinned.
    assert!(
        std::path::Path::new(UNC_ROOT).is_dir(),
        "{UNC_ROOT} is not reachable, so no UNC current directory can be \
         established -- an environment limitation, not a failure of the \
         behaviour under test"
    );

    let exe = std::env::current_exe().expect("the test binary knows its own path");
    let status = std::process::Command::new(exe)
        .arg("--exact")
        .arg(TEST_NAME)
        .arg("--nocapture")
        // Without this the child runs zero tests and exits 0 -- the parent
        // would report success having measured nothing at all. The test it is
        // told to run is the ignored one, so the filter alone is not enough.
        .arg("--include-ignored")
        .env(CHILD_MARKER, "1")
        .current_dir(UNC_ROOT)
        .status()
        .expect("re-execute this test binary with a UNC current directory");

    assert!(
        status.success(),
        "the child, running with {UNC_ROOT} as its current directory, did not \
         pass: {status}"
    );
}

fn assert_root_relative_takes_the_share_root() {
    let cwd = std::env::current_dir().expect("the child has a current directory");
    let cwd = cwd.to_str().expect("the current directory is UTF-8");

    // Proving the child is where the parent put it. Without this the test could
    // pass having silently inherited an ordinary drive-rooted directory --
    // `cmd.exe`, for one, refuses a UNC current directory and falls back to the
    // Windows directory, so a child that is not a plain Win32 process can lose
    // it without saying so.
    assert!(
        cwd.starts_with(r"\\"),
        "precondition: the child must actually hold a UNC current directory, \
         not a drive-rooted one: {cwd}"
    );

    let resolved = ResolveFullPath::new(Wtf16String::from(r"\foo"))
        .perform()
        .expect("a root-relative path resolves");

    assert_eq!(
        resolved.to_string_lossy(),
        format!(r"{UNC_ROOT}\foo"),
        "a root-relative path takes the ROOT of the current directory, which \
         under a UNC current directory is the share root -- so \"the current \
         drive\", as a draft of the module doc said, has no referent here"
    );
}
