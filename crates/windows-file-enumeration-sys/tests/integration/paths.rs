// Copyright (c) 2026 Mike Grier
//! Invalid and inaccessible paths, a fully qualified `\\?\` path far beyond
//! `MAX_PATH`, and native WTF-16 names -- including one with an unpaired
//! surrogate, which is exactly the kind of filesystem content this crate's
//! native-width name storage exists to survive without loss.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use windows_file_enumeration_sys::{EnumerationError, EnumerationRequest, RequestFailure, Session};

use crate::support::{Scratch, drain_to_terminal, entry_names};

#[test]
fn a_missing_directory_reports_a_directory_open_failure() {
    let scratch = Scratch::empty();
    let (session, receiver) = Session::new(8, 8).expect("room");
    let request =
        EnumerationRequest::for_path(&scratch.child("does-not-exist")).expect("resolvable");
    let handle = session.try_begin(request).expect("room");
    let enumeration = handle.id();
    handle.detach();

    let (entries, outcome) = drain_to_terminal(&receiver, enumeration);
    assert!(entries.is_empty());
    let failure = outcome.failure().expect("a failure");
    assert!(
        matches!(failure, EnumerationError::DirectoryOpen(_)),
        "{failure:?}"
    );
}

#[test]
fn a_file_named_as_a_directory_reports_a_directory_open_failure() {
    let scratch = Scratch::with_files(&["plain.txt"]);
    let (session, receiver) = Session::new(8, 8).expect("room");
    let request = EnumerationRequest::for_path(&scratch.child("plain.txt")).expect("resolvable");
    let handle = session.try_begin(request).expect("room");
    let enumeration = handle.id();
    handle.detach();

    let (entries, outcome) = drain_to_terminal(&receiver, enumeration);
    assert!(entries.is_empty());
    let failure = outcome.failure().expect("a failure");
    assert!(
        matches!(failure, EnumerationError::DirectoryOpen(_)),
        "{failure:?}"
    );
}

#[test]
fn an_inaccessible_system_directory_is_reported_rather_than_panicking() {
    // A directory present on every ordinary Windows installation whose ACL
    // denies ordinary users. Whether *this* run has the rights to it depends
    // on how the test host is configured -- the property under test is that
    // either outcome is reported cleanly, never a panic or a hang.
    let path = PathBuf::from(r"C:\System Volume Information");
    let (session, receiver) = Session::new(8, 8).expect("room");
    let request = EnumerationRequest::for_path(&path).expect("resolvable");
    let handle = session.try_begin(request).expect("room");
    let enumeration = handle.id();
    handle.detach();

    let (_entries, outcome) = drain_to_terminal(&receiver, enumeration);
    match outcome.failure() {
        Some(failure) => assert!(
            matches!(failure, EnumerationError::DirectoryOpen(_)),
            "{failure:?}"
        ),
        None => assert!(outcome.is_completed(), "{outcome:?}"),
    }
}

#[test]
fn a_verbatim_path_far_beyond_max_path_still_enumerates() {
    // Nested nine levels deep at 32 characters each, well past the 260-byte
    // ordinary limit once joined -- reachable only through the `\\?\` form
    // this crate accepts verbatim rather than silently truncating.
    let scratch = Scratch::empty();
    let mut deep = scratch.path().to_path_buf();
    for segment in 0..9 {
        deep.push(format!("segment-{segment:02}-abcdefghijklmnopqr"));
    }
    std::fs::create_dir_all(&deep).expect("a deeply nested scratch directory");
    std::fs::write(deep.join("leaf.txt"), b"").expect("a leaf file");
    assert!(
        deep.as_os_str().len() > 260,
        "the fixture must actually exceed MAX_PATH: {}",
        deep.display()
    );

    let verbatim = format!(r"\\?\{}", deep.display());
    let request = EnumerationRequest::for_path(verbatim.as_ref()).expect("resolvable");

    let (session, receiver) = Session::new(8, 8).expect("room");
    let handle = session.try_begin(request).expect("room");
    let enumeration = handle.id();
    handle.detach();

    let (entries, outcome) = drain_to_terminal(&receiver, enumeration);
    assert!(outcome.is_completed(), "{outcome:?}");
    assert_eq!(entry_names(&entries), ["leaf.txt"]);
}

#[test]
fn an_ordinary_path_beyond_max_path_is_rejected_rather_than_silently_truncated() {
    let long_name = "x".repeat(300);
    let path = PathBuf::from(r"C:\").join(long_name);
    let error = EnumerationRequest::for_path(&path).expect_err("longer than MAX_PATH");
    assert_eq!(error.failure(), RequestFailure::PathTooLong);
}

#[test]
fn a_name_with_an_unpaired_surrogate_survives_the_round_trip() {
    // 0xD800 alone -- a high surrogate with no low surrogate to pair with --
    // is exactly the kind of native content native-width WTF-16 storage
    // exists to carry without loss. `OsString` on Windows is itself
    // WTF-8-equivalent, so this is not doing anything exotic: it is asking
    // the filesystem to hold what a real, if unusual, filesystem can hold.
    let scratch = Scratch::empty();
    let mut units: Vec<u16> = "ill-formed-".encode_utf16().collect();
    units.push(0xD800);
    units.extend(".txt".encode_utf16());
    let name = OsString::from_wide(&units);
    let path = scratch.path().join(&name);
    if std::fs::write(&path, b"").is_err() {
        // Some filesystems or redirectors genuinely cannot hold this; that is
        // an environmental fact this crate cannot change, not something for
        // this test to paper over.
        eprintln!("skipping: this filesystem rejected an unpaired-surrogate name");
        return;
    }

    let (session, receiver) = Session::new(8, 8).expect("room");
    let request = EnumerationRequest::for_path(scratch.path()).expect("resolvable");
    let handle = session.try_begin(request).expect("room");
    let enumeration = handle.id();
    handle.detach();

    let (entries, outcome) = drain_to_terminal(&receiver, enumeration);
    assert!(outcome.is_completed(), "{outcome:?}");
    assert_eq!(entries.len(), 1, "{entries:?}");
    let windows_file_enumeration_sys::Completion::Entry { entry, .. } = &entries[0] else {
        unreachable!();
    };
    assert_eq!(
        entry.name().as_units(),
        units.as_slice(),
        "the unpaired surrogate must survive verbatim, not be replaced or dropped"
    );
}
