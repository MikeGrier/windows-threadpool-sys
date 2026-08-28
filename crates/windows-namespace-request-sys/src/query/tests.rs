// Copyright (c) Mike Grier.

//! Tests for the `GetFileInformationByHandleEx` entry.
//!
//! The cursor-sharing measurements this entry's contract rests on are asserted
//! here against a real directory, with a deliberately small buffer so the
//! cursor questions actually arise. A buffer large enough to drain the fixture
//! in one call would answer none of them.

use std::fs::File;
use std::os::windows::io::AsHandle;

use windows_sys::Win32::Foundation::{ERROR_BAD_LENGTH, ERROR_NO_MORE_FILES};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_BASIC_INFO, FILE_ID_EXTD_DIR_INFO, FILE_ID_INFO, FileStandardInfo,
};

use super::{FileInformationClass, QueryFileInformation};
use crate::CapturedHandle;
use crate::buffer::AlignedBuffer;
use crate::handle::tests::{Fixture, handle_allocation};

/// Small on purpose: large enough for a few entries, far too small to drain the
/// fixture, so the enumeration cursor is left mid-directory.
const SMALL_BATCH: usize = 320;

/// Enough files that SMALL_BATCH cannot hold the whole directory, so an
/// enumeration is genuinely left mid-directory and the cursor questions
/// actually arise. A fixture that fits in one batch would answer none of them
/// and would pass regardless of what the cursor does.
const ENOUGH_TO_SPAN_BATCHES: usize = 24;

fn query(file: &File, class: FileInformationClass) -> QueryFileInformation {
    QueryFileInformation::new(
        CapturedHandle::capture(file.as_handle()).expect("capture the handle"),
        class,
    )
}

/// Reads the entry names out of one `FILE_ID_EXTD_DIR_INFO` batch.
///
/// The consumer's job, done here because the crate deliberately returns bytes
/// and does not parse them.
fn names_in(batch: &AlignedBuffer) -> Vec<String> {
    let mut names = Vec::new();
    let mut offset = 0_usize;

    loop {
        let base = batch.as_ptr().wrapping_add(offset);
        // SAFETY: the batch is 8-byte aligned and Windows wrote whole records
        // into it; each record's NextEntryOffset bounds the walk.
        let info = unsafe { &*base.cast::<FILE_ID_EXTD_DIR_INFO>() };

        let name_bytes = info.FileNameLength as usize;
        let name_offset = std::mem::offset_of!(FILE_ID_EXTD_DIR_INFO, FileName);
        // SAFETY: the name follows the fixed part, in-bounds by the same
        // record-length guarantee.
        let units = unsafe {
            std::slice::from_raw_parts(base.wrapping_add(name_offset).cast::<u16>(), name_bytes / 2)
        };
        names.push(String::from_utf16_lossy(units));

        if info.NextEntryOffset == 0 {
            break;
        }
        offset += info.NextEntryOffset as usize;
    }

    names
}

/// Reads one batch and returns its names, or `None` once the directory is done.
fn next_names(request: &QueryFileInformation) -> Option<Vec<String>> {
    match request.perform() {
        Ok(batch) => Some(names_in(&batch)),
        Err(error) if error.code() == ERROR_NO_MORE_FILES => None,
        Err(error) => panic!("unexpected enumeration failure: {error}"),
    }
}

#[test]
fn a_fixed_size_class_returns_the_whole_buffer() {
    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");
    let fixture = Fixture::new("query-basic");
    let file = fixture.open_file();

    let bytes = query(&file, FileInformationClass::BASIC)
        .with_capacity(size_of::<FILE_BASIC_INFO>())
        .perform()
        .expect("query FileBasicInfo");

    assert_eq!(
        bytes.len(),
        size_of::<FILE_BASIC_INFO>(),
        "the whole buffer comes back; the call reports no written length"
    );
}

#[test]
fn the_returned_buffer_is_eight_byte_aligned() {
    // Not decoration: a misaligned batch fails the very first query with
    // ERROR_NOACCESS, and reading its i64 fields would be UB regardless.
    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");
    let fixture = Fixture::with_extra_files("query-alignment", ENOUGH_TO_SPAN_BATCHES);
    let directory = fixture.open_directory();

    let batch = query(&directory, FileInformationClass::ID_EXTD_DIRECTORY_RESTART)
        .with_capacity(SMALL_BATCH)
        .perform()
        .expect("read the first batch");

    assert_eq!(batch.as_ptr() as usize % 8, 0);
    assert_eq!(batch.align(), 8);
}

#[test]
fn the_id_class_reads_a_real_file_identity() {
    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");
    let fixture = Fixture::new("query-id");
    let file = fixture.open_file();

    let bytes = query(&file, FileInformationClass::ID)
        .with_capacity(size_of::<FILE_ID_INFO>())
        .perform()
        .expect("query FileIdInfo");

    // SAFETY: Windows wrote a FILE_ID_INFO into an aligned buffer of exactly
    // that size.
    let info = unsafe { &*bytes.as_ptr().cast::<FILE_ID_INFO>() };
    assert_ne!(
        info.VolumeSerialNumber, 0,
        "a real file has a real volume serial"
    );
}

