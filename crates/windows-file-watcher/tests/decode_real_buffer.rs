// Copyright (c) 2026 Mike Grier
//! Integration test: decode a buffer produced by a real `ReadDirectoryChangesW`
//! call. The read is issued *overlapped* with a bounded wait and is cancelled on
//! timeout, so the worker can never block forever. A worker thread arms the read
//! and signals an "armed" channel; the test thread waits for that signal -- a
//! deterministic handshake, not a fixed delay -- before making the change, and
//! the worker is joined on every exit path rather than left detached to wedge the
//! rest of the suite.
#![cfg(windows)]

use std::ffi::OsString;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr;
use std::sync::mpsc;
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_IO_PENDING, FALSE, HANDLE, INVALID_HANDLE_VALUE, TRUE, WAIT_FAILED,
    WAIT_OBJECT_0,
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

/// The `(kind, name)` pairs the worker decodes, or an error string.
type WatchResult = Result<Vec<(ChangeKind, OsString)>, String>;

/// Joins its worker on drop, so a panic before the explicit join cannot detach an
/// armed read into later tests. The worker's read is bounded and self-cancelling,
/// so the join always returns.
struct JoinOnDrop(Option<std::thread::JoinHandle<WatchResult>>);

impl Drop for JoinOnDrop {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            let _ = handle.join();
        }
    }
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
    // Uniquely named (pid + nanosecond nonce), so repeated runs never collide. It
    // is removed only on a clean pass (the final line of this test). If any
    // `expect`/assertion panics, unwinding skips that cleanup and the directory is
    // deliberately left behind for post-mortem inspection of the failure — there
    // is intentionally no RAII always-cleanup guard.
    std::fs::create_dir(&dir).expect("create temp dir");

    let (armed_tx, armed_rx) = mpsc::channel::<()>();
    let watch_dir = dir.clone();
    // Held by a guard that joins the (bounded) worker on every exit path: if a
    // pre-join `expect` below panics, the guard's Drop joins it rather than
    // detaching an armed read into later tests.
    let mut worker = JoinOnDrop(Some(std::thread::spawn(move || {
        watch_once(&watch_dir, &armed_tx)
    })));

    // Wait until the worker has actually armed the read before making the change,
    // so the test is deterministic on a slow runner instead of relying on a fixed
    // sleep that could fire before ReadDirectoryChangesW is even issued.
    armed_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("the worker must arm the read");
    std::fs::write(dir.join("created.txt"), b"hi").expect("create file");

    // Take the handle and join it for the result (the guard's Drop then does
    // nothing). Joining cannot hang: the overlapped read has a bounded wait and is
    // cancelled on timeout, so it always returns.
    let changes = worker
        .0
        .take()
        .expect("worker handle present")
        .join()
        .expect("the worker thread must not panic")
        .expect("the completion must decode to changes, not a desync");

    assert!(
        changes.iter().any(|(kind, name)| {
            *kind == ChangeKind::Added && name == &OsString::from("created.txt")
        }),
        "expected an Added record for created.txt; got {changes:?}"
    );

    // Reached only on a clean pass; on failure the directory is intentionally
    // left for analysis (see the note at creation).
    let _ = std::fs::remove_dir_all(&dir);
}

/// Open `dir`, arm one overlapped `ReadDirectoryChangesW`, signal `armed` once
/// the read is queued, then wait for it under a bounded timeout and decode the
/// completion into `(kind, name)` pairs. Closing the directory handle is
/// `read_overlapped`'s responsibility (see its doc comment): it is closed
/// exactly once on every path where the read is confirmed retired, and is
/// deliberately leaked, alongside the other in-flight resources, on the one path
/// where retirement could not be confirmed.
fn watch_once(dir: &Path, armed: &mpsc::Sender<()>) -> Result<Vec<(ChangeKind, OsString)>, String> {
    let name = wide_z(dir);
    // SAFETY: `name` is a valid NUL-terminated wide path.
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

    read_overlapped(handle, armed)
}

