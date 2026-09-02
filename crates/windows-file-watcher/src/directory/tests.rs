// Copyright (c) 2026 Mike Grier
//! Unit tests for opening a watched directory.
//!
//! Every test drives a real `CreateFileW`, since the contract under test is
//! about what Win32 accepts and how its failures classify, not about any
//! abstraction this crate layers on top.

use std::path::{Path, PathBuf};

use super::{DirectoryHandle, OpenFailure, VolumeIdentity};

/// A uniquely named temp directory, removed when the test passes.
///
/// Named by pid plus a nanosecond nonce so concurrent and repeated runs never
/// collide. Cleanup is deliberately *not* RAII: if an assertion panics,
/// unwinding skips it and the directory is left for post-mortem inspection.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "windows-file-watcher-{label}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&path).expect("create temp dir");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn cleanup(self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

// --- opens that must succeed ---

#[test]
fn opens_a_plain_directory() {
    let dir = TempDir::new("open-plain");
    assert!(DirectoryHandle::open(dir.path()).is_ok());
    dir.cleanup();
}

#[test]
fn opens_the_system_temp_directory() {
    assert!(DirectoryHandle::open(&std::env::temp_dir()).is_ok());
}

#[test]
fn opens_the_current_directory_by_relative_path() {
    // Also a D-85 guard: a relative path cannot survive a `\\?\` prefix, which
    // accepts only fully qualified paths. See the section at the end of this
    // file for the rest of the pass-through guards.
    assert!(
        DirectoryHandle::open(Path::new(".")).is_ok(),
        "a relative path must be accepted; Win32 resolves it"
    );
}

#[test]
fn opens_a_nested_directory() {
    let dir = TempDir::new("open-nested");
    let nested = dir.path().join("a").join("b");
    std::fs::create_dir_all(&nested).expect("create nested");
    assert!(DirectoryHandle::open(&nested).is_ok());
    dir.cleanup();
}

#[test]
fn opens_a_directory_whose_name_is_not_ascii() {
    let dir = TempDir::new("open-unicode");
    // Includes an astral character, so the name is a surrogate pair in UTF-16.
    let child = dir.path().join("caf\u{e9}-\u{1f600}-\u{10437}");
    std::fs::create_dir(&child).expect("create unicode-named dir");
    assert!(DirectoryHandle::open(&child).is_ok());
    dir.cleanup();
}

#[test]
fn opens_a_directory_with_a_trailing_separator() {
    let dir = TempDir::new("open-trailing");
    let with_sep = format!("{}\\", dir.path().display());
    assert!(DirectoryHandle::open(Path::new(&with_sep)).is_ok());
    dir.cleanup();
}

#[test]
fn opens_a_directory_containing_files() {
    let dir = TempDir::new("open-populated");
    for index in 0..8 {
        std::fs::write(dir.path().join(format!("file-{index}.txt")), b"x").expect("write");
    }
    assert!(DirectoryHandle::open(dir.path()).is_ok());
    dir.cleanup();
}

#[test]
fn the_handle_is_usable_and_reports_itself() {
    let dir = TempDir::new("open-usable");
    let handle = DirectoryHandle::open(dir.path()).expect("open");
    assert!(!handle.as_raw().is_null(), "a live handle is never null");
    assert!(
        format!("{handle:?}").contains("DirectoryHandle"),
        "Debug names the type"
    );
    dir.cleanup();
}

#[test]
fn the_share_mode_admits_a_second_watcher() {
    // A watcher is an observer: opening a directory must not lock anyone else
    // out, including a second watch on the same directory.
    let dir = TempDir::new("open-shared");
    let first = DirectoryHandle::open(dir.path()).expect("first open");
    let second = DirectoryHandle::open(dir.path()).expect("second open");
    assert_ne!(
        first.as_raw(),
        second.as_raw(),
        "the two opens are distinct handles"
    );
    dir.cleanup();
}

