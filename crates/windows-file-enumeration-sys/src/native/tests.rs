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

fn buffer() -> NativeBuffer {
    NativeBuffer::try_new(MINIMUM_BUFFER_CAPACITY).expect("allocation")
}

fn open(path: &std::path::Path) -> Result<OwnedHandle, EnumerationError> {
    open_directory(&wide(path), &token())
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
