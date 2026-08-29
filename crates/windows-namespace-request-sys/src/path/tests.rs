// Copyright (c) Mike Grier.
// Copied from windows-file-enumeration-sys/src/path/tests.rs at 126eb5f.

//! Tests for the request path contract.

use super::*;
use std::env;

fn prepare_str(path: &str) -> Result<Wtf16String, PathError> {
    prepare(&Wtf16String::from(path)).map(PreparedPath::into_wtf16)
}

fn text(path: &Wtf16Str) -> String {
    path.to_string_lossy()
}

#[test]
fn an_empty_path_is_rejected() {
    let error = prepare(&Wtf16String::new()).expect_err("an empty path names nothing");
    assert_eq!(error.failure(), PathFailure::EmptyPath);
    assert_eq!(error.raw_os_error(), None);
}

#[test]
fn an_interior_nul_is_rejected() {
    // Win32 would stop at the NUL and open a shorter, different path.
    let path = Wtf16String::from_units(&[0x0043, 0x003A, 0x005C, 0x0000, 0x0061]);
    let error = prepare(&path).expect_err("an interior NUL truncates the path");
    assert_eq!(error.failure(), PathFailure::InteriorNul);
}

#[test]
fn a_verbatim_drive_path_is_kept_exactly() {
    let prepared = prepare_str(r"\\?\C:\Windows\System32").expect("fully qualified");
    assert_eq!(text(&prepared), r"\\?\C:\Windows\System32");
}