#[test]
fn the_share_mode_admits_writers_while_watching() {
    // FILE_SHARE_WRITE: holding the directory open must not stop files being
    // created or modified inside it, which is the whole point of watching.
    let dir = TempDir::new("open-writable");
    let handle = DirectoryHandle::open(dir.path()).expect("open");
    std::fs::write(dir.path().join("created-while-open.txt"), b"data")
        .expect("create a file while the directory handle is open");
    drop(handle);
    dir.cleanup();
}

#[test]
fn many_sequential_opens_do_not_exhaust_handles() {
    // Each open must actually close on drop; a leak would show up here.
    let dir = TempDir::new("open-repeat");
    for _ in 0..256 {
        let handle = DirectoryHandle::open(dir.path()).expect("open");
        drop(handle);
    }
    dir.cleanup();
}

// --- classified failures ---

#[test]
fn a_missing_leaf_classifies_as_not_found() {
    let dir = TempDir::new("missing-leaf");
    let missing = dir.path().join("does-not-exist");
    let error = DirectoryHandle::open(&missing).expect_err("must fail");
    assert_eq!(error.failure(), OpenFailure::NotFound);
    assert!(
        error.failure().is_retryable(),
        "a path may legitimately appear later"
    );
    dir.cleanup();
}

#[test]
fn a_missing_intermediate_component_classifies_as_not_found() {
    // Win32 reports ERROR_PATH_NOT_FOUND here rather than ERROR_FILE_NOT_FOUND;
    // both must land in the same class.
    let dir = TempDir::new("missing-parent");
    let missing = dir.path().join("absent-parent").join("leaf");
    let error = DirectoryHandle::open(&missing).expect_err("must fail");
    assert_eq!(error.failure(), OpenFailure::NotFound);
    dir.cleanup();
}

#[test]
fn a_regular_file_classifies_as_not_a_directory() {
    // FILE_LIST_DIRECTORY and FILE_READ_DATA are the same bit, so this open
    // succeeds at the Win32 level and only the explicit attribute check rejects
    // it. Without that check the mistake would surface much later, as a
    // mis-classified read failure.
    let dir = TempDir::new("not-a-dir");
    let file = dir.path().join("regular.txt");
    std::fs::write(&file, b"contents").expect("write file");
    let error = DirectoryHandle::open(&file).expect_err("must fail");
    assert_eq!(error.failure(), OpenFailure::NotADirectory);
    assert!(
        !error.failure().is_retryable(),
        "a file will never become a directory"
    );
    dir.cleanup();
}

#[test]
fn an_interior_nul_classifies_as_invalid_path_without_calling_win32() {
    // Win32 would stop at the NUL and open a shorter path than asked for, so
    // this must be rejected outright rather than passed through.
    let dir = TempDir::new("interior-nul");
    // The prefix alone is a real, openable directory -- which is exactly the
    // silent-truncation hazard being guarded against.
    assert!(DirectoryHandle::open(dir.path()).is_ok());

    let mut poisoned = std::ffi::OsString::from(dir.path());
    poisoned.push(unsafe_nul());
    poisoned.push("trailing");
    let error = DirectoryHandle::open(Path::new(&poisoned)).expect_err("must fail");
    assert_eq!(error.failure(), OpenFailure::InvalidPath);
    assert!(!error.failure().is_retryable());
    dir.cleanup();
}

/// An `OsString` fragment holding a single NUL code unit.
fn unsafe_nul() -> std::ffi::OsString {
    use std::os::windows::ffi::OsStringExt;
    std::ffi::OsString::from_wide(&[0u16])
}

#[test]
fn an_empty_path_fails_and_is_classified() {
    // Whatever Win32 reports, it must classify rather than escape unclassified.
    let error = DirectoryHandle::open(Path::new("")).expect_err("must fail");
    assert!(
        matches!(
            error.failure(),
            OpenFailure::NotFound | OpenFailure::InvalidPath | OpenFailure::Retryable
        ),
        "unexpected classification: {:?}",
        error.failure()
    );
}

