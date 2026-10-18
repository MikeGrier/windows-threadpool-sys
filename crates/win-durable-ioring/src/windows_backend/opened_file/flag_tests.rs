// Copyright (c) 2026 Mike Grier
//! The namespace, lifetime and I/O-mode flags of an open (DI-14.2, DI-14.9):
//! `FILE_FLAG_DELETE_ON_CLOSE`, `FILE_FLAG_OPEN_NO_RECALL`, `FILE_FLAG_POSIX_SEMANTICS` and
//! `FILE_FLAG_OVERLAPPED`.

use std::fs;
use std::io;

use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_DELETE_ON_CLOSE, FILE_FLAG_OPEN_NO_RECALL, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_FLAG_OVERLAPPED, FILE_FLAG_POSIX_SEMANTICS, FILE_FLAG_RANDOM_ACCESS,
};

use super::{
    Cached, DataAccess, Disposition, FILE_FLAG_NO_BUFFERING, FileOptions, FileRights, OpenedFile,
    ReadWrite, ShareMode, SharingViolation, Unbuffered,
};
use crate::dioring::tests::{TempFile, wtf};

fn read_write() -> FileOptions<ReadWrite> {
    FileOptions::new().read().write()
}

fn flags<A: DataAccess>(options: FileOptions<A>) -> u32 {
    options.parameters(0).flags
}

#[test]
fn each_flag_sets_its_own_bit_and_no_other() {
    let base = read_write();
    assert_eq!(flags(base), 0, "no flag asked for, none set");
    assert_eq!(flags(base.delete_on_close(true)), FILE_FLAG_DELETE_ON_CLOSE);
    assert_eq!(flags(base.open_no_recall(true)), FILE_FLAG_OPEN_NO_RECALL);
    assert_eq!(flags(base.posix_semantics(true)), FILE_FLAG_POSIX_SEMANTICS);
    assert_eq!(flags(base.overlapped(true)), FILE_FLAG_OVERLAPPED);
    assert_eq!(
        flags(
            base.delete_on_close(true)
                .open_no_recall(true)
                .posix_semantics(true)
                .overlapped(true)
        ),
        FILE_FLAG_DELETE_ON_CLOSE
            | FILE_FLAG_OPEN_NO_RECALL
            | FILE_FLAG_POSIX_SEMANTICS
            | FILE_FLAG_OVERLAPPED,
        "together, they are joined and none replaces another"
    );
    assert_eq!(
        flags(base.overlapped(true).overlapped(false)),
        0,
        "the flag can be taken back"
    );
    assert_eq!(
        flags(base.delete_on_close(true).delete_on_close(false)),
        0,
        "a flag can be taken back"
    );
}

#[test]
fn the_flags_are_added_to_the_modes_and_the_other_options_never_in_place_of_them() {
    let flags = read_write()
        .open_reparse_point(true)
        .random_access(true)
        .delete_on_close(true)
        .parameters(FILE_FLAG_NO_BUFFERING)
        .flags;
    assert_eq!(
        flags,
        FILE_FLAG_NO_BUFFERING
            | FILE_FLAG_OPEN_REPARSE_POINT
            | FILE_FLAG_RANDOM_ACCESS
            | FILE_FLAG_DELETE_ON_CLOSE
    );
}

#[test]
fn changing_the_data_access_keeps_the_flags() {
    let moved = FileOptions::new()
        .delete_on_close(true)
        .open_no_recall(true)
        .posix_semantics(true)
        .overlapped(true)
        .read()
        .write();
    assert_eq!(
        flags(moved),
        FILE_FLAG_DELETE_ON_CLOSE
            | FILE_FLAG_OPEN_NO_RECALL
            | FILE_FLAG_POSIX_SEMANTICS
            | FILE_FLAG_OVERLAPPED
    );
}