#[test]
fn a_directory_enumerates_and_then_reports_no_more_files() {
    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");
    let fixture = Fixture::with_extra_files("query-enumerate", ENOUGH_TO_SPAN_BATCHES);
    let directory = fixture.open_directory();

    let restart = query(&directory, FileInformationClass::ID_EXTD_DIRECTORY_RESTART)
        .with_capacity(SMALL_BATCH);
    let next =
        query(&directory, FileInformationClass::ID_EXTD_DIRECTORY).with_capacity(SMALL_BATCH);

    let mut seen = next_names(&restart).expect("the first batch");
    while let Some(batch) = next_names(&next) {
        seen.extend(batch);
    }

    assert!(seen.contains(&".".to_owned()));
    assert!(
        seen.contains(&"f.t".to_owned()),
        "the fixture's file must appear: {seen:?}"
    );
}

#[test]
fn a_duplicate_shares_the_enumeration_cursor() {
    // Measured, not reasoned from the object model. This is the fact the whole
    // entry's contract rests on.
    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");
    let fixture = Fixture::with_extra_files("query-cursor-shared", ENOUGH_TO_SPAN_BATCHES);
    let directory = fixture.open_directory();

    let restart = query(&directory, FileInformationClass::ID_EXTD_DIRECTORY_RESTART)
        .with_capacity(SMALL_BATCH);
    let first = next_names(&restart).expect("the first batch");

    // A *duplicate* of the same handle, which is what a request owns.
    let duplicate =
        query(&directory, FileInformationClass::ID_EXTD_DIRECTORY).with_capacity(SMALL_BATCH);
    let continued = next_names(&duplicate).expect("the duplicate continues");

    assert!(
        continued.iter().all(|name| !first.contains(name)),
        "a duplicate continues where the source stopped rather than restarting: \
         first={first:?} continued={continued:?}"
    );
}

#[test]
fn two_separate_opens_do_not_share_the_cursor() {
    // The control that gives the previous test its meaning: without it, a
    // "continuation" could not be told from any other second batch.
    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");
    let fixture = Fixture::with_extra_files("query-cursor-control", ENOUGH_TO_SPAN_BATCHES);
    let first_open = fixture.open_directory();
    let second_open = fixture.open_directory();

    let first = next_names(
        &query(&first_open, FileInformationClass::ID_EXTD_DIRECTORY_RESTART)
            .with_capacity(SMALL_BATCH),
    )
    .expect("the first open's batch");
    let second = next_names(
        &query(
            &second_open,
            FileInformationClass::ID_EXTD_DIRECTORY_RESTART,
        )
        .with_capacity(SMALL_BATCH),
    )
    .expect("the second open's batch");

    assert_eq!(
        first, second,
        "a separate open restarts, so the two agree; a duplicate would not"
    );
}

#[test]
fn dropping_a_duplicate_does_not_disturb_the_source_enumeration() {
    // What makes it safe for a request to own a duplicate and drop it.
    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");
    let fixture = Fixture::with_extra_files("query-cursor-drop", ENOUGH_TO_SPAN_BATCHES);
    let directory = fixture.open_directory();

    let restart = query(&directory, FileInformationClass::ID_EXTD_DIRECTORY_RESTART)
        .with_capacity(SMALL_BATCH);
    let first = next_names(&restart).expect("the first batch");

    drop(query(&directory, FileInformationClass::BASIC));

    let next =
        query(&directory, FileInformationClass::ID_EXTD_DIRECTORY).with_capacity(SMALL_BATCH);
    let continued = next_names(&next).expect("the source continues");

    assert!(
        continued.iter().all(|name| !first.contains(name)),
        "the source's enumeration survives a duplicate being dropped"
    );
}

#[test]
fn a_pure_read_does_not_disturb_an_enumeration_in_progress() {
    // The narrowing that matters: only the two directory classes mutate the
    // cursor, so every other query composes freely with an enumeration.
    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");
    let fixture = Fixture::with_extra_files("query-interleave", ENOUGH_TO_SPAN_BATCHES);
    let directory = fixture.open_directory();

    let restart = query(&directory, FileInformationClass::ID_EXTD_DIRECTORY_RESTART)
        .with_capacity(SMALL_BATCH);
    let first = next_names(&restart).expect("the first batch");

    // Interleaved pure reads, on the same handle and on a duplicate.
    query(&directory, FileInformationClass::BASIC)
        .with_capacity(size_of::<FILE_BASIC_INFO>())
        .perform()
        .expect("an interleaved FileBasicInfo");
    query(&directory, FileInformationClass::ID)
        .with_capacity(size_of::<FILE_ID_INFO>())
        .perform()
        .expect("an interleaved FileIdInfo");

    let next =
        query(&directory, FileInformationClass::ID_EXTD_DIRECTORY).with_capacity(SMALL_BATCH);
    let continued = next_names(&next).expect("the enumeration continues");

    assert!(
        continued.iter().all(|name| !first.contains(name)),
        "an interleaved pure read must not move the cursor"
    );
}

