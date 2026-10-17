// Copyright (c) 2026 Mike Grier
//! One file, real writers: other processes grow, shrink and delete a file a `HeldFile` holds.
#![cfg(windows)]

use std::fs::OpenOptions;
use std::io::Write;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use win_multi_file_tail_watcher::{HeldFile, OpenError};
use wtf_string::Wtf16String;

const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_PATH_NOT_FOUND: u32 = 3;

/// What each pass of the child's loop adds to the file.
const LINE: &str = "123456789ab\r\n";
const PASSES: u64 = 5;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "win-multi-file-tail-watcher-it-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir(&dir).expect("create the test directory");
        Self(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
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

/// A `cmd.exe` (`/q`: its own command echo would otherwise land in the file) that, for each line read on its stdin, appends [`LINE`] to its stdout, which is
/// the file at `path`.
fn start_writer(path: &Path) -> Child {
    let stdout = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .expect("open the child's stdout file");
    Command::new("cmd")
        .raw_arg(format!(
            "/q /c \"for /l %i in (1,1,{PASSES}) do ((set /p x=)&(echo {}))\"",
            LINE.trim_end()
        ))
        .stdin(Stdio::piped())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::null())
        .spawn()
        .expect("start cmd.exe")
}

/// Wait until the held length is exactly `expected`, so a length that overshoots fails at once.
fn expect_len(held: &HeldFile, expected: u64) {
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut last = u64::MAX;
    while Instant::now() < deadline {
        last = held.len().expect("len");
        assert!(
            last <= expected,
            "length {last} passed the expected {expected}"
        );
        if last == expected {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("length stayed at {last}, expected {expected}");
}

#[test]
fn a_child_processs_output_is_tracked_a_write_at_a_time() {
    let dir = TempDir::new("child");
    let path = dir.path("out.log");
    let mut child = start_writer(&path);
    let held = HeldFile::open(&wtf(&path)).expect("open");
    let mut stdin = child.stdin.take().expect("stdin");

    expect_len(&held, 0);
    for pass in 1..=PASSES {
        stdin.write_all(b"go\r\n").expect("release one pass");
        stdin.flush().expect("flush");
        expect_len(&held, pass * LINE.len() as u64);
    }
    drop(stdin);
    assert!(child.wait().expect("wait").success());
    assert_eq!(held.len().expect("len"), PASSES * LINE.len() as u64);
}

#[test]
fn a_shrink_by_another_handle_is_the_smaller_length() {
    let dir = TempDir::new("shrink");
    let path = dir.path("s.log");
    std::fs::write(&path, vec![b'x'; 5000]).expect("create");
    let held = HeldFile::open(&wtf(&path)).expect("open");
    assert_eq!(held.len().expect("len"), 5000);

    let writer = OpenOptions::new().write(true).open(&path).expect("writer");
    for len in [4096u64, 100, 0, 7] {
        writer.set_len(len).expect("set_len");
        assert_eq!(held.len().expect("len"), len);
    }
}

#[test]
fn a_missing_path_yields_its_win32_code() {
    let dir = TempDir::new("missing");
    let in_dir = HeldFile::open(&wtf(&dir.path("absent.log"))).expect_err("no such file");
    assert_eq!(code(&in_dir), Some(ERROR_FILE_NOT_FOUND));

    let in_missing_dir =
        HeldFile::open(&wtf(&dir.path("no-dir").join("a.log"))).expect_err("no such directory");
    assert_eq!(code(&in_missing_dir), Some(ERROR_PATH_NOT_FOUND));

    let at = HeldFile::open_at(&wtf(&dir.path("absent.log")), 0).expect_err("no such file");
    match at {
        OpenError::Io(error) => assert_eq!(code(&error), Some(ERROR_FILE_NOT_FOUND)),
        OpenError::OffsetBeyondLength { .. } => panic!("an open failure, not an offset"),
    }
}

#[test]
fn an_offset_past_the_length_yields_the_distinct_error() {
    let dir = TempDir::new("offset");
    let path = dir.path("o.log");
    std::fs::write(&path, b"0123456789").expect("create");

    match HeldFile::open_at(&wtf(&path), 11).expect_err("past the end") {
        OpenError::OffsetBeyondLength { offset, len } => assert_eq!((offset, len), (11, 10)),
        OpenError::Io(error) => panic!("an offset error, not an open failure: {error}"),
    }
    assert_eq!(
        HeldFile::open_at(&wtf(&path), 10)
            .expect("at the end")
            .start_offset(),
        10
    );
}

#[test]
fn other_processes_can_write_to_and_delete_the_file_while_it_is_held() {
    let dir = TempDir::new("others");
    let path = dir.path("h.log");
    std::fs::write(&path, b"abc").expect("create");
    let held = HeldFile::open(&wtf(&path)).expect("open");

    let appended = Command::new("cmd")
        .raw_arg(format!("/c \"echo more>> \"{}\"\"", path.display()))
        .stdout(Stdio::null())
        .status()
        .expect("run an appender");
    assert!(appended.success(), "a second process appends while held");
    expect_len(&held, 3 + "more\r\n".len() as u64);

    let deleted = Command::new("cmd")
        .raw_arg(format!("/c \"del \"{}\"\"", path.display()))
        .stdout(Stdio::null())
        .status()
        .expect("run a deleter");
    assert!(deleted.success(), "a second process deletes while held");
    assert!(!path.exists());
    assert_eq!(held.len().expect("len"), 3 + "more\r\n".len() as u64);
}
