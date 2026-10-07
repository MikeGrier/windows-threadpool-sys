// Copyright (c) 2026 Mike Grier
//! Unit tests for `SharedHandle`.
//!
//! Whether the handle is still open is observed through Windows, not through the reference count:
//! each file is opened with no sharing, so any other open of it fails with a sharing violation
//! while the handle lives and succeeds once it has closed.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsHandle, AsRawHandle, OwnedHandle};
use std::path::PathBuf;

use super::SharedHandle;

/// `ERROR_SHARING_VIOLATION`: what opening a file fails with while another handle holds it with no
/// sharing allowed.
const ERROR_SHARING_VIOLATION: i32 = 32;

/// No sharing: while a handle opened this way lives, every other open of the file fails.
const SHARE_NONE: u32 = 0;

/// A file in the temporary directory, removed when the test ends.
struct TempFile(PathBuf);

impl TempFile {
    fn new(test: &str) -> Self {
        Self(std::env::temp_dir().join(format!("wsooh-{test}-{}.tmp", std::process::id())))
    }

    /// Open the file for writing with no sharing, creating it if needed.
    fn open_exclusive(&self) -> File {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .share_mode(SHARE_NONE)
            .open(&self.0)
            .expect("open the test file")
    }

    /// Whether some handle still holds the file: a plain open fails with a sharing violation
    /// exactly while one does.
    fn is_held(&self) -> bool {
        match File::open(&self.0) {
            Ok(_) => false,
            Err(error) if error.raw_os_error() == Some(ERROR_SHARING_VIOLATION) => true,
            Err(error) => panic!("unexpected error probing {}: {error}", self.0.display()),
        }
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[test]
fn new_lends_the_handle_it_was_given() {
    let file = TempFile::new("new-lends");
    let owned = OwnedHandle::from(file.open_exclusive());
    let raw = owned.as_raw_handle();
    let shared = SharedHandle::new(owned);
    assert_eq!(shared.as_raw_handle(), raw);
    assert_eq!(shared.as_handle().as_raw_handle(), raw);
}

#[test]
fn from_file_keeps_the_same_handle() {
    let file = TempFile::new("from-file");
    let opened = file.open_exclusive();
    let raw = opened.as_raw_handle();
    let shared = SharedHandle::from(opened);
    assert_eq!(shared.as_raw_handle(), raw);
}

#[test]
fn from_owned_handle_is_new() {
    let file = TempFile::new("from-owned");
    let owned = OwnedHandle::from(file.open_exclusive());
    let raw = owned.as_raw_handle();
    let shared: SharedHandle = owned.into();
    assert_eq!(shared.as_raw_handle(), raw);
}

#[test]
fn a_clone_shares_the_handle() {
    let file = TempFile::new("clone-shares");
    let first = SharedHandle::from(file.open_exclusive());
    let second = first.clone();
    assert!(first.same_handle(&second));
    assert_eq!(first.as_raw_handle(), second.as_raw_handle());
}

#[test]
fn a_clone_of_a_clone_shares_the_handle() {
    let file = TempFile::new("clone-of-clone");
    let first = SharedHandle::from(file.open_exclusive());
    let third = first.clone().clone();
    assert!(first.same_handle(&third));
    assert!(third.same_handle(&first));
}

#[test]
fn separately_opened_handles_are_not_the_same_handle() {
    let file = TempFile::new("separate");
    std::fs::write(&file.0, b"x").expect("create the test file");
    let one = SharedHandle::from(File::open(&file.0).expect("first open"));
    let two = SharedHandle::from(File::open(&file.0).expect("second open"));
    assert!(!one.same_handle(&two));
    assert!(!two.same_handle(&one));
    assert!(one.same_handle(&one));
}

#[test]
fn the_handle_stays_open_while_any_clone_lives() {
    let file = TempFile::new("stays-open");
    let first = SharedHandle::from(file.open_exclusive());
    let second = first.clone();
    drop(first);
    assert!(file.is_held(), "a surviving clone keeps the handle open");
    drop(second);
}

#[test]
fn the_handle_closes_when_the_last_clone_drops() {
    let file = TempFile::new("closes");
    let first = SharedHandle::from(file.open_exclusive());
    let second = first.clone();
    assert!(file.is_held());
    drop(first);
    drop(second);
    assert!(!file.is_held(), "the last drop closes the handle");
}

#[test]
fn the_only_holder_gets_the_handle_back() {
    let file = TempFile::new("only-holder");
    let shared = SharedHandle::from(file.open_exclusive());
    let raw = shared.as_raw_handle();
    let owned = shared.try_into_owned().expect("the only holder");
    assert_eq!(owned.as_raw_handle(), raw);
}

#[test]
fn a_holder_that_is_not_the_only_one_gets_itself_back() {
    let file = TempFile::new("not-only");
    let first = SharedHandle::from(file.open_exclusive());
    let second = first.clone();
    let returned = first.try_into_owned().expect_err("another holder remains");
    assert!(returned.same_handle(&second));
    assert!(file.is_held(), "a refused hand-back closes nothing");
}

#[test]
fn the_handle_comes_back_once_the_other_holders_drop() {
    let file = TempFile::new("comes-back");
    let first = SharedHandle::from(file.open_exclusive());
    let second = first.clone();
    let first = first.try_into_owned().expect_err("second still holds it");
    drop(second);
    let owned = first.try_into_owned().expect("now the only holder");
    drop(owned);
    assert!(!file.is_held());
}

#[test]
fn handing_the_handle_back_does_not_close_it() {
    let file = TempFile::new("hand-back");
    let shared = SharedHandle::from(file.open_exclusive());
    let owned = shared.try_into_owned().expect("the only holder");
    assert!(file.is_held(), "the owned handle is still open");
    drop(owned);
    assert!(!file.is_held(), "and closes when it drops");
}

#[test]
fn a_clone_can_live_on_another_thread() {
    let file = TempFile::new("thread");
    let shared = SharedHandle::from(file.open_exclusive());
    let raw = shared.as_raw_handle() as usize;
    let elsewhere = shared.clone();
    let seen = std::thread::spawn(move || elsewhere.as_raw_handle() as usize)
        .join()
        .expect("the thread ran");
    assert_eq!(seen, raw);
    let _ = shared
        .try_into_owned()
        .expect("the thread's clone has dropped");
}

#[test]
fn the_lent_handle_works_with_windows_calls() -> io::Result<()> {
    let file = TempFile::new("lent");
    let shared = SharedHandle::from(file.open_exclusive());
    {
        let mut writer = File::from(shared.as_handle().try_clone_to_owned()?);
        writer.write_all(b"written through a lent handle")?;
    }
    let mut reader = File::from(shared.as_handle().try_clone_to_owned()?);
    let mut text = String::new();
    std::io::Seek::rewind(&mut reader)?;
    reader.read_to_string(&mut text)?;
    assert_eq!(text, "written through a lent handle");
    Ok(())
}

#[test]
fn shared_handle_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<SharedHandle>();
}

#[test]
fn debug_names_the_type() {
    let file = TempFile::new("debug");
    let shared = SharedHandle::from(file.open_exclusive());
    assert!(format!("{shared:?}").starts_with("SharedHandle("));
}
