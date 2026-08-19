// Copyright (c) 2026 Mike Grier
//! Integration test: decode a buffer produced by a real `ReadDirectoryChangesW`
//! call. The watch is armed on a worker thread and the change is made from the
//! test thread after a short delay. The read is issued *overlapped* with a
//! bounded wait and is cancelled on timeout, so the worker can never block
//! forever, and the test joins it before returning rather than leaving a
//! detached thread that could wedge the rest of the suite.
#![cfg(windows)]

use std::ffi::OsString;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr;
use std::sync::mpsc;
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_IO_PENDING, FALSE, HANDLE, INVALID_HANDLE_VALUE, TRUE, WAIT_OBJECT_0,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED, FILE_LIST_DIRECTORY,
    FILE_NOTIFY_CHANGE_FILE_NAME, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    OPEN_EXISTING, ReadDirectoryChangesW,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

use windows_file_watcher::{ChangeKind, DecodedBatch, decode_batch};

/// The NUL-terminated wide form of a path, for `CreateFileW`.
fn wide_z(p: &Path) -> Vec<u16> {
    p.as_os_str().encode_wide().chain(Some(0)).collect()
}

#[test]
fn decodes_a_real_read_directory_changes_buffer() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "windows-file-watcher-decode-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir(&dir).expect("create temp dir");

    let (armed_tx, armed_rx) = mpsc::channel::<()>();
    let watch_dir = dir.clone();
    let worker = std::thread::spawn(move || watch_once(&watch_dir, &armed_tx));

    // Wait until the worker has actually armed the read before making the change,
    // so the test is deterministic on a slow runner instead of relying on a fixed
    // sleep that could fire before ReadDirectoryChangesW is even issued.
    armed_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("the worker must arm the read");
    std::fs::write(dir.join("created.txt"), b"hi").expect("create file");

    // Joining cannot hang: the worker's overlapped read has a bounded wait and is
    // cancelled on timeout, so it always returns.
    let changes = worker
        .join()
        .expect("the worker thread must not panic")
        .expect("the completion must decode to changes, not a desync");

    assert!(
        changes.iter().any(|(kind, name)| {
            *kind == ChangeKind::Added && name == &OsString::from("created.txt")
        }),
        "expected an Added record for created.txt; got {changes:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Open `dir`, arm one overlapped `ReadDirectoryChangesW`, signal `armed` once
/// the read is queued, then wait for it under a bounded timeout and decode the
/// completion into `(kind, name)` pairs. The directory handle is closed exactly
/// once before returning.
fn watch_once(dir: &Path, armed: &mpsc::Sender<()>) -> Result<Vec<(ChangeKind, OsString)>, String> {
    let name = wide_z(dir);
    // SAFETY: `name` is a valid NUL-terminated wide path; the returned handle is
    // closed exactly once below before this function returns.
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            FILE_LIST_DIRECTORY,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(format!(
            "CreateFileW failed: {}",
            std::io::Error::last_os_error()
        ));
    }

    let result = read_overlapped(handle, armed);

    // SAFETY: `handle` came from `CreateFileW` and is closed exactly once.
    unsafe { CloseHandle(handle) };
    result
}

