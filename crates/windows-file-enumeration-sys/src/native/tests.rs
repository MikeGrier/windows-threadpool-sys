// Copyright (c) 2026 Mike Grier
//! Tests for the Win32 layer, against real directories.

use super::*;
use crate::request::MINIMUM_BUFFER_CAPACITY;
use crate::scratch::Scratch;
use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_PATH_NOT_FOUND};
use wtf_string::Wtf16String;

fn token() -> ImpersonationToken {
    ImpersonationToken::capture().expect("the calling thread has a context")
}

fn wide(path: &std::path::Path) -> Wtf16String {
    Wtf16String::from_os_str(path.as_os_str())
}

fn buffer() -> AlignedBuf {
    AlignedBuf::zeroed(MINIMUM_BUFFER_CAPACITY, RECORD_ALIGNMENT).expect("allocation")
}

fn open(path: &std::path::Path) -> Result<SharedHandle, EnumerationError> {
    let anchored =
        AnchoredPath::new(wide(path)).expect("an absolute path has no current directory to read");
    open_directory(&anchored, &token())
}

#[test]
fn an_ordinary_directory_opens() {
    let scratch = Scratch::empty();
    open(scratch.path()).expect("a directory this test just created");
}

#[test]
fn a_missing_directory_reports_an_open_failure() {
    // Not exhaustion: the same code means something else entirely from a
    // refill, which is why the classification is phase-specific.
    let scratch = Scratch::empty();
    let error = open(&scratch.child("no-such-directory")).expect_err("missing");
    let code = error.code().expect("a raw code").code();
    assert!(
        matches!(error, EnumerationError::DirectoryOpen(_)),
        "{error:?}"
    );
    assert!(
        code == ERROR_FILE_NOT_FOUND || code == ERROR_PATH_NOT_FOUND,
        "unexpected code {code}"
    );
}

#[test]
fn a_file_is_rejected_at_the_open() {
    // FILE_LIST_DIRECTORY is the same bit as FILE_READ_DATA, so the open
    // itself succeeds on a file. Directory-ness is established here rather than
    // being inferred from a refill code that cannot distinguish "you named a
    // file" from "this filesystem cannot do extended directory information".
    let scratch = Scratch::with_files(&["plain.txt"]);
    let error = open(&scratch.child("plain.txt")).expect_err("not a directory");
    assert!(
        matches!(error, EnumerationError::DirectoryOpen(_)),
        "{error:?}"
    );
    assert_eq!(error.code().expect("a raw code").code(), ERROR_DIRECTORY);
}

#[test]
fn an_empty_directory_still_returns_its_dot_entries() {
    // An empty *subdirectory* is not an empty listing: it holds . and ...
    // Only a directory with no records at all -- an empty volume root, say --
    // reaches the first-query-empty form, which is why that rule is stated in
    // terms of the query rather than in terms of "empty".
    let scratch = Scratch::empty();
    let directory = open(scratch.path()).expect("open");
    let mut buffer = buffer();

    let first = refill(&directory, &mut buffer, Refill::First);
    assert!(matches!(first, RefillOutcome::Batch), "{first:?}");

    let second = refill(&directory, &mut buffer, Refill::Next);
    assert!(matches!(second, RefillOutcome::Exhausted), "{second:?}");
}

#[test]
fn a_non_empty_directory_returns_a_batch_then_exhausts() {
    let scratch = Scratch::with_files(&["a.txt", "b.txt", "c.txt"]);
    let directory = open(scratch.path()).expect("open");
    let mut buffer = buffer();

    let first = refill(&directory, &mut buffer, Refill::First);
    assert!(matches!(first, RefillOutcome::Batch), "{first:?}");

    // A small buffer may need several queries; the run must end in exhaustion
    // rather than a failure.
    for _ in 0..64 {
        match refill(&directory, &mut buffer, Refill::Next) {
            RefillOutcome::Batch => continue,
            RefillOutcome::Exhausted => return,
            RefillOutcome::Failed(error) => panic!("unexpected failure: {error:?}"),
        }
    }
    panic!("the directory never reported exhaustion");
}

#[test]
fn a_restart_refill_reads_the_directory_again() {
    let scratch = Scratch::with_files(&["a.txt"]);
    let directory = open(scratch.path()).expect("open");
    let mut buffer = buffer();

    assert!(matches!(
        refill(&directory, &mut buffer, Refill::First),
        RefillOutcome::Batch
    ));
    // Restarting rewinds, so the same handle yields a batch again rather than
    // continuing from where the last query stopped.
    assert!(matches!(
        refill(&directory, &mut buffer, Refill::First),
        RefillOutcome::Batch
    ));
}

#[test]
fn a_volume_serial_is_obtainable_for_an_ordinary_directory() {
    let scratch = Scratch::empty();
    let directory = open(scratch.path()).expect("open");
    let serial = volume_serial(&directory).expect("a local volume reports its serial");
    assert_ne!(serial, 0, "a real volume has a non-zero serial");
}

#[test]
fn exhaustion_is_not_a_failure_on_either_query() {
    assert!(
        classify_refill_failure(
            Win32Error::from_code(ERROR_NO_MORE_FILES),
            Refill::First,
            1024
        )
        .is_none()
    );
    assert!(
        classify_refill_failure(
            Win32Error::from_code(ERROR_NO_MORE_FILES),
            Refill::Next,
            1024
        )
        .is_none()
    );
}

