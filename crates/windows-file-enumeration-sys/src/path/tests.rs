// Copyright (c) 2026 Mike Grier
//! Tests for the request path contract.

use super::*;
use std::env;

fn prepare_str(path: &str) -> Result<Wtf16String, RequestError> {
    prepare(&Wtf16String::from(path))
}

fn text(path: &Wtf16Str) -> String {
    path.to_string_lossy()
}

#[test]
fn an_empty_path_is_rejected() {
    let error = prepare(&Wtf16String::new()).expect_err("an empty path names nothing");
    assert_eq!(error.failure(), RequestFailure::EmptyPath);
    assert_eq!(error.code(), None);
}

#[test]
fn an_interior_nul_is_rejected() {
    // Win32 would stop at the NUL and open a shorter, different path.
    let path = Wtf16String::from_units(&[0x0043, 0x003A, 0x005C, 0x0000, 0x0061]);
    let error = prepare(&path).expect_err("an interior NUL truncates the path");
    assert_eq!(error.failure(), RequestFailure::InteriorNul);
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
    assert_eq!(error.failure(), RequestFailure::NotFullyQualified);
}

#[test]
fn a_rootless_verbatim_path_is_rejected() {
    let error = prepare_str(r"\\?\name").expect_err("no root component");
    assert_eq!(error.failure(), RequestFailure::NotFullyQualified);
}

#[test]
fn a_verbatim_path_with_an_empty_root_is_rejected() {
    let error = prepare_str(r"\\?\\dir").expect_err("the root component is empty");
    assert_eq!(error.failure(), RequestFailure::NotFullyQualified);
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
            RequestFailure::NotFullyQualified,
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
    assert_eq!(error.failure(), RequestFailure::PathTooLong);
}

#[test]
fn a_relative_path_that_resolves_past_max_path_is_rejected() {
    // Short enough on input, too long once the current directory is prepended:
    // the limit has to be applied to the resolved form as well.
    let current = env::current_dir().expect("a current directory");
    let room = 259usize.saturating_sub(current.to_string_lossy().len());
    let error = prepare_str(&"a".repeat(room + 8)).expect_err("resolves past the ordinary limit");
    assert_eq!(error.failure(), RequestFailure::PathTooLong);
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

// ---------------------------------------------------------------------------
// Boundaries.
//
// The tests above use comfortably-wrong values -- a 400-character path, a
// relative path eight units past the limit -- which prove the check exists but
// not that it is in the right place. A mutation sweep moved the limit by one in
// both directions and changed `>` to `>=` and `==`, and every one of those
// survived. These pin the exact unit at which the answer changes.
// ---------------------------------------------------------------------------

/// An absolute path of exactly `units` UTF-16 units, already in normal form so
/// `GetFullPathNameW` returns it unchanged and the resolved length is the input
/// length.
fn absolute_path_of_length(units: usize) -> String {
    let prefix = r"C:\";
    format!("{prefix}{}", "a".repeat(units - prefix.len()))
}

#[test]
fn an_ordinary_path_of_exactly_max_path_content_is_accepted() {
    // 259 = MAX_PATH - 1, the longest path that leaves room for the terminator.
    // Rejecting this is the off-by-one that a "400 characters is too long" test
    // cannot see, and it is the expensive direction: it refuses a path Windows
    // would have opened.
    let path = absolute_path_of_length(259);
    assert_eq!(path.chars().count(), 259);

    let prepared = prepare_str(&path).expect("259 units is within the ordinary limit");
    assert_eq!(text(&prepared), path);
}

#[test]
fn an_ordinary_path_one_unit_past_max_path_content_is_rejected() {
    // 260 counts the terminator, so 260 content units do not fit.
    let path = absolute_path_of_length(260);
    assert_eq!(path.chars().count(), 260);

    let error = prepare_str(&path).expect_err("260 units leaves no room for the terminator");
    assert_eq!(error.failure(), RequestFailure::PathTooLong);
}

#[test]
fn the_ordinary_limit_is_one_less_than_max_path() {
    // The relationship the two tests above rest on, stated directly so a change
    // to the constant fails here with its reason rather than only as a puzzling
    // length assertion elsewhere.
    assert_eq!(MAX_PATH_CONTENT, MAX_PATH - 1);
    assert_eq!(MAX_PATH_CONTENT, 259);
}

#[test]
fn a_verbatim_drive_relative_path_with_a_separator_is_rejected() {
    // `\\?\C:foo` -- the existing case -- has no separator at all, so it is
    // refused before the root is ever inspected and never reaches the
    // drive-designator check. This form does reach it: the root is `C:foo`,
    // which contains a colon but is not a drive.
    //
    // Without this, the check could report every root as a drive and nothing
    // would notice.
    let error = prepare_str(r"\\?\C:foo\bar").expect_err("drive-relative, not fully qualified");
    assert_eq!(error.failure(), RequestFailure::NotFullyQualified);
}

#[test]
fn a_verbatim_root_needs_a_letter_before_its_colon_not_merely_a_colon() {
    // Both halves of the drive-designator test are load-bearing, and only a
    // root that satisfies one but not the other shows it. `1:` has the colon in
    // the right place and is still not a drive, so a check that accepted
    // *either* condition would wave it through.
    for path in [r"\\?\1:\", r"\\?\1:\dir"] {
        let error = prepare_str(path).expect_err("a digit is not a drive letter");
        assert_eq!(
            error.failure(),
            RequestFailure::NotFullyQualified,
            "for {path}"
        );
    }

    // Deliberately no companion case for "second unit is not a colon". The
    // check is guarded by `root.contains(&COLON)`, so a colonless root -- a
    // volume GUID, say -- never reaches it and is accepted on its own terms.
    // `1:` is therefore the only shape that satisfies one half of the rule
    // while failing the other, which is what makes it the whole test.
    let prepared = prepare_str(r"\\?\Ca\dir").expect("a colonless root is not a drive at all");
    assert_eq!(text(&prepared), r"\\?\Ca\dir");
}

#[test]
fn a_verbatim_drive_root_is_still_accepted() {
    // The positive case for the two tests above, so a check that rejected
    // everything would not pass them by being uniformly strict.
    let prepared = prepare_str(r"\\?\C:\dir").expect("a drive root is fully qualified");
    assert_eq!(text(&prepared), r"\\?\C:\dir");
}
