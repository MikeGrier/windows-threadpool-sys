// Copyright (c) Mike Grier.

//! Tests for the owned handle primitive.
//!
//! One case captures a handle value **after** it has been closed, which is only
//! meaningful while that value stays unallocated: if another thread opened
//! something in the window between the close and the capture, Windows could hand
//! the value back out and the capture would succeed against a different object.
//! Every test that opens a handle therefore takes [`handle_allocation`] for
//! read, and that one test takes it for write, so the window is closed against
//! this binary's own concurrency rather than left to timing.

use std::fs::{File, OpenOptions};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsHandle, AsRawHandle, OwnedHandle, RawHandle};
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

use windows_sys::Win32::Foundation::ERROR_INVALID_HANDLE;
use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetCurrentThread};

use super::{CapturedHandle, HandleCaptureFailure, pseudo};

/// Serialises handle allocation across this test binary.
///
/// Read: "I may open handles." Write: "no other test may open a handle while I
/// reason about a specific handle value."
fn handle_allocation() -> &'static RwLock<()> {
    static LOCK: OnceLock<RwLock<()>> = OnceLock::new();
    LOCK.get_or_init(|| RwLock::new(()))
}

/// A temporary directory holding one file, removed on drop.
///
/// Named per process and per label so concurrent tests cannot collide.
struct Fixture {
    directory: PathBuf,
}

const FILE_CONTENTS: &[u8] = b"contents";

