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

// --- M11: measuring `OpenFileById` against a live directory handle
// empirically, per D-52's precedent of measuring rather than assuming Win32
// behavior. (`ReOpenFile` was tried first and consistently failed with
// `ERROR_ACCESS_DENIED` against a directory on an ordinary, unprivileged
// process -- it needs `SeBackupPrivilege` *enabled*, which
// `FILE_FLAG_BACKUP_SEMANTICS` alone does not grant.) ---

#[test]
fn reopen_by_id_preserves_identity_while_the_original_stays_open() {
    let dir = TempDir::new("reopen-same-identity");
    let original = DirectoryHandle::open(dir.path()).expect("open");
    let original_identity = original.identity();

    // SAFETY: `original`'s handle is live for the whole body of this test.
    let hint = unsafe { std::os::windows::io::BorrowedHandle::borrow_raw(original.as_raw()) };
    let reopened = DirectoryHandle::reopen_by_id(hint, original_identity.file_reference())
        .expect("OpenFileById against a live handle's own file reference");

    assert_eq!(
        reopened.identity(),
        original_identity,
        "OpenFileById must reopen the same object its file reference already names"
    );
    // The original handle is untouched by reopening it: a second, independent
    // syscall against it still agrees.
    assert_eq!(original.identity(), original_identity);

    drop(reopened);
    drop(original);
    dir.cleanup();
}

#[test]
fn reopen_by_id_survives_the_directory_being_deleted_from_under_it() {
    // Measured, not assumed (D-52): a directory handle opened with
    // `FILE_SHARE_DELETE` (this crate's own share mode) keeps its underlying
    // object alive -- "delete pending" -- for as long as the handle stays
    // open, even after every directory-entry reference to it is gone. This is
    // exactly the state `WatcherInner::reopen_via_existing_handle` reopens
    // against, so this measures precisely that, not a hypothetical.
    let dir = TempDir::new("reopen-deleted");
    let original = DirectoryHandle::open(dir.path()).expect("open");
    let original_identity = original.identity();

    std::fs::remove_dir(dir.path()).expect("unlink the directory while the handle is still open");

    // SAFETY: `original`'s handle is still open -- only its directory entry
    // was removed, not the handle itself -- and serves only as the volume
    // hint here, not as the object being reopened.
    let hint = unsafe { std::os::windows::io::BorrowedHandle::borrow_raw(original.as_raw()) };
    let reopened = DirectoryHandle::reopen_by_id(hint, original_identity.file_reference())
        .expect("OpenFileById against a delete-pending object's own file reference");

    assert_eq!(
        reopened.identity(),
        original_identity,
        "OpenFileById reopens the same (delete-pending) object its file reference names, \
         never a different one that happens to appear at the original path later"
    );

    drop(reopened);
    drop(original);
    // Nothing left on disk to clean up: the directory was already unlinked.
}

#[test]
fn reopen_by_id_ignores_a_new_directory_recreated_at_the_same_path() {
    // The critical measurement M11.2's design depends on: once a *new*
    // directory exists at the original path, `OpenFileById` against the old
    // file reference must keep reopening the *old* (delete-pending) object,
    // never silently pick up the new one that happens to share the path.
    let dir = TempDir::new("reopen-recreated");
    let original = DirectoryHandle::open(dir.path()).expect("open");
    let original_identity = original.identity();

    std::fs::remove_dir(dir.path()).expect("unlink the directory while the handle is still open");
    std::fs::create_dir(dir.path()).expect("recreate a new directory at the same path");
    let fresh_identity = DirectoryHandle::open(dir.path())
        .expect("open the recreated directory")
        .identity();
    assert_ne!(
        original_identity, fresh_identity,
        "a recreated directory must have a genuinely different identity for this test to mean anything"
    );

    // SAFETY: `original`'s handle is still open throughout, used only as the
    // volume hint.
    let hint = unsafe { std::os::windows::io::BorrowedHandle::borrow_raw(original.as_raw()) };
    let reopened = DirectoryHandle::reopen_by_id(hint, original_identity.file_reference())
        .expect("OpenFileById against a live file reference");

    assert_eq!(
        reopened.identity(),
        original_identity,
        "OpenFileById must never silently switch to a different object recreated at the same path"
    );

    drop(reopened);
    drop(original);
    dir.cleanup();
}

#[test]
fn reopen_by_id_follows_the_directory_if_it_is_renamed_and_canonical_path_detects_it() {
    // The other side of `OpenFileById`'s path independence (M11.2): unlike a
    // recreated-at-the-same-path object (a *different* file the reopen must
    // ignore), a renamed *same* object is exactly what OpenFileById is
    // supposed to keep following -- but `WatcherInner::reopen_via_existing_handle`
    // must still notice the path no longer matches what this watcher was
    // subscribed to, via `canonical_path`, rather than silently watching the
    // directory at its new location under the old subscription.
    let parent = TempDir::new("reopen-rename-parent");
    let original_path = parent.path().join("original");
    std::fs::create_dir(&original_path).expect("create the directory to be renamed");
    let original = DirectoryHandle::open(&original_path).expect("open");
    let original_identity = original.identity();
    let path_before = original
        .canonical_path()
        .expect("query the path before the rename");

    let renamed_path = parent.path().join("renamed");
    std::fs::rename(&original_path, &renamed_path).expect("rename while the handle is open");

    // SAFETY: `original`'s handle is still open throughout, used only as the
    // volume hint.
    let hint = unsafe { std::os::windows::io::BorrowedHandle::borrow_raw(original.as_raw()) };
    let reopened = DirectoryHandle::reopen_by_id(hint, original_identity.file_reference())
        .expect("OpenFileById against a live file reference");

    assert_eq!(
        reopened.identity(),
        original_identity,
        "a rename does not change the object's own identity"
    );
    let path_after = reopened
        .canonical_path()
        .expect("query the path after the rename");
    assert_ne!(
        path_before, path_after,
        "OpenFileById follows the object to its new location, so the canonical path must \
         change -- this is exactly what `reopen_via_existing_handle` must detect and refuse"
    );

    drop(reopened);
    drop(original);
    parent.cleanup();
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
