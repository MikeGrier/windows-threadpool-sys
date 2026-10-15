// Copyright (c) 2026 Mike Grier
use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::Write;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use windows_overlapped_io_sys::FileType;
use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, ERROR_SHARING_VIOLATION,
};
use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;
use wtf_string::Wtf16String;

use super::HeldFile;

/// A fresh directory for one test, removed with its contents when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "win-multi-file-tail-watcher-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir(&dir).expect("create the test directory");
        Self(dir)
    }

    fn file(&self, name: impl AsRef<std::ffi::OsStr>, content: &[u8]) -> PathBuf {
        let path = self.0.join(name.as_ref());
        std::fs::write(&path, content).expect("create the test file");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn wtf(path: &Path) -> Wtf16String {
    Wtf16String::from(path.as_os_str())
}

fn code(error: &std::io::Error) -> Option<u32> {
    error.raw_os_error().map(i32::cast_unsigned)
}

#[test]
fn an_existing_file_opens_and_its_observer_is_a_live_disk_handle() {
    let dir = TempDir::new("opens");
    let held = HeldFile::open(&wtf(&dir.file("a.log", b"hello"))).expect("open");
    assert_eq!(
        held.observer().file_type().expect("file_type"),
        FileType::Disk
    );
}

#[test]
fn an_empty_file_opens() {
    let dir = TempDir::new("empty");
    let held = HeldFile::open(&wtf(&dir.file("empty.log", b""))).expect("open");
    assert_eq!(
        held.observer().file_type().expect("file_type"),
        FileType::Disk
    );
}

#[test]
fn every_observer_of_one_file_shares_its_handle() {
    let dir = TempDir::new("shared");
    let held = HeldFile::open(&wtf(&dir.file("a.log", b"x"))).expect("open");
    let first = held.observer();
    let second = held.observer();
    let third = first.clone();
    assert!(first.same_handle(&second));
    assert!(second.same_handle(&third));
}

#[test]
fn two_holds_of_one_path_are_independent() {
    let dir = TempDir::new("twice");
    let path = wtf(&dir.file("a.log", b"x"));
    let first = HeldFile::open(&path).expect("first");
    let second = HeldFile::open(&path).expect("second");
    assert!(!first.observer().same_handle(&second.observer()));
}

#[test]
fn an_observer_keeps_the_handle_open_after_the_file_is_dropped() {
    let dir = TempDir::new("outlive");
    let held = HeldFile::open(&wtf(&dir.file("a.log", b"x"))).expect("open");
    let observer = held.observer();
    drop(held);
    assert_eq!(observer.file_type().expect("file_type"), FileType::Disk);
}

#[test]
fn a_missing_file_is_file_not_found() {
    let dir = TempDir::new("missing");
    let error = HeldFile::open(&wtf(&dir.0.join("nothing.log"))).expect_err("no such file");
    assert_eq!(code(&error), Some(ERROR_FILE_NOT_FOUND), "{error:?}");
}

#[test]
fn a_missing_directory_is_path_not_found() {
    let dir = TempDir::new("nodir");
    let error =
        HeldFile::open(&wtf(&dir.0.join("no-such-dir").join("a.log"))).expect_err("no directory");
    assert_eq!(code(&error), Some(ERROR_PATH_NOT_FOUND), "{error:?}");
}

#[test]
fn an_empty_path_is_refused_with_a_win32_code() {
    let error = HeldFile::open(&Wtf16String::new()).expect_err("an empty path");
    assert_eq!(code(&error), Some(ERROR_PATH_NOT_FOUND), "{error:?}");
}

#[test]
fn a_directory_is_refused_with_a_win32_code() {
    let dir = TempDir::new("isdir");
    let error = HeldFile::open(&wtf(&dir.0)).expect_err("a directory is not a file");
    assert_eq!(code(&error), Some(ERROR_ACCESS_DENIED), "{error:?}");
}

#[test]
fn a_file_another_handle_holds_exclusively_is_a_sharing_violation() {
    let dir = TempDir::new("exclusive");
    let path = dir.file("a.log", b"x");
    let _exclusive = OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(0)
        .open(&path)
        .expect("open exclusively");
    let error = HeldFile::open(&wtf(&path)).expect_err("shared with nobody");
    assert_eq!(code(&error), Some(ERROR_SHARING_VIOLATION), "{error:?}");
}

#[test]
fn a_path_containing_a_nul_is_refused_without_a_win32_code() {
    let mut path = Wtf16String::from("a");
    path.push_str("\0b");
    let error = HeldFile::open(&path).expect_err("a NUL names no file");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput, "{error:?}");
    assert_eq!(error.raw_os_error(), None, "{error:?}");
}