#[test]
fn the_cursor_moving_classes_are_named_correctly() {
    // The predicate is what a consumer binds to, so it must agree with the
    // measurements above rather than being a separate claim.
    assert!(FileInformationClass::ID_EXTD_DIRECTORY.moves_enumeration_cursor());
    assert!(FileInformationClass::ID_EXTD_DIRECTORY_RESTART.moves_enumeration_cursor());

    assert!(!FileInformationClass::BASIC.moves_enumeration_cursor());
    assert!(!FileInformationClass::ID.moves_enumeration_cursor());
    assert!(!FileInformationClass::CASE_SENSITIVE.moves_enumeration_cursor());
    assert!(!FileInformationClass::from_raw(FileStandardInfo).moves_enumeration_cursor());
}

#[test]
fn all_five_audited_classes_are_reachable() {
    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");
    let fixture = Fixture::new("query-classes");
    let directory = fixture.open_directory();

    for class in [
        FileInformationClass::BASIC,
        FileInformationClass::ID,
        FileInformationClass::CASE_SENSITIVE,
        FileInformationClass::ID_EXTD_DIRECTORY_RESTART,
        FileInformationClass::ID_EXTD_DIRECTORY,
    ] {
        let request = query(&directory, class).with_capacity(SMALL_BATCH);
        assert_eq!(request.class(), class);
        // Each reaches Windows and is judged there; none is refused here.
        let _ = request.perform();
    }
}

#[test]
fn an_unaudited_class_reaches_windows_unaltered() {
    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");
    let fixture = Fixture::new("query-unaudited");
    let file = fixture.open_file();

    let request =
        query(&file, FileInformationClass::from_raw(FileStandardInfo)).with_capacity(SMALL_BATCH);

    assert_eq!(request.class().as_raw(), FileStandardInfo);
    request
        .perform()
        .expect("a class this crate has no constant for still works");
}

#[test]
fn a_buffer_too_small_reports_the_raw_code() {
    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");
    let fixture = Fixture::new("query-small");
    let file = fixture.open_file();

    let error = query(&file, FileInformationClass::BASIC)
        .with_capacity(8)
        .perform()
        .expect_err("a buffer smaller than FILE_BASIC_INFO cannot be filled");

    assert_eq!(
        error.code(),
        ERROR_BAD_LENGTH,
        "the code is passed through, not reclassified"
    );
}

#[test]
fn a_new_request_carries_a_stated_default_capacity() {
    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");
    let fixture = Fixture::new("query-defaults");
    let file = fixture.open_file();

    let request = query(&file, FileInformationClass::BASIC);

    assert_eq!(request.capacity(), QueryFileInformation::DEFAULT_CAPACITY);
    assert_eq!(request.class(), FileInformationClass::BASIC);
}

#[test]
fn a_copy_duplicates_the_handle() {
    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");
    let fixture = Fixture::new("query-copy");
    let file = fixture.open_file();
    let request = query(&file, FileInformationClass::BASIC).with_capacity(SMALL_BATCH);

    let copy = request.try_clone().expect("duplicate the handle");

    use std::os::windows::io::AsRawHandle;
    assert_ne!(
        copy.handle().as_handle().as_raw_handle(),
        request.handle().as_handle().as_raw_handle()
    );
    assert_eq!(copy.capacity(), request.capacity());
}

#[test]
fn a_query_performs_the_same_way_on_another_thread() {
    const fn assert_send<T: Send>() {}
    const fn assert_sync<T: Sync>() {}

    assert_send::<QueryFileInformation>();
    assert_sync::<QueryFileInformation>();

    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");
    let fixture = Fixture::new("query-thread");
    let file = fixture.open_file();
    let request =
        query(&file, FileInformationClass::BASIC).with_capacity(size_of::<FILE_BASIC_INFO>());

    let length = std::thread::spawn(move || {
        request
            .perform()
            .expect("query on a worker that never saw the file")
            .len()
    })
    .join()
    .expect("the worker did not panic");

    assert_eq!(length, size_of::<FILE_BASIC_INFO>());
}

#[test]
fn a_request_survives_the_handle_it_was_built_from() {
    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");
    let fixture = Fixture::new("query-outlives");

    let request = {
        let file = fixture.open_file();
        query(&file, FileInformationClass::BASIC).with_capacity(size_of::<FILE_BASIC_INFO>())
    };

    request
        .perform()
        .expect("the owned duplicate outlives its source");
}