/// Arm a single overlapped read on `handle`, signal `armed` once it is queued in
/// the kernel, and wait for it under a bounded timeout, cancelling and draining
/// the read if it does not complete in time so the kernel is no longer
/// referencing the buffer when this function returns.
fn read_overlapped(
    handle: HANDLE,
    armed: &mpsc::Sender<()>,
) -> Result<Vec<(ChangeKind, OsString)>, String> {
    /// The read is bounded so a missed change surfaces as an error, never a hang.
    const TIMEOUT_MS: u32 = 30_000;

    // Manual-reset, initially non-signaled event backing the overlapped read.
    // SAFETY: default attributes; an unnamed event whose handle is closed once.
    let event = unsafe { CreateEventW(ptr::null(), TRUE, FALSE, ptr::null()) };
    if event.is_null() {
        return Err(format!(
            "CreateEventW failed: {}",
            std::io::Error::last_os_error()
        ));
    }

    // A `Vec<u32>` guarantees the DWORD alignment `ReadDirectoryChangesW` requires.
    let mut buffer = vec![0_u32; 1024];
    // The OVERLAPPED lives on the heap (a `Box`) so that, on the unlikely path
    // where a cancelled read cannot be confirmed retired, it can be *leaked*
    // rather than freed while the kernel might still write to it.
    // SAFETY: `OVERLAPPED` is plain data; zeroing then setting `hEvent` is the
    // documented way to bind the read to our wait event.
    let mut overlapped: Box<OVERLAPPED> = Box::new(unsafe { std::mem::zeroed() });
    overlapped.hEvent = event;

    // SAFETY: `buffer`, `overlapped`, and `event` all outlive the read below; a
    // non-null overlapped issues the read asynchronously and signals `event` on
    // completion. `lpBytesReturned` is undefined for overlapped reads, so it is
    // null and `GetOverlappedResult` supplies the count instead.
    let arm_ok = unsafe {
        ReadDirectoryChangesW(
            handle,
            buffer.as_mut_ptr().cast(),
            (buffer.len() * 4) as u32,
            0, // bWatchSubtree = FALSE
            FILE_NOTIFY_CHANGE_FILE_NAME,
            ptr::null_mut(),
            &mut *overlapped,
            None,
        )
    };
    if arm_ok == 0 {
        // An overlapped read reports a successfully *queued* read as FALSE +
        // ERROR_IO_PENDING; only a different error is a genuine arm failure.
        // Treating ERROR_IO_PENDING as failure here would close the event and drop
        // `buffer`/`overlapped` while the kernel still references them.
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
            // SAFETY: `event` is valid and not associated with any in-flight read.
            unsafe { CloseHandle(event) };
            return Err(format!("ReadDirectoryChangesW failed to arm: {err}"));
        }
    }

    // The read is now queued in the kernel; the test may safely make its change.
    let _ = armed.send(());

    // SAFETY: `event` is a valid manual-reset event handle.
    let waited = unsafe { WaitForSingleObject(event, TIMEOUT_MS) };
    let mut transferred: u32 = 0;

    if waited == WAIT_OBJECT_0 {
        // The event signaled: the read has completed and is retired, so the buffer
        // is safe to read and the locals are safe to drop.
        // SAFETY: the overlapped result is available.
        let ok = unsafe { GetOverlappedResult(handle, &*overlapped, &mut transferred, FALSE) };
        let outcome = if ok == 0 {
            Err(format!(
                "GetOverlappedResult failed: {}",
                std::io::Error::last_os_error()
            ))
        } else {
            // SAFETY: the kernel wrote `transferred` bytes into `buffer`.
            let bytes = unsafe {
                std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), transferred as usize)
            };
            decode(bytes)
        };
        // SAFETY: the read is retired; the event is closed exactly once.
        unsafe { CloseHandle(event) };
        return outcome;
    }

    // Timeout or wait failure. Cancel the pending read, then confirm it is retired
    // within a *bounded* wait before `buffer`/`overlapped` are dropped. A cancelled
    // overlapped read still signals its event on completion, so we wait on the
    // event rather than blocking unbounded in `GetOverlappedResult(.., TRUE)`.
    // SAFETY: `handle`/`overlapped` name the single in-flight read.
    let cancelled = unsafe { CancelIoEx(handle, &*overlapped) } != 0;
    let cancel_err = std::io::Error::last_os_error();
    // SAFETY: `event` is a valid handle.
    let retired = unsafe { WaitForSingleObject(event, TIMEOUT_MS) } == WAIT_OBJECT_0;

    if retired {
        // SAFETY: the read is retired; collect and discard the aborted result.
        unsafe { GetOverlappedResult(handle, &*overlapped, &mut transferred, FALSE) };
        // SAFETY: the read is retired; the event is closed exactly once.
        unsafe { CloseHandle(event) };
        return Err(format!(
            "the watch did not report a change within {TIMEOUT_MS} ms \
             (wait result {waited}, cancelled = {cancelled})"
        ));
    }

    // The read could not be confirmed retired within the bound (cancel reported
    // {cancel_err}). The kernel may still write into `buffer`/`overlapped`, so
    // dropping them would be unsound. Leak the heap `buffer` and the boxed
    // `overlapped`, and leave `event` open, rather than free live storage; this
    // path is not expected to occur for a cancelled directory read.
    std::mem::forget(buffer);
    std::mem::forget(overlapped);
    Err(format!(
        "overlapped read could not be retired within {TIMEOUT_MS} ms after CancelIoEx \
         (cancel error {cancel_err}); leaked buffer/overlapped to stay sound"
    ))
}

/// Decode a completion buffer into `(kind, name)` pairs, mapping a desync to an
/// error so the test fails loudly rather than silently.
fn decode(bytes: &[u8]) -> Result<Vec<(ChangeKind, OsString)>, String> {
    match decode_batch(bytes) {
        DecodedBatch::Changes(changes) => Ok(changes
            .into_iter()
            .map(|c| (c.kind, c.name.to_os_string()))
            .collect()),
        DecodedBatch::Desync(cause) => Err(format!("unexpected desync: {cause:?}")),
    }
}