#[test]
fn another_handle_can_still_write_to_a_held_file() {
    let dir = TempDir::new("write");
    let path = dir.file("a.log", b"one");
    let _held = HeldFile::open(&wtf(&path)).expect("open");
    let mut writer = OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("a writer is not refused");
    writer.write_all(b"two").expect("write");
    drop(writer);
    assert_eq!(std::fs::read(&path).expect("read"), b"onetwo");
}

#[test]
fn another_handle_can_still_delete_a_held_file_and_the_hold_survives() {
    let dir = TempDir::new("delete");
    let path = dir.file("a.log", b"x");
    let held = HeldFile::open(&wtf(&path)).expect("open");
    std::fs::remove_file(&path).expect("delete is not refused");
    assert_eq!(
        held.observer().file_type().expect("file_type"),
        FileType::Disk
    );
}

/// The hold has no write access: a new open that denies write sharing would otherwise collide with
/// it.
#[test]
fn the_hold_is_read_only() {
    let dir = TempDir::new("readonly");
    let path = dir.file("a.log", b"x");
    let _held = HeldFile::open(&wtf(&path)).expect("open");
    let _deny_writers = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(&path)
        .expect("a hold with write access would make this a sharing violation");
}

/// The hold shares write and delete: a new open that asks for them is not refused. (The two tests
/// above, with a real writer and a real delete, are the same fact from the other side.)
#[test]
fn a_reader_that_denies_nothing_opens_beside_the_hold() {
    let dir = TempDir::new("beside");
    let path = dir.file("a.log", b"x");
    let _held = HeldFile::open(&wtf(&path)).expect("open");
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .expect("a read/write open shares with the hold");
}

#[test]
fn names_with_spaces_and_unicode_open() {
    let dir = TempDir::new("names");
    for name in [
        "with space.log",
        "caf\u{e9}.log",
        "\u{4e2d}\u{6587}.log",
        "emoji-\u{1F4C4}.log",
        "dots.in.name.log",
        "UPPER.LOG",
    ] {
        let held =
            HeldFile::open(&wtf(&dir.file(name, b"x"))).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert_eq!(
            held.observer().file_type().expect("file_type"),
            FileType::Disk,
            "{name}"
        );
    }
}

#[test]
fn a_name_with_an_unpaired_surrogate_opens_the_file_that_has_it() {
    let dir = TempDir::new("surrogate");
    let name = OsString::from_wide(&[
        u16::from(b'a'),
        0xD800,
        u16::from(b'b'),
        0xDFFF,
        u16::from(b'.'),
    ]);
    let path = dir.0.join(&name);
    std::fs::write(&path, b"x").expect("create a file whose name is not valid Unicode");
    assert!(
        path.to_str().is_none(),
        "the name must not be valid Unicode for this test to mean anything"
    );

    let held = HeldFile::open(&wtf(&path)).expect("open by the exact units");
    assert_eq!(
        held.observer().file_type().expect("file_type"),
        FileType::Disk
    );
}

#[test]
fn a_path_past_max_path_opens_by_its_verbatim_form() {
    let dir = TempDir::new("long");
    let mut deep = std::fs::canonicalize(&dir.0).expect("canonicalize");
    for segment in 0..12 {
        deep.push(format!("segment-number-{segment:02}-padding"));
    }
    std::fs::create_dir_all(&deep).expect("create deep directories");
    let path = deep.join("deep.log");
    std::fs::write(&path, b"x").expect("create");
    assert!(path.as_os_str().len() > 260);

    let held = HeldFile::open(&wtf(&path)).expect("open");
    assert_eq!(
        held.observer().file_type().expect("file_type"),
        FileType::Disk
    );
}

#[test]
fn a_held_file_can_move_between_threads() {
    fn assert_send_and_sync<T: Send + Sync>() {}
    assert_send_and_sync::<HeldFile>();
}