#[test]
fn a_failure_preserves_the_underlying_os_error() {
    let dir = TempDir::new("preserve-os-error");
    let missing = dir.path().join("nope");
    let error = DirectoryHandle::open(&missing).expect_err("must fail");
    // Through the `Error::source` chain, which is the surface a caller actually
    // has: the classification is ours, the code underneath is the platform's.
    let underlying = std::error::Error::source(&error)
        .and_then(|source| source.downcast_ref::<std::io::Error>())
        .expect("the original error is kept as the source");
    assert!(
        underlying.raw_os_error().is_some(),
        "the original OS error code is kept for diagnostics"
    );
    assert!(
        format!("{error}").contains("NotFound"),
        "Display names the classification"
    );
    dir.cleanup();
}

#[test]
fn every_failure_class_agrees_with_its_retry_policy() {
    // The permanent pair is exactly the caller-input pair; everything
    // environmental stays retryable, which is what D-14 requires.
    assert!(OpenFailure::NotFound.is_retryable());
    assert!(OpenFailure::Unsupported.is_retryable());
    assert!(OpenFailure::Retryable.is_retryable());
    assert!(!OpenFailure::NotADirectory.is_retryable());
    assert!(!OpenFailure::InvalidPath.is_retryable());
}

// --- `canonical_path`: where the handle actually is, not what opened it ---

#[test]
fn canonical_path_reports_where_the_handle_actually_is() {
    // The point of this call is that it does *not* echo back the string that
    // opened the handle, so the test compares against the OS's own answer for
    // the same directory rather than against that string.
    let dir = TempDir::new("canonical-basic");
    let handle = DirectoryHandle::open(dir.path()).expect("open");

    let reported = handle.canonical_path().expect("canonical path");
    let expected = std::fs::canonicalize(dir.path()).expect("std canonicalize");
    assert_eq!(
        reported, expected,
        "the reported path must name the same object the OS resolves this \
         directory to"
    );

    drop(handle);
    dir.cleanup();
}