impl Fixture {
    fn new(label: &str) -> Self {
        let directory = std::env::temp_dir().join(format!(
            "windows-namespace-request-sys-{}-{label}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("create the fixture directory");
        std::fs::write(directory.join("f.t"), FILE_CONTENTS).expect("write the fixture file");
        Self { directory }
    }

    fn directory(&self) -> &Path {
        &self.directory
    }

    fn file(&self) -> PathBuf {
        self.directory.join("f.t")
    }

    fn open_file(&self) -> File {
        File::open(self.file()).expect("open the fixture file")
    }

    /// Opens the directory the way every audited consumer does.
    fn open_directory(&self) -> File {
        OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(self.directory())
            .expect("open the fixture directory")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

fn expect_invalid_handle_error(raw: RawHandle, expected: HandleCaptureFailure) {
    // SAFETY: the value is never dereferenced; capture_raw validates it and,
    // for the DuplicateHandle cases, Win32 rejects it.
    let error = unsafe { CapturedHandle::capture_raw(raw) }
        .expect_err("capture must refuse this handle value");

    assert_eq!(error.failure(), expected);
    assert_eq!(
        error.raw_os_error(),
        Some(i32::try_from(ERROR_INVALID_HANDLE).expect("ERROR_INVALID_HANDLE fits in i32"))
    );
}

#[test]
fn captures_a_live_file_handle() {
    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");
    let fixture = Fixture::new("capture-file");
    let file = fixture.open_file();

    let captured = CapturedHandle::capture(file.as_handle()).expect("capture the file handle");

    assert!(!captured.as_handle().as_raw_handle().is_null());
}

#[test]
fn captures_a_live_directory_handle() {
    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");
    let fixture = Fixture::new("capture-directory");
    let directory = fixture.open_directory();

    let captured =
        CapturedHandle::capture(directory.as_handle()).expect("capture the directory handle");

    assert!(!captured.as_handle().as_raw_handle().is_null());
}

#[test]
fn the_duplicate_is_a_distinct_handle_value() {
    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");
    let fixture = Fixture::new("distinct-value");
    let file = fixture.open_file();

    let captured = CapturedHandle::capture(file.as_handle()).expect("capture the file handle");

    assert_ne!(
        captured.as_handle().as_raw_handle(),
        file.as_raw_handle(),
        "a duplicate is a second reference, so it has its own handle value"
    );
}

#[test]
fn dropping_the_duplicate_leaves_the_source_usable() {
    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");
    let fixture = Fixture::new("drop-duplicate");
    let file = fixture.open_file();

    let captured = CapturedHandle::capture(file.as_handle()).expect("capture the file handle");
    drop(captured);

    let metadata = file
        .metadata()
        .expect("the source handle must survive the duplicate being closed");
    assert_eq!(metadata.len(), FILE_CONTENTS.len() as u64);
}

#[test]
fn the_duplicate_outlives_the_source_being_dropped() {
    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");
    let fixture = Fixture::new("outlive-source");
    let captured = {
        let file = fixture.open_file();
        CapturedHandle::capture(file.as_handle()).expect("capture the file handle")
    };

    let adopted = File::from(captured.into_owned_handle());
    let metadata = adopted
        .metadata()
        .expect("the duplicate must survive its source being closed");
    assert_eq!(metadata.len(), FILE_CONTENTS.len() as u64);
}

#[test]
fn try_clone_yields_a_second_independent_duplicate() {
    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");
    let fixture = Fixture::new("try-clone");
    let file = fixture.open_file();
    let first = CapturedHandle::capture(file.as_handle()).expect("capture the file handle");

    let second = first.try_clone().expect("duplicate the duplicate");

    assert_ne!(
        first.as_handle().as_raw_handle(),
        second.as_handle().as_raw_handle()
    );
    drop(second);

    let adopted = File::from(first.into_owned_handle());
    adopted
        .metadata()
        .expect("closing one duplicate must not disturb the other");
}

#[test]
fn refuses_a_null_handle() {
    expect_invalid_handle_error(std::ptr::null_mut(), HandleCaptureFailure::NullHandle);
}

#[test]
fn refuses_invalid_handle_value() {
    // SAFETY: GetCurrentProcess only reads a constant pseudo-handle value.
    let current_process = unsafe { GetCurrentProcess() };
    assert_eq!(
        current_process as isize,
        pseudo::CURRENT_PROCESS,
        "INVALID_HANDLE_VALUE and the current-process pseudo-handle are the same value, \
         which is why an unchecked CreateFileW failure would otherwise duplicate cleanly"
    );

    expect_invalid_handle_error(current_process, HandleCaptureFailure::InvalidHandleValue);
}

#[test]
fn refuses_the_current_thread_pseudo_handle() {
    // SAFETY: GetCurrentThread only reads a constant pseudo-handle value.
    let raw = unsafe { GetCurrentThread() };
    assert_eq!(
        raw as isize,
        pseudo::CURRENT_THREAD,
        "the named constant must agree with what Win32 actually returns"
    );

    expect_invalid_handle_error(raw, HandleCaptureFailure::PseudoHandle);
}

#[test]
fn refuses_every_remaining_pseudo_handle() {
    // Windows exports no function for the token pseudo-handles -- they are
    // header macros -- so these come from the constants the capture path itself
    // uses rather than being restated here.
    for value in [
        pseudo::RESERVED,
        pseudo::CURRENT_PROCESS_TOKEN,
        pseudo::CURRENT_THREAD_TOKEN,
        pseudo::CURRENT_THREAD_EFFECTIVE_TOKEN,
    ] {
        expect_invalid_handle_error(value as RawHandle, HandleCaptureFailure::PseudoHandle);
    }
}

#[test]
fn refuses_a_closed_handle() {
    let fixture = Fixture::new("closed-handle");

    let exclusive = handle_allocation()
        .write()
        .expect("the lock is not poisoned");
    let raw = {
        let file = fixture.open_file();
        file.as_raw_handle()
    };
    expect_invalid_handle_error(raw, HandleCaptureFailure::DuplicateHandle);
    drop(exclusive);
}

#[test]
fn refuses_a_handle_value_that_was_never_valid() {
    // Misaligned and implausibly large: never a kernel handle, and not a
    // pseudo-handle either, so it reaches DuplicateHandle and is refused there.
    let raw = 0x0badf00d_usize as RawHandle;

    expect_invalid_handle_error(raw, HandleCaptureFailure::DuplicateHandle);
}

#[test]
fn the_error_names_the_stage_that_failed() {
    // SAFETY: a null handle is validated, never dereferenced.
    let null = unsafe { CapturedHandle::capture_raw(std::ptr::null_mut()) }
        .expect_err("a null handle cannot be captured");
    assert!(
        null.to_string().contains("null source handle"),
        "unexpected message: {null}"
    );

    // SAFETY: as above; this value is validated before any use.
    let refused = unsafe { CapturedHandle::capture_raw(0x0badf00d_usize as RawHandle) }
        .expect_err("a bogus handle cannot be captured");
    assert!(
        refused.to_string().contains("DuplicateHandle"),
        "unexpected message: {refused}"
    );
}

#[test]
fn a_captured_handle_moves_and_shares_across_threads() {
    const fn assert_send<T: Send>() {}
    const fn assert_sync<T: Sync>() {}

    assert_send::<CapturedHandle>();
    assert_sync::<CapturedHandle>();

    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");
    let fixture = Fixture::new("cross-thread");
    let file = fixture.open_file();
    let captured = CapturedHandle::capture(file.as_handle()).expect("capture the file handle");

    let observed = std::thread::spawn(move || {
        let adopted = File::from(captured.into_owned_handle());
        adopted
            .metadata()
            .expect("the duplicate must be usable on another thread")
            .len()
    })
    .join()
    .expect("the worker did not panic");

    assert_eq!(observed, FILE_CONTENTS.len() as u64);
}

#[test]
fn into_owned_handle_and_from_agree() {
    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");
    let fixture = Fixture::new("into-owned");
    let file = fixture.open_file();
    let captured = CapturedHandle::capture(file.as_handle()).expect("capture the file handle");
    let expected = captured.as_handle().as_raw_handle();

    let owned = OwnedHandle::from(captured);

    assert_eq!(owned.as_raw_handle(), expected);
}
