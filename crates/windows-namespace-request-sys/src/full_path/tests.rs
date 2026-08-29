// Copyright (c) Mike Grier.

//! Tests for the `GetFullPathNameW` entry.
//!
//! The negatives matter more than the positives here: this call is lexical, and
//! a suite that only ever resolved existing paths would leave a reader
//! believing it verifies something.

use windows_sys::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
use wtf_string::Wtf16String;

use super::{FullPathError, ResolveFullPath};
use crate::handle::tests::handle_allocation;
use crate::outcome::Win32Error;

fn resolve(path: &str) -> String {
    ResolveFullPath::new(Wtf16String::from(path))
        .perform()
        .expect("resolve the path")
        .to_string_lossy()
}

#[test]
fn dot_and_dotdot_components_are_resolved() {
    assert_eq!(
        resolve(r"C:\Windows\System32\..\.\Temp"),
        r"C:\Windows\Temp"
    );
}

#[test]
fn a_trailing_dot_component_is_resolved_away() {
    assert_eq!(resolve(r"C:\Windows\."), r"C:\Windows");
}

#[test]
fn forward_slashes_are_normalised_to_backslashes() {
    assert_eq!(resolve("C:/Windows/System32"), r"C:\Windows\System32");
}

#[test]
fn an_already_absolute_path_is_returned_unchanged() {
    assert_eq!(resolve(r"C:\Windows"), r"C:\Windows");
}

#[test]
fn a_path_that_does_not_exist_resolves_perfectly_happily() {
    // The call is lexical and touches no filesystem. A consumer wanting a
    // verified path wants an open plus GetFinalPathNameByHandleW.
    assert_eq!(
        resolve(r"C:\no-such-directory\..\nothing-here.txt"),
        r"C:\nothing-here.txt"
    );
}

#[test]
fn a_drive_letter_is_never_expanded() {
    // The hazard this entry does NOT close, asserted so the documentation and
    // the behaviour cannot drift apart: the result still starts with the drive
    // letter, whose meaning depends on the logon session at open time.
    let resolved = resolve(r"C:\Windows");

    assert!(
        resolved.starts_with("C:"),
        "the drive letter survives resolution: {resolved}"
    );
    assert!(
        !resolved.starts_with(r"\\?\"),
        "and it is not turned into a verbatim or device path: {resolved}"
    );
}

#[test]
fn a_relative_path_resolves_against_the_current_directory() {
    // Which is exactly why this must happen where the caller is: the process
    // current directory is shared mutable state.
    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");

    let current = std::env::current_dir().expect("read the current directory");
    let resolved = resolve("some-relative-name.txt");

    let expected = current.join("some-relative-name.txt");
    assert_eq!(
        resolved,
        expected.to_str().expect("the current directory is UTF-8")
    );
}

#[test]
fn a_deeply_nested_path_beyond_the_first_attempt_still_resolves() {
    // Forces the grow-the-buffer retry rather than assuming it works from
    // paths that always fitted the first attempt.
    let segment = "a-directory-with-a-deliberately-long-name";
    let mut path = String::from(r"C:\");
    for index in 0..12 {
        path.push_str(&format!("{segment}-{index:02}\\"));
    }
    path.push_str("file.txt");

    let resolved = resolve(&path);

    assert!(
        resolved.chars().count() > 260,
        "the fixture must exceed the first attempt: {} chars",
        resolved.chars().count()
    );
    assert!(resolved.ends_with("file.txt"), "unexpected: {resolved}");
}

#[test]
fn an_empty_path_reports_the_raw_code() {
    let outcome = ResolveFullPath::new(Wtf16String::new()).perform();

    // Specifically the Win32 variant, not merely "an error". The point of the
    // split is that a caller can tell a refusal by Windows from this crate's own
    // retry giving up; asserting only `is_err` would pass just as happily if
    // every failure collapsed back into one shape.
    assert!(
        matches!(outcome, Err(FullPathError::Win32(_))),
        "an empty path names nothing, and Windows says so rather than this crate"
    );
}

#[test]
fn the_two_failures_are_distinguishable() {
    // The defect this replaced: the unstable case returned a synthesized
    // ERROR_INSUFFICIENT_BUFFER, a code Win32 also returns on its own, so these
    // two values were indistinguishable to a caller matching on the error.
    let refused = FullPathError::from(Win32Error::from_code(ERROR_INSUFFICIENT_BUFFER));
    let gave_up = FullPathError::Unstable {
        attempts: super::MAX_ATTEMPTS,
    };

    assert!(matches!(refused, FullPathError::Win32(_)));
    assert!(matches!(gave_up, FullPathError::Unstable { .. }));

    // And they do not read the same either, so a log distinguishes them too.
    assert_ne!(refused.to_string(), gave_up.to_string());
}

#[test]
fn an_unstable_failure_reports_the_attempts_it_made() {
    let error = FullPathError::Unstable {
        attempts: super::MAX_ATTEMPTS,
    };

    assert!(
        error.to_string().contains(&super::MAX_ATTEMPTS.to_string()),
        "the attempt count is the one diagnostic this variant carries: {error}"
    );
}

#[test]
fn only_a_win32_failure_carries_a_source() {
    use std::error::Error;

    // `Unstable` is this crate's own conclusion rather than something Windows
    // reported, so it has no underlying error to chain to.
    let refused = FullPathError::from(Win32Error::from_code(ERROR_INSUFFICIENT_BUFFER));
    let gave_up = FullPathError::Unstable {
        attempts: super::MAX_ATTEMPTS,
    };

    assert!(refused.source().is_some());
    assert!(gave_up.source().is_none());
}

#[test]
fn the_request_reports_the_path_it_was_built_from() {
    let request = ResolveFullPath::new(Wtf16String::from(r"C:\Windows"));

    assert_eq!(request.path().to_string_lossy(), r"C:\Windows");
}

#[test]
fn a_request_is_cloneable_and_resolves_the_same_way() {
    // Unlike the handle-taking entries, this one owns no handle, so an
    // infallible Clone is honest here.
    let request = ResolveFullPath::new(Wtf16String::from(r"C:\Windows\.\System32"));
    let clone = request.clone();

    assert_eq!(
        clone
            .perform()
            .expect("the clone resolves")
            .to_string_lossy(),
        request
            .perform()
            .expect("the original resolves")
            .to_string_lossy()
    );
}

#[test]
fn a_resolution_performs_the_same_way_on_another_thread() {
    const fn assert_send<T: Send>() {}
    const fn assert_sync<T: Sync>() {}

    assert_send::<ResolveFullPath>();
    assert_sync::<ResolveFullPath>();

    let request = ResolveFullPath::new(Wtf16String::from(r"C:\Windows\.\System32"));

    let resolved = std::thread::spawn(move || {
        request
            .perform()
            .expect("resolve on a worker")
            .to_string_lossy()
    })
    .join()
    .expect("the worker did not panic");

    assert_eq!(resolved, r"C:\Windows\System32");
}
