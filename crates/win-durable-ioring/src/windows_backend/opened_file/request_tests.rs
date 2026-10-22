// Copyright (c) 2026 Mike Grier
//! The open as a build and a perform (DI-13.7.4).

use std::fs::{self, File};
use std::io::{self, Write};
use std::os::windows::io::AsHandle;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use wtf_string::Wtf16String;

use super::{
    Cached, Disposition, FileOptions, OpenRequest, OpenedFile, ReadWrite, ShareMode, Unbuffered,
};
use crate::dioring::tests::{TempFile, wtf};
use durable_ioring::{NoData, ReadOnly};

fn read_write() -> FileOptions<ReadWrite> {
    FileOptions::new().read().write()
}

fn io_through(opened: &impl AsHandle) -> File {
    File::from(
        opened
            .as_handle()
            .try_clone_to_owned()
            .expect("duplicate the handle"),
    )
}

/// The current directory is the process's, so a test that changes it holds this, and puts it back.
static CURRENT_DIRECTORY: Mutex<()> = Mutex::new(());

struct Directory(PathBuf);

impl Directory {
    fn enter(to: &Path) -> Self {
        let before = std::env::current_dir().expect("a current directory");
        std::env::set_current_dir(to).expect("change directory");
        Self(before)
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        std::env::set_current_dir(&self.0).expect("restore the directory");
    }
}

#[test]
fn a_request_can_be_sent_to_another_thread() {
    fn is_send<T: Send>() {}
    is_send::<OpenRequest<Cached, ReadWrite>>();
    is_send::<OpenRequest<Unbuffered, ReadOnly>>();
    is_send::<OpenRequest<Cached, NoData>>();
}

#[test]
fn a_request_built_on_one_thread_opens_the_file_its_path_named_when_it_was_built() {
    let _held = CURRENT_DIRECTORY.lock().unwrap_or_else(|e| e.into_inner());
    let temp = TempFile::new("rq-dir");
    let here = temp.0.with_extension("here");
    let there = temp.0.with_extension("there");
    fs::create_dir_all(&here).expect("create a directory");
    fs::create_dir_all(&there).expect("create another");
    fs::write(here.join("file"), b"here").expect("fill");
    fs::write(there.join("file"), b"there, longer").expect("fill");

    let request = {
        let _in = Directory::enter(&here);
        OpenedFile::<Cached>::request(Wtf16String::from("file"), read_write()).expect("build")
    };
    // The directory has changed by the time the request is made, on another thread.
    let _in = Directory::enter(&there);
    let opened = std::thread::spawn(move || request.perform())
        .join()
        .expect("the thread")
        .expect("perform");
    assert_eq!(
        io_through(&opened).metadata().expect("metadata").len(),
        4,
        "it opened the file in the directory the request was built in"
    );

    drop((opened, _in));
    fs::remove_dir_all(&here).expect("remove");
    fs::remove_dir_all(&there).expect("remove");
}

#[test]
fn performing_twice_opens_twice() {
    let temp = TempFile::new("rq-twice");
    let request = OpenedFile::<Cached>::request(wtf(&temp.0), read_write()).expect("build");
    let first = request.perform().expect("the first");
    let second = request.perform().expect("the second");
    assert!(
        !first.handle().same_handle(second.handle()),
        "each perform is its own open"
    );
}

#[test]
fn a_second_perform_is_judged_against_the_first_as_a_second_open_is() {
    let temp = TempFile::new("rq-exclusive");
    let request = OpenedFile::<Cached>::request(wtf(&temp.0), read_write().share(ShareMode::NONE))
        .expect("build");
    let _first = request.perform().expect("the first");
    let error = request.perform().expect_err("the file is held exclusively");
    assert_eq!(error.kind(), io::ErrorKind::ResourceBusy);
}

#[test]
fn an_error_from_the_build_and_one_from_the_perform_are_distinguishable() {
    let temp = TempFile::new("rq-errors");
    let missing = temp.0.with_extension("missing");

    // The build refuses a path that cannot mean a file, before anything is touched.
    let built = OpenedFile::<Cached>::request(Wtf16String::from("a\0b"), read_write());
    assert_eq!(
        built.expect_err("an interior NUL").kind(),
        io::ErrorKind::InvalidInput
    );

    // The build accepts a path to a file that is not there: whether it is is the perform's to find.
    let request = OpenedFile::<Cached>::request(wtf(&missing), read_write())
        .expect("a request for a missing file builds");
    assert!(!missing.exists(), "building opened nothing");
    let error = request.perform().expect_err("no such file");
    assert_eq!(error.kind(), io::ErrorKind::NotFound);

    // And what failed to open can be made to succeed by changing the world, not the request.
    fs::write(&missing, b"now").expect("create it");
    request.perform().expect("it opens once the file is there");
    fs::remove_file(&missing).expect("remove");
}

#[test]
fn building_creates_nothing_and_performing_creates_what_the_disposition_says() {
    let temp = TempFile::new("rq-create");
    let fresh = temp.0.with_extension("fresh");
    let request = OpenedFile::<Cached>::request(
        wtf(&fresh),
        read_write().disposition(Disposition::CreateNew),
    )
    .expect("build");
    assert!(!fresh.exists());
    drop(request.perform().expect("create"));
    assert!(fresh.exists());
    let error = request.perform().expect_err("it exists now");
    assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
    fs::remove_file(&fresh).expect("remove");
}

#[test]
fn open_is_the_build_and_the_perform_run_together() {
    let temp = TempFile::new("rq-same");
    let by_open = OpenedFile::<Cached>::open(wtf(&temp.0), read_write()).expect("open");
    let by_request = OpenedFile::<Cached>::request(wtf(&temp.0), read_write())
        .expect("build")
        .perform()
        .expect("perform");

    // Two handles to the one file: what is written through one is there through the other.
    io_through(&by_open)
        .write_all(b"through open")
        .expect("write");
    assert_eq!(
        io_through(&by_request).metadata().expect("metadata").len(),
        b"through open".len() as u64
    );

    // And a failure is the same failure by either route.
    let missing = temp.0.with_extension("missing");
    let through_open =
        OpenedFile::<Cached>::open(wtf(&missing), read_write()).expect_err("no such file");
    let through_request = OpenedFile::<Cached>::request(wtf(&missing), read_write())
        .expect("build")
        .perform()
        .expect_err("no such file");
    assert_eq!(through_open.kind(), through_request.kind());
    assert_eq!(through_open.raw_os_error(), through_request.raw_os_error());
}

#[test]
fn an_unbuffered_request_for_a_path_on_another_machine_builds_and_is_judged_when_performed() {
    // Nothing is looked at until the perform, so the build of a path on a device that does not
    // exist succeeds; the refusal itself is
    // `an_unbuffered_file_is_refused_on_a_remote_device_and_a_cached_one_is_not`.
    let request = OpenedFile::<Unbuffered>::request(
        Wtf16String::from(r"\\no-such-server-for-a-test\share\file"),
        read_write(),
    )
    .expect("the build checks nothing");
    request.perform().expect_err("the server is not there");
}