/// Arm a single overlapped read on `handle`, signal `armed` once it is queued in
/// the kernel, and wait for it under a bounded timeout, cancelling and draining
/// the read if it does not complete in time so the kernel is no longer
/// referencing the buffer when this function returns. `handle` is closed exactly
/// once on every return path except the final one: if a cancelled read cannot be
/// confirmed retired within the second bounded wait, the kernel may still
/// reference `buffer`/`overlapped`/`handle`, so all three are deliberately
/// leaked rather than freed/closed while still possibly live -- this mirrors the
/// crate's own established teardown convention (see
/// `windows-overlapped-io-sys/DESIGN-NOTES.md`: closing a handle only cancels
/// its outstanding operations, reclamation must wait for the completion to be
/// observed) applied to a caller with no completion port to later observe it.
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
        let err = std::io::Error::last_os_error();
        // SAFETY: `handle` came from `CreateFileW` and no read was ever armed on it.
        unsafe { CloseHandle(handle) };
        return Err(format!("CreateEventW failed: {err}"));
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
            std::mem::size_of_val(buffer.as_slice()) as u32,
            FALSE, // bWatchSubtree
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
            // SAFETY: no read was armed on `handle`.
            unsafe { CloseHandle(handle) };
            return Err(format!("ReadDirectoryChangesW failed to arm: {err}"));
        }
    }

    // The read is now queued in the kernel; the test may safely make its change.
    let _ = armed.send(());

    // SAFETY: `event` is a valid manual-reset event handle.
    let waited = unsafe { WaitForSingleObject(event, TIMEOUT_MS) };
    // Captured immediately: `WAIT_FAILED`'s `GetLastError` would otherwise be
    // overwritten by the `CancelIoEx` call below (which sets its own error, or
    // leaves a stale one on success), losing the real diagnosis.
    let wait_err = (waited == WAIT_FAILED).then(std::io::Error::last_os_error);
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
        // SAFETY: no I/O remains outstanding on `handle`.
        unsafe { CloseHandle(handle) };
        return outcome;
    }

    // Timeout or wait failure. Cancel the pending read, then confirm it is retired
    // within a *bounded* wait before `buffer`/`overlapped` are dropped. A cancelled
    // overlapped read still signals its event on completion, so we wait on the
    // event rather than blocking unbounded in `GetOverlappedResult(.., TRUE)`.
    // SAFETY: `handle`/`overlapped` name the single in-flight read.
    let cancelled = unsafe { CancelIoEx(handle, &*overlapped) } != 0;
    // GetLastError is only meaningful when `CancelIoEx` reported failure; Win32
    // does not clear it on success, so reading it unconditionally could capture a
    // stale, unrelated error.
    let cancel_err = if cancelled {
        None
    } else {
        Some(std::io::Error::last_os_error())
    };
    // SAFETY: `event` is a valid handle.
    let retired = unsafe { WaitForSingleObject(event, TIMEOUT_MS) } == WAIT_OBJECT_0;

    if retired {
        // SAFETY: the read is retired; collect and discard the aborted result.
        unsafe { GetOverlappedResult(handle, &*overlapped, &mut transferred, FALSE) };
        // SAFETY: the read is retired; the event is closed exactly once.
        unsafe { CloseHandle(event) };
        // SAFETY: no I/O remains outstanding on `handle`.
        unsafe { CloseHandle(handle) };
        let wait_note = match wait_err {
            Some(e) => format!("WaitForSingleObject failed: {e}"),
            None => format!("wait result {waited}"),
        };
        return Err(format!(
            "the watch did not report a change within {TIMEOUT_MS} ms \
             ({wait_note}, cancelled = {cancelled})"
        ));
    }

    // The read could not be confirmed retired within the bound. The kernel may
    // still reference `buffer`/`overlapped`/`handle`, so freeing or closing any
    // of them would be unsound: leak the heap `buffer` and the boxed
    // `overlapped`, leave `event` and `handle` open, rather than free or close
    // live storage; this path is not expected to occur for a cancelled directory
    // read.
    let cancel_note = match cancel_err {
        None => "CancelIoEx succeeded".to_owned(),
        Some(e) => format!("CancelIoEx failed: {e}"),
    };
    std::mem::forget(buffer);
    std::mem::forget(overlapped);
    Err(format!(
        "overlapped read could not be retired within {TIMEOUT_MS} ms ({cancel_note}); \
         leaked buffer/overlapped/handle to stay sound"
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
