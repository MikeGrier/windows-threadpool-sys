// Copyright (c) 2026 Mike Grier
//! Invalid and inaccessible paths, a fully qualified `\\?\` path far beyond
//! `MAX_PATH`, a relative path fixed when the request is built, and native
//! WTF-16 names -- including one with an unpaired surrogate, which is exactly
//! the kind of filesystem content this crate's native-width name storage
//! exists to survive without loss.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use windows_file_enumeration_sys::{EnumerationError, EnumerationRequest, Session};

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
fn an_ordinary_path_beyond_max_path_is_accepted_and_its_open_is_the_hosts() {
    // The request keeps the path as given. Whether `CreateFileW` takes an
    // unprefixed path past `MAX_PATH` depends on the host's long-path manifest
    // and the machine's policy, so either outcome is a clean report.
    let long_name = "x".repeat(300);
    let path = PathBuf::from(r"C:\").join(long_name);
    let request = EnumerationRequest::for_path(&path).expect("accepted as given");
    assert_eq!(request.win32_path().len(), path.as_os_str().len());

    let (session, receiver) = Session::new(8, 8).expect("room");
    let handle = session.try_begin(request).expect("room");
    let enumeration = handle.id();
    handle.detach();

    let (entries, outcome) = drain_to_terminal(&receiver, enumeration);
    assert!(entries.is_empty());
    let failure = outcome.failure().expect("no such directory");
    assert!(
        matches!(failure, EnumerationError::DirectoryOpen(_)),
        "{failure:?}"
    );
}

#[test]
fn an_empty_path_is_reported_by_the_open() {
    let request = EnumerationRequest::for_path(PathBuf::new().as_path()).expect("accepted");
    let (session, receiver) = Session::new(8, 8).expect("room");
    let handle = session.try_begin(request).expect("room");
    let enumeration = handle.id();
    handle.detach();

    let (entries, outcome) = drain_to_terminal(&receiver, enumeration);
    assert!(entries.is_empty());
    let failure = outcome.failure().expect("an empty path names nothing");
    assert!(
        matches!(failure, EnumerationError::DirectoryOpen(_)),
        "{failure:?}"
    );
}

const CHILD_MARKER: &str = "WFE_RELATIVE_CHILD";
const CHILD_TEST: &str =
    "paths::a_relative_path_names_the_directory_it_was_built_in_after_the_directory_changes";

// The process current directory is shared by every test in the process, so the
// test that changes it runs in a child process of its own, as the request
// crate's do. It is what tells the path the worker opens from the path as
// given: the two name the same directory until the current directory moves.
#[test]
fn a_relative_path_names_the_directory_it_was_built_in_after_the_directory_changes() {
    const NAME: &str = "wfe-relative-subject";

    if std::env::var_os(CHILD_MARKER).is_some() {
        let built_in = std::env::current_dir().expect("the current directory");
        let request = EnumerationRequest::for_path(NAME.as_ref()).expect("accepted");
        assert_eq!(request.path().to_string_lossy(), NAME);
        assert_eq!(
            request.win32_path().to_string_lossy(),
            built_in.join(NAME).to_string_lossy(),
            "grafted onto the current directory when the request was built"
        );

        let elsewhere = std::env::var_os("SystemRoot").expect("Windows names its directory");
        std::env::set_current_dir(elsewhere).expect("change the current directory");

        let (session, receiver) = Session::new(8, 8).expect("room");
        let handle = session.try_begin(request).expect("room");
        let enumeration = handle.id();
        handle.detach();

        let (entries, outcome) = drain_to_terminal(&receiver, enumeration);
        assert!(outcome.is_completed(), "{outcome:?}");
        assert_eq!(entry_names(&entries), ["here.txt"]);
        return;
    }

    let scratch = Scratch::empty();
    let subject = scratch.child(NAME);
    std::fs::create_dir(&subject).expect("the subject directory");
    std::fs::write(subject.join("here.txt"), b"").expect("a file in it");

    let output = std::process::Command::new(std::env::current_exe().expect("this test binary"))
        .arg("--exact")
        .arg(CHILD_TEST)
        .arg("--nocapture")
        .env(CHILD_MARKER, "1")
        .current_dir(scratch.path())
        .output()
        .expect("run this test in a child process");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("1 passed"),
        "the child did not pass: {}\n{stdout}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
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