#[test]
fn canonical_path_follows_a_rename_rather_than_reporting_the_opening_string() {
    // A handle keeps naming its object across a rename, so the path a client
    // opened with can go stale while the handle stays perfectly valid. Being
    // fresh rather than cached is the whole reason this exists -- a diagnostic
    // that printed the opening string here would name a directory that is no
    // longer there.
    let dir = TempDir::new("canonical-rename");
    let handle = DirectoryHandle::open(dir.path()).expect("open");
    let before = handle.canonical_path().expect("canonical path before");

    let renamed = dir.path().with_file_name(format!(
        "windows-file-watcher-canonical-renamed-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&renamed);
    std::fs::rename(dir.path(), &renamed).expect("rename the open directory");

    let after = handle.canonical_path().expect("canonical path after");
    assert_ne!(
        before, after,
        "a fresh query must notice the rename; an equal answer would mean the \
         path had been cached at open"
    );
    assert_eq!(
        after,
        std::fs::canonicalize(&renamed).expect("std canonicalize"),
        "and it must name where the object went, not merely differ"
    );

    drop(handle);
    let _ = std::fs::remove_dir_all(&renamed);
    dir.cleanup();
}

#[test]
fn volume_identity_equality_is_on_the_serial_alone() {
    // PR #20 review response: the filesystem name and volume label are both
    // mutable (a rename, or different media sharing a label/filesystem
    // type), so neither is a sound identity signal -- only the volume
    // serial number is.
    let a = VolumeIdentity::synthetic(1, "NTFS", "SAME-LABEL");
    let b = VolumeIdentity::synthetic(1, "FAT32", "DIFFERENT-LABEL");
    assert_eq!(
        a, b,
        "the same serial is the same volume, regardless of label/filesystem \
         (a mere rename must not look like a media swap)"
    );

    let c = VolumeIdentity::synthetic(2, "NTFS", "SAME-LABEL");
    assert_ne!(
        a, c,
        "a different serial is different media, even with an identical \
         label/filesystem (media swapped for other media sharing a label \
         must not go undetected)"
    );
}

#[cfg(feature = "test-util")]
#[test]
fn volume_identity_for_test_is_the_public_synthetic_seam() {
    let a = VolumeIdentity::for_test(0x1234, "NTFS", "System");
    let b = VolumeIdentity::for_test(0x1234, "ReFS", "Data");
    let c = VolumeIdentity::for_test(0x9999, "NTFS", "System");
    assert_eq!(a.filesystem_name(), "NTFS");
    assert_eq!(a.volume_label(), "System");
    // Identity compares by volume serial alone.
    assert_eq!(a, b);
    assert_ne!(a, c);
}

// --- pure helpers and accessors (mutation-testing gaps) ---
//
// A `cargo mutants` run left survivors in three places that need no Win32 call
// at all. Every test in this file above drives a real `CreateFileW`, which is
// right for the open/classify contract but meant the pure helpers underneath it
// were only ever exercised incidentally, on whatever values a real handle
// happened to produce.

#[test]
fn trim_nul_stops_at_the_first_nul() {
    // `replace == with !=` survived here, which inverts the search into "stop
    // at the first non-NUL" -- returning an empty slice for every real buffer.
    let units: Vec<u16> = "AB\0CD".encode_utf16().collect();
    assert_eq!(
        super::trim_nul(&units),
        &"AB".encode_utf16().collect::<Vec<_>>()[..],
        "the content before the first NUL is the string"
    );
}

#[test]
fn trim_nul_keeps_a_slice_with_no_nul_whole() {
    // The `map_or` default. A fixed-size Win32 buffer filled exactly to its
    // length has no terminator to find, and truncating it would silently drop
    // the last unit.
    let units: Vec<u16> = "ABCD".encode_utf16().collect();
    assert_eq!(super::trim_nul(&units), &units[..]);
}

#[test]
fn trim_nul_of_a_leading_nul_is_empty() {
    let units = [0u16, 65, 66];
    assert!(
        super::trim_nul(&units).is_empty(),
        "a buffer Win32 wrote nothing into is the empty string, not its residue"
    );
}

#[test]
fn trim_nul_of_an_empty_slice_is_empty() {
    assert!(super::trim_nul(&[]).is_empty());
}

#[test]
fn trim_nul_ignores_everything_after_the_first_nul() {
    // Two NULs with content between them: the residue of a previous, longer
    // write is exactly what a reused buffer holds.
    let units = [65u16, 0, 66, 0, 67];
    assert_eq!(super::trim_nul(&units), &[65u16][..]);
}

#[test]
fn each_open_failure_code_classifies_to_its_own_outcome() {
    // Deleting the `ERROR_DIRECTORY` and `ERROR_INVALID_NAME` match arms both
    // survived: every arm falls through to `Retryable`, so dropping one turns a
    // permanent failure into one the retry machinery would chase forever.
    //
    // Asserted per-code rather than through a real failing open, because a real
    // open cannot be made to produce each of these on demand.
    use super::{OpenFailure, classify};
    use windows_sys::Win32::Foundation::{
        ERROR_DIRECTORY, ERROR_FILE_NOT_FOUND, ERROR_INVALID_FUNCTION, ERROR_INVALID_NAME,
        ERROR_NOT_SUPPORTED, ERROR_PATH_NOT_FOUND,
    };

    let cases: [(u32, OpenFailure); 6] = [
        (ERROR_FILE_NOT_FOUND, OpenFailure::NotFound),
        (ERROR_PATH_NOT_FOUND, OpenFailure::NotFound),
        (ERROR_DIRECTORY, OpenFailure::NotADirectory),
        (ERROR_INVALID_FUNCTION, OpenFailure::Unsupported),
        (ERROR_NOT_SUPPORTED, OpenFailure::Unsupported),
        (ERROR_INVALID_NAME, OpenFailure::InvalidPath),
    ];

    for (code, expected) in cases {
        let error = std::io::Error::from_raw_os_error(code as i32);
        assert_eq!(
            classify(&error),
            expected,
            "error {code} must classify as {expected:?}"
        );
    }
}

#[test]
fn an_unrecognised_code_is_retryable() {
    // The fallback arm, asserted beside the named ones so a classifier that
    // returned `Retryable` for everything could not pass the group above.
    let error = std::io::Error::from_raw_os_error(0x0000_DEAD);
    assert_eq!(super::classify(&error), OpenFailure::Retryable);
}

#[test]
fn an_error_with_no_os_code_is_retryable() {
    // The `raw_os_error()` guard: a synthesised error carries no code, and
    // guessing a permanent classification from one would strand a watch.
    let error = std::io::Error::other("no OS code behind this one");
    assert_eq!(super::classify(&error), OpenFailure::Retryable);
}

#[test]
fn volume_identity_reports_the_descriptive_fields_it_was_built_with() {
    // The accessors are plain `pub fn`, but their only test was behind
    // `#[cfg(feature = "test-util")]` -- so with default features they shipped
    // untested, and replacing either body with `String::new()` or a constant
    // survived. This test uses the crate-internal `synthetic` seam so it runs
    // in every configuration.
    let identity = VolumeIdentity::synthetic(0x1234, "ReFS", "Data");
    assert_eq!(identity.filesystem_name(), "ReFS");
    assert_eq!(identity.volume_label(), "Data");
}

#[test]
fn volume_identitys_two_descriptive_fields_do_not_alias() {
    // Distinct values, so an accessor returning the *other* field -- which no
    // equality-only test could notice, since identity compares by serial --
    // fails here.
    let identity = VolumeIdentity::synthetic(7, "NTFS", "System");
    assert_ne!(identity.filesystem_name(), identity.volume_label());
    assert_eq!(identity.filesystem_name(), "NTFS");
    assert_eq!(identity.volume_label(), "System");
}

#[test]
fn volume_identity_accepts_empty_descriptive_fields() {
    // An unlabelled volume is ordinary, and the empty string must come back as
    // itself rather than being confused with "not read".
    let identity = VolumeIdentity::synthetic(1, "", "");
    assert_eq!(identity.filesystem_name(), "");
    assert_eq!(identity.volume_label(), "");
}

#[test]
fn case_sensitivity_is_read_from_the_directory_rather_than_assumed() {
    // `is_case_sensitive_dir` and the `is_case_sensitive` accessor both survived
    // being replaced with a constant, and every operator in the flag test
    // survived too. All of it was covered only by directories that happen to be
    // case-insensitive, where a hard-coded `false` is indistinguishable from a
    // real read.
    //
    // The fix is a directory of each kind. `fsutil file setCaseSensitiveInfo`
    // needs no elevation on NTFS, and if it is unavailable this test says so
    // rather than passing quietly on half the evidence.
    let insensitive = TempDir::new("case-insensitive");
    let handle = DirectoryHandle::open(insensitive.path()).expect("opens");
    assert!(
        !handle.is_case_sensitive(),
        "a directory with the flag clear must report insensitive"
    );

    let sensitive = TempDir::new("case-sensitive");
    let marked = std::process::Command::new("fsutil.exe")
        .args([
            "file",
            "setCaseSensitiveInfo",
            &sensitive.path().display().to_string(),
            "enable",
        ])
        .output();

    let enabled = matches!(&marked, Ok(output) if output.status.success());
    assert!(
        enabled,
        "could not mark a directory case-sensitive, so the positive half of \
         this contract went untested: {marked:?}"
    );

    let handle = DirectoryHandle::open(sensitive.path()).expect("opens");
    assert!(
        handle.is_case_sensitive(),
        "a directory with FILE_CS_FLAG_CASE_SENSITIVE_DIR set must report \
         sensitive -- this is the half a hard-coded `false` passes"
    );
}

// --- D-85: the caller's path reaches Win32 verbatim ---
//
// These exist to fail if this crate ever "helpfully" prepends `\\?\`. That
// prefix is a different path *parsing mode*, not a longer-path switch, so
// adopting it on a caller's behalf silently changes what their path means --
// and it does so on paths that have nothing to do with `MAX_PATH`, which is
// what makes the change easy to justify to oneself and hard to notice.
//
// Each asserts on the resolved *identity*, not merely that something opened: a
// path that resolves to the wrong directory is the failure mode worth catching,
// and `is_ok()` would not see it.
//
// Deliberately absent: a test that a path longer than `MAX_PATH` opens. That
// depends on the host executable's `longPathAware` manifest, which a Rust test
// binary does not have, so such a test would pin the harness rather than this
// crate. `opens_the_current_directory_by_relative_path` above is part of this
// set -- a relative path cannot survive the prefix either.

/// The identity of a directory opened by its plain absolute path, to compare a
/// differently-spelled route to the same directory against.
fn identity_of(path: &Path) -> super::DirectoryId {
    DirectoryHandle::open(path)
        .expect("the plain absolute path must open")
        .identity()
}

#[test]
fn forward_slashes_resolve_to_the_same_directory() {
    // Win32 translates `/` to `\` during ordinary parsing. Under `\\?\` it does
    // not, and this path would fail with `ERROR_FILE_NOT_FOUND`.
    let dir = TempDir::new("verbatim-slashes");
    let expected = identity_of(dir.path());

    let with_slashes = dir.path().to_string_lossy().replace('\\', "/");
    let handle = DirectoryHandle::open(Path::new(&with_slashes))
        .expect("a forward-slash spelling must open, because Win32 translates it");
    assert_eq!(
        handle.identity(),
        expected,
        "the forward-slash spelling must reach the same directory"
    );

    drop(handle);
    dir.cleanup();
}

#[test]
fn a_dot_component_resolves_to_the_same_directory() {
    // Win32 resolves `.` during ordinary parsing. Under `\\?\` it is a literal
    // component and this fails with `ERROR_INVALID_NAME`.
    let dir = TempDir::new("verbatim-dot");
    let expected = identity_of(dir.path());

    let with_dot = dir.path().join(".");
    let handle = DirectoryHandle::open(&with_dot)
        .expect("a `.` component must open, because Win32 resolves it");
    assert_eq!(
        handle.identity(),
        expected,
        "`.` must resolve to the directory itself"
    );

    drop(handle);
    dir.cleanup();
}

#[test]
fn a_dot_dot_component_resolves_to_the_parent() {
    // As above for `..`, and this one proves the component was *resolved*
    // rather than merely tolerated: the handle must land on the parent.
    let dir = TempDir::new("verbatim-dotdot");
    let child = dir.path().join("child");
    std::fs::create_dir(&child).expect("create the child directory");
    let expected = identity_of(dir.path());

    let up_again = child.join("..");
    let handle = DirectoryHandle::open(&up_again)
        .expect("a `..` component must open, because Win32 resolves it");
    assert_eq!(
        handle.identity(),
        expected,
        "`..` must resolve back to the parent, not open the child"
    );

    drop(handle);
    dir.cleanup();
}

#[test]
fn a_caller_supplied_verbatim_prefix_is_forwarded_and_honoured() {
    // The other direction of D-85, and the route a caller takes when they want
    // extended-length or verbatim semantics: their own `\\?\` path must arrive
    // intact. This is what makes "we never add the prefix" a complete contract
    // rather than a refusal.
    let dir = TempDir::new("verbatim-prefixed");
    let expected = identity_of(dir.path());

    let prefixed = format!(r"\\?\{}", dir.path().display());
    let handle = DirectoryHandle::open(Path::new(&prefixed))
        .expect("a caller's own `\\?\\` path must be forwarded unchanged and open");
    assert_eq!(
        handle.identity(),
        expected,
        "the verbatim spelling must reach the same directory"
    );

    drop(handle);
    dir.cleanup();
}

#[test]
fn canonical_path_grows_its_buffer_when_the_path_does_not_fit() {
    // `canonical_path` sizes a 512-unit buffer and retries on the documented
    // two-call convention. Reaching that retry needs a resolved path of 512+
    // units, which needs a directory deeper than `MAX_PATH` -- and D-85's
    // pass-through is what makes that openable without the host executable
    // carrying a `longPathAware` manifest: the caller's own `\\?\` path is
    // forwarded unchanged, and Win32 honours it.
    //
    // This corrects the note that opened M15.10: the retry is reachable through
    // this crate's own API, so it needs no junction/reparse-point fixture and no
    // spawned `mklink`.
    use std::os::windows::ffi::OsStrExt;

    let dir = TempDir::new("canonical-long");
    let mut deep = dir.path().to_path_buf();
    while format!(r"\\?\{}", deep.display()).len() < 560 {
        deep.push("segment-0123456789abcdef");
    }
    // Created through an explicitly prefixed string, so the fixture does not
    // depend on any library prefixing on its behalf.
    let prefixed = format!(r"\\?\{}", deep.display());
    std::fs::create_dir_all(&prefixed).expect("create the deep directory");

    let handle = DirectoryHandle::open(Path::new(&prefixed))
        .expect("a caller's own `\\?\\` path opens past MAX_PATH (D-85)");
    let reported = handle.canonical_path().expect("canonical path");

    let units = reported.as_os_str().encode_wide().count();
    assert!(
        units > 512,
        "the fixture must actually overflow the first buffer, got {units} units"
    );
    assert_eq!(
        reported,
        std::fs::canonicalize(&prefixed).expect("std canonicalize"),
        "the grown buffer must carry the whole path, not a truncated one"
    );

    drop(handle);
    let _ = std::fs::remove_dir_all(&prefixed);
    dir.cleanup();
}
/// A directory whose `\\?\` spelling is exactly `target` UTF-16 units, built by
/// padding the final component. Components stay well under the 255-unit limit.
///
/// **Built from the base's *canonical* spelling, not the one handed in.** A
/// machine whose temp directory contains an 8.3 short name -- `C:\Users\RUNNER~1\...`
/// on a GitHub-hosted runner -- canonicalizes it to the long form, so a fixture
/// measured against the short spelling is reported *longer* than it was built to
/// be. Measured: `RUNNER~1` expands to `runneradmin`, three units wider, and the
/// length assertions failed on CI with `left: 511, right: 508` while passing on
/// a developer machine whose temp path is already canonical.
fn deep_dir_of_prefixed_len(base: &Path, target: usize) -> PathBuf {
    let canonical = std::fs::canonicalize(base).expect("the base directory must exist");
    // `canonicalize` already returns the `\\?\` form on Windows, and the caller
    // re-adds that prefix when it measures, so strip it here rather than
    // counting it twice.
    let mut path = match canonical.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        Some(stripped) => PathBuf::from(stripped),
        None => canonical.clone(),
    };
    loop {
        let current = format!(r"\\?\{}", path.display()).len();
        assert!(
            current + 2 <= target,
            "the base path is already too long to hit {target}"
        );
        let remaining = target - current - 1;
        if remaining <= 200 {
            path.push("x".repeat(remaining));
            return path;
        }
        path.push("x".repeat(200));
    }
}

#[test]
fn canonical_path_is_exact_on_both_sides_of_its_first_buffer() {
    // The first buffer is 512 units and the two-call convention's success test
    // is `written < buffer.len()`, so 511 units is the last length that fits in
    // one call and 512 is the first that needs the regrow. Walking both sides
    // pins that boundary: an off-by-one in either the success test or the
    // regrow would leave one of these lengths truncated or looping.
    use std::os::windows::ffi::OsStrExt;

    let dir = TempDir::new("canon-bound");
    for target in 508..=516 {
        let deep = deep_dir_of_prefixed_len(dir.path(), target);
        let prefixed = format!(r"\\?\{}", deep.display());
        assert_eq!(prefixed.len(), target, "the fixture must be exactly sized");
        std::fs::create_dir_all(&prefixed).expect("create the sized directory");

        let handle = DirectoryHandle::open(Path::new(&prefixed)).expect("open the sized directory");
        let reported = handle.canonical_path().expect("canonical path");
        assert_eq!(
            reported.as_os_str().encode_wide().count(),
            target,
            "a {target}-unit path must be reported whole"
        );
        assert_eq!(
            reported,
            std::fs::canonicalize(&prefixed).expect("std canonicalize"),
            "a {target}-unit path must be reported correctly"
        );

        drop(handle);
        let _ = std::fs::remove_dir_all(&prefixed);
    }
    dir.cleanup();
}