#[test]
fn an_empty_first_query_is_exhaustion_but_a_later_one_is_a_failure() {
    // The whole point of tracking the phase: the same code means two different
    // things depending on which query reported it.
    assert!(
        classify_refill_failure(
            Win32Error::from_code(ERROR_FILE_NOT_FOUND),
            Refill::First,
            1024
        )
        .is_none()
    );
    let late = classify_refill_failure(
        Win32Error::from_code(ERROR_FILE_NOT_FOUND),
        Refill::Next,
        1024,
    )
    .expect("a failure");
    assert!(
        matches!(late, EnumerationError::DirectoryQuery(_)),
        "{late:?}"
    );
}

#[test]
fn unsupported_class_codes_are_classified_as_a_capability_failure() {
    for code in [
        ERROR_INVALID_FUNCTION,
        ERROR_NOT_SUPPORTED,
        ERROR_INVALID_PARAMETER,
    ] {
        let error = classify_refill_failure(Win32Error::from_code(code), Refill::First, 1024)
            .expect("a failure");
        assert!(
            matches!(error, EnumerationError::UnsupportedExtendedDirectoryInfo(_)),
            "{code} became {error:?}"
        );
    }
}

#[test]
fn oversize_record_codes_report_the_capacity_they_did_not_fit() {
    for code in [ERROR_MORE_DATA, ERROR_INSUFFICIENT_BUFFER, ERROR_BAD_LENGTH] {
        let error = classify_refill_failure(Win32Error::from_code(code), Refill::Next, 4096)
            .expect("a failure");
        match error {
            EnumerationError::RecordTooLarge {
                buffer_capacity, ..
            } => assert_eq!(buffer_capacity, 4096),
            other => panic!("{code} became {other:?}"),
        }
    }
}

#[test]
fn an_unrecognised_code_is_an_ordinary_query_failure() {
    let error = classify_refill_failure(
        Win32Error::from_code(ERROR_ACCESS_DENIED),
        Refill::Next,
        1024,
    )
    .expect("a failure");
    assert!(
        matches!(error, EnumerationError::DirectoryQuery(_)),
        "{error:?}"
    );
}

// --- The directory's calls are the request crate's entries (SAFE-2) ----------------------------

#[test]
fn the_open_asks_for_listing_with_every_share_and_backup_semantics_and_nothing_more() {
    // The constraints M21.3 recorded, on the request the open performs: listing access and not
    // `GENERIC_READ`; every share, so enumerating stops no one; an existing directory; and backup
    // semantics, which a directory handle needs at all. The flags are exactly that one, so the
    // handle is not overlapped.
    // A relative path, so the Win32 form is the anchored one and differs from the given one: an
    // open that anchored it a second time, or opened the given path, would not match.
    let anchored = AnchoredPath::new(wide(std::path::Path::new("a-relative-child")))
        .expect("the current directory is readable");
    let request = open_request(&anchored);

    assert_eq!(request.desired_access(), FILE_LIST_DIRECTORY);
    assert_eq!(
        request.share_mode(),
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE
    );
    assert_eq!(request.creation_disposition(), OPEN_EXISTING);
    assert_eq!(request.flags_and_attributes(), FILE_FLAG_BACKUP_SEMANTICS);
    assert!(request.security().is_none() && request.template().is_none());
    assert_eq!(request.path(), anchored.given());
    assert_eq!(request.win32_path(), anchored.win32());
    assert_ne!(request.win32_path(), request.path());
}

#[test]
fn an_open_directory_shares_delete_so_it_does_not_stop_a_rename() {
    let scratch = Scratch::with_files(&["inside.txt"]);
    let held = scratch.child("held");
    std::fs::create_dir(&held).expect("a subdirectory");
    let directory = open(&held).expect("open");

    std::fs::rename(&held, scratch.child("renamed")).expect("a held directory can be renamed");
    assert!(
        volume_serial(&directory).is_ok(),
        "the handle follows the rename"
    );
}

#[test]
fn a_refill_query_is_the_phases_class_on_the_handle_it_was_given() {
    let scratch = Scratch::empty();
    let directory = open(scratch.path()).expect("open");

    let first = refill_query(&directory, Refill::First);
    let next = refill_query(&directory, Refill::Next);

    assert_eq!(
        first.class(),
        FileInformationClass::ID_EXTD_DIRECTORY_RESTART
    );
    assert_eq!(next.class(), FileInformationClass::ID_EXTD_DIRECTORY);
    assert!(first.handle().same_handle(&directory));
    assert!(next.handle().same_handle(&directory));
}

#[test]
fn a_whole_read_leaves_the_directory_with_no_other_holder() {
    // `try_into_owned` succeeds only for the last holder, so a clone kept by the open, the
    // directory check, the serial read or any refill would show here. Together with the entries
    // holding the very handle they are given, this is what "makes no handle beyond its open" is.
    let scratch = Scratch::with_files(&["a.txt", "b.txt", "c.txt"]);
    let directory = open(scratch.path()).expect("open");
    let mut buffer = buffer();
    volume_serial(&directory).expect("a serial");

    assert!(matches!(
        refill(&directory, &mut buffer, Refill::First),
        RefillOutcome::Batch
    ));
    while let RefillOutcome::Batch = refill(&directory, &mut buffer, Refill::Next) {}

    directory
        .try_into_owned()
        .expect("every query's share was released");
}