#[test]
fn a_file_opened_delete_on_close_is_gone_when_its_last_handle_closes() {
    let temp = TempFile::new("fl-delete");
    let fresh = temp.0.with_extension("fresh");
    let options = read_write()
        .rights(FileRights::DELETE)
        .disposition(Disposition::OpenAlways)
        .delete_on_close(true);

    let first = OpenedFile::<Cached>::open(wtf(&fresh), options).expect("create it");
    assert!(fresh.exists());
    // A second handle to the file, opened without the flag, keeps it alive.
    let second = OpenedFile::<Cached>::open(wtf(&fresh), read_write()).expect("a second open");
    drop(first);
    assert!(fresh.exists(), "a handle is still open");
    drop(second);
    assert!(!fresh.exists(), "the last handle closed");
}

#[test]
fn a_file_opened_without_the_flag_stays_when_closed() {
    let temp = TempFile::new("fl-stays");
    let fresh = temp.0.with_extension("fresh");
    let options = read_write().disposition(Disposition::OpenAlways);
    drop(OpenedFile::<Cached>::open(wtf(&fresh), options).expect("create it"));
    assert!(fresh.exists());
    fs::remove_file(&fresh).expect("remove");
}

#[test]
fn delete_on_close_needs_no_right_to_delete_among_the_rights_asked_for() {
    // Measured on Windows 11 build 26300, and evidence, not a contract: the `CreateFileW` pages do not
    // say the right to delete is needed (`ZwCreateFile`'s does, for its own flag), and the kernel does
    // not refuse an open that leaves it out: the file goes all the same.
    let temp = TempFile::new("fl-no-delete");
    let opened = OpenedFile::<Cached>::open(wtf(&temp.0), read_write().delete_on_close(true))
        .expect("opens without the right to delete");
    drop(opened);
    assert!(!temp.0.exists(), "and the file is deleted when it closes");
}

#[test]
fn a_later_open_that_does_not_share_deletion_is_a_sharing_violation_while_the_flag_is_held() {
    // Measured on Windows 11 build 26300: the handle that sets the flag holds the right to delete,
    // so a later open has to share deletion, whatever it asks for itself.
    let temp = TempFile::new("fl-share");
    let _held = OpenedFile::<Cached>::open(wtf(&temp.0), read_write().delete_on_close(true))
        .expect("the first");
    let no_delete = ShareMode::READ | ShareMode::WRITE;

    let error = OpenedFile::<Cached>::open(wtf(&temp.0), read_write().share(no_delete))
        .expect_err("does not share deletion");
    assert_eq!(error.kind(), io::ErrorKind::ResourceBusy);
    assert!(
        error
            .get_ref()
            .is_some_and(|inner| inner.downcast_ref::<SharingViolation>().is_some())
    );

    // And one that shares it, with the right to delete or without, is admitted.
    OpenedFile::<Cached>::open(wtf(&temp.0), read_write()).expect("shares deletion");
    OpenedFile::<Cached>::open(wtf(&temp.0), read_write().rights(FileRights::DELETE))
        .expect("asks for the right to delete and shares it");
}

#[test]
fn the_other_two_flags_are_accepted_by_the_kernel_on_a_local_file() {
    // Passed on and nothing a target depends on; what the kernel does with them is measured only as
    // far as that the open succeeds and the file is usable.
    for options in [
        read_write().open_no_recall(true),
        read_write().posix_semantics(true),
        read_write().open_no_recall(true).posix_semantics(true),
    ] {
        let temp = TempFile::new("fl-others");
        let opened = OpenedFile::<Cached>::open(wtf(&temp.0), options).expect("open");
        drop(opened);
        assert!(temp.0.exists());
    }
}

#[test]
fn the_flags_are_passed_to_an_unbuffered_file_as_to_a_cached_one() {
    let temp = TempFile::new("fl-unbuffered");
    let options = read_write()
        .rights(FileRights::DELETE)
        .disposition(Disposition::OpenAlways)
        .delete_on_close(true);
    let fresh = temp.0.with_extension("fresh");
    let file = OpenedFile::<Unbuffered>::open(wtf(&fresh), options).expect("create it");
    assert!(fresh.exists());
    drop(file);
    assert!(!fresh.exists(), "the last handle closed");
}