#[test]
fn a_new_files_length_is_its_content() {
    let dir = TempDir::new("len-content");
    for (name, content) in [
        ("zero", &b""[..]),
        ("one", b"x"),
        ("five", b"hello"),
        ("page", &[7u8; 4096]),
        ("page-plus", &[7u8; 4097]),
        ("big", &[7u8; 100_000]),
    ] {
        let held = HeldFile::open(&wtf(&dir.file(name, content))).expect("open");
        assert_eq!(held.len().expect("len"), content.len() as u64, "{name}");
    }
}

#[test]
fn the_length_tracks_each_write_by_a_writer_that_stays_open() {
    let dir = TempDir::new("len-grow");
    let path = dir.file("grow.log", b"");
    let held = HeldFile::open(&wtf(&path)).expect("open");
    let mut writer = OpenOptions::new().append(true).open(&path).expect("writer");
    let mut expected = 0u64;
    for step in 1..=12usize {
        let chunk = vec![b'a'; step * 37];
        writer.write_all(&chunk).expect("write");
        expected += chunk.len() as u64;
        assert_eq!(held.len().expect("len"), expected, "step {step}");
    }
}

#[test]
fn a_shrink_by_another_handle_is_the_smaller_length() {
    let dir = TempDir::new("len-shrink");
    let path = dir.file("shrink.log", &[1u8; 1000]);
    let held = HeldFile::open(&wtf(&path)).expect("open");
    let writer = OpenOptions::new().write(true).open(&path).expect("writer");
    for len in [600u64, 600, 10, 0, 250] {
        writer.set_len(len).expect("set_len");
        assert_eq!(held.len().expect("len"), len);
    }
}

#[test]
fn the_length_is_not_cut_to_32_bits() {
    let dir = TempDir::new("len-big");
    let path = dir.file("big.bin", b"");
    let target = (1u64 << 32) + 5;
    OpenOptions::new()
        .write(true)
        .open(&path)
        .expect("writer")
        .set_len(target)
        .expect("set_len");
    let held = HeldFile::open(&wtf(&path)).expect("open");
    assert_eq!(held.len().expect("len"), target);
}

#[test]
fn the_length_is_still_read_after_the_file_is_deleted() {
    let dir = TempDir::new("len-deleted");
    let path = dir.file("gone.log", b"twelve bytes");
    let held = HeldFile::open(&wtf(&path)).expect("open");
    std::fs::remove_file(&path).expect("delete while held");
    assert_eq!(held.len().expect("len"), 12);
}

#[test]
fn the_length_is_still_read_after_the_file_is_renamed() {
    let dir = TempDir::new("len-renamed");
    let path = dir.file("before.log", b"abc");
    let held = HeldFile::open(&wtf(&path)).expect("open");
    std::fs::rename(&path, dir.0.join("after.log")).expect("rename while held");
    std::fs::write(&path, b"a different file at the old path, longer").expect("recreate");
    assert_eq!(held.len().expect("len"), 3);
}

#[test]
fn an_observer_reads_the_same_length_and_outlives_the_hold() {
    let dir = TempDir::new("len-observer");
    let path = dir.file("o.log", b"12345");
    let held = HeldFile::open(&wtf(&path)).expect("open");
    let observer = held.observer();
    assert_eq!(observer.file_len().expect("len"), held.len().expect("len"));
    drop(held);
    OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("writer")
        .write_all(b"678")
        .expect("write");
    assert_eq!(observer.file_len().expect("len"), 8);
}

#[test]
fn two_holds_of_one_path_read_the_same_length() {
    let dir = TempDir::new("len-two");
    let path = dir.file("t.log", b"abcd");
    let first = HeldFile::open(&wtf(&path)).expect("open");
    let second = HeldFile::open(&wtf(&path)).expect("open");
    std::fs::write(&path, b"abcdefgh").expect("write");
    assert_eq!(first.len().expect("len"), 8);
    assert_eq!(second.len().expect("len"), 8);
}

#[test]
fn repeated_reads_with_no_change_agree() {
    let dir = TempDir::new("len-steady");
    let held = HeldFile::open(&wtf(&dir.file("s.log", b"steady"))).expect("open");
    for _ in 0..50 {
        assert_eq!(held.len().expect("len"), 6);
    }
}