#[test]
fn a_verbatim_path_keeps_its_trailing_separator() {
    // In verbatim form a trailing separator is a literal component boundary,
    // not syntax this crate may tidy away.
    let prepared = prepare_str(r"\\?\C:\Windows\").expect("fully qualified");
    assert_eq!(text(&prepared), r"\\?\C:\Windows\");
}

#[test]
fn a_verbatim_path_keeps_dot_components_verbatim() {
    let prepared = prepare_str(r"\\?\C:\a\..\b").expect("fully qualified");
    assert_eq!(text(&prepared), r"\\?\C:\a\..\b");
}

#[test]
fn a_verbatim_path_may_exceed_max_path() {
    let long = format!(r"\\?\C:\{}", "a".repeat(400));
    let prepared = prepare_str(&long).expect("verbatim paths carry no MAX_PATH limit");
    assert_eq!(text(&prepared), long);
}

#[test]
fn a_verbatim_unc_path_is_accepted() {
    let prepared = prepare_str(r"\\?\UNC\server\share\dir").expect("fully qualified");
    assert_eq!(text(&prepared), r"\\?\UNC\server\share\dir");
    // The share alone, with no trailing component, is still a directory.
    prepare_str(r"\\?\UNC\server\share").expect("a share root is fully qualified");
}

#[test]
fn a_verbatim_volume_guid_path_is_accepted() {
    prepare_str(r"\\?\Volume{12345678-1234-1234-1234-123456789abc}\dir")
        .expect("a volume GUID names an absolute root");
}

#[test]
fn a_drive_relative_verbatim_path_is_rejected() {
    // `\\?\C:foo` is drive-*relative*, and verbatim parsing would treat the
    // whole thing as one literal name rather than resolving it.
    let error = prepare_str(r"\\?\C:foo").expect_err("not fully qualified");
    assert_eq!(error.failure(), PathFailure::NotFullyQualified);
}

#[test]
fn a_rootless_verbatim_path_is_rejected() {
    let error = prepare_str(r"\\?\name").expect_err("no root component");
    assert_eq!(error.failure(), PathFailure::NotFullyQualified);
}

#[test]
fn a_verbatim_path_with_an_empty_root_is_rejected() {
    let error = prepare_str(r"\\?\\dir").expect_err("the root component is empty");
    assert_eq!(error.failure(), PathFailure::NotFullyQualified);
}

#[test]
fn an_incomplete_verbatim_unc_path_is_rejected() {
    for path in [
        r"\\?\UNC\server",
        r"\\?\UNC\server\",
        r"\\?\UNC\\share",
        r"\\?\UNC\",
    ] {
        let error = prepare_str(path).expect_err("a server without a share names no filesystem");
        assert_eq!(
            error.failure(),
            PathFailure::NotFullyQualified,
            "for {path}"
        );
    }
}

#[test]
fn an_ordinary_absolute_path_is_resolved_and_kept() {
    let prepared = prepare_str(r"C:\Windows\System32").expect("resolvable");
    assert_eq!(text(&prepared), r"C:\Windows\System32");
}

#[test]
fn an_ordinary_path_is_normalised_by_win32() {
    // Unlike a verbatim path, an ordinary one is the form Win32 itself parses,
    // so resolving it here yields exactly what a later open would have used.
    let prepared = prepare_str(r"C:\Windows\..\Windows\System32").expect("resolvable");
    assert_eq!(text(&prepared), r"C:\Windows\System32");
}

#[test]
fn forward_slashes_are_normalised() {
    let prepared = prepare_str("C:/Windows/System32").expect("resolvable");
    assert_eq!(text(&prepared), r"C:\Windows\System32");
}

#[test]
fn a_relative_path_is_snapshotted_against_the_current_directory() {
    // The whole point of resolving at build time: the answer must not depend on
    // what the current directory happens to be when a worker later runs.
    let current = env::current_dir().expect("a current directory");
    let prepared = prepare_str("subdir").expect("resolvable");
    let expected = current.join("subdir");
    assert_eq!(text(&prepared), expected.to_string_lossy());
}

#[test]
fn an_ordinary_path_longer_than_max_path_is_rejected() {
    let long = format!(r"C:\{}", "a".repeat(400));
    let error = prepare_str(&long).expect_err("beyond the ordinary limit");
    assert_eq!(error.failure(), PathFailure::PathTooLong);
}

#[test]
fn a_relative_path_that_resolves_past_max_path_is_rejected() {
    // Short enough on input, too long once the current directory is prepended:
    // the limit has to be applied to the resolved form as well.
    let current = env::current_dir().expect("a current directory");
    let room = 259usize.saturating_sub(current.to_string_lossy().len());
    let error = prepare_str(&"a".repeat(room + 8)).expect_err("resolves past the ordinary limit");
    assert_eq!(error.failure(), PathFailure::PathTooLong);
}

#[test]
fn a_reserved_device_name_resolves_into_the_device_namespace() {
    // `NUL` is not a directory, but that is discovered when it is opened; the
    // path contract only has to produce a well-formed stored path.
    let prepared = prepare_str("NUL").expect("resolvable");
    assert_eq!(text(&prepared), r"\\.\NUL");
}

#[test]
fn a_device_namespace_path_is_resolved_rather_than_kept_verbatim() {
    // Only `\\?\` disables path parsing; `\\.\` is normalised like any other
    // ordinary form.
    let prepared = prepare_str(r"\\.\C:\Windows\..\Windows").expect("resolvable");
    assert_eq!(text(&prepared), r"\\.\C:\Windows");
}

// The cases below are new here rather than copied: they cover the surface this
// crate added around the relocated preparation.

#[test]
fn a_prepared_path_exposes_its_units_both_ways() {
    let prepared = prepare(&Wtf16String::from(r"\\?\C:\Windows")).expect("fully qualified");

    assert_eq!(prepared.as_wtf16().to_string_lossy(), r"\\?\C:\Windows");
    assert_eq!(
        prepared.into_wtf16().to_string_lossy(),
        r"\\?\C:\Windows",
        "borrowing and taking must agree"
    );
}

#[test]
fn a_prepared_path_is_comparable_and_cloneable() {
    let first = prepare(&Wtf16String::from(r"\\?\C:\Windows")).expect("fully qualified");
    let second = prepare(&Wtf16String::from(r"\\?\C:\Windows")).expect("fully qualified");
    let other = prepare(&Wtf16String::from(r"\\?\C:\Users")).expect("fully qualified");

    assert_eq!(first, second);
    assert_eq!(first, first.clone());
    assert_ne!(first, other);
}

#[test]
fn a_prepared_path_moves_across_threads() {
    const fn assert_send<T: Send>() {}
    const fn assert_sync<T: Sync>() {}

    assert_send::<PreparedPath>();
    assert_sync::<PreparedPath>();

    let prepared = prepare(&Wtf16String::from(r"\\?\C:\Windows")).expect("fully qualified");

    let observed = std::thread::spawn(move || prepared.as_wtf16().to_string_lossy())
        .join()
        .expect("the worker did not panic");

    assert_eq!(observed, r"\\?\C:\Windows");
}

#[test]
fn every_failure_describes_itself_without_a_raw_code() {
    for failure in [
        PathFailure::EmptyPath,
        PathFailure::InteriorNul,
        PathFailure::PathTooLong,
        PathFailure::NotFullyQualified,
        PathFailure::PathResolution,
    ] {
        assert!(
            !failure.description().is_empty(),
            "{failure:?} must describe itself"
        );
    }
}

#[test]
fn an_error_without_an_os_code_renders_only_its_description() {
    let error = prepare(&Wtf16String::new()).expect_err("an empty path names nothing");

    assert_eq!(error.to_string(), PathFailure::EmptyPath.description());
    assert!(std::error::Error::source(&error).is_none());
}
