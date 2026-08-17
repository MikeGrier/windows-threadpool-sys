// Copyright (c) 2026 Mike Grier
//! End-to-end verification of the `TP_IO` backend: object creation, the balanced
//! `StartThreadpoolIo` / `CancelThreadpoolIo` accounting on the immediate-failure
//! path, and one real overlapped read reclaimed from the pool's callback.
//!
//! The full behavioral matrix (immediate success, cancellation, and rundown with
//! operations outstanding) is covered by the integration tests.

use std::io;
use std::os::windows::io::AsRawHandle;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use windows_overlapped_io_sys::{
    Issued, Operation, OperationState, Submitted, UnassociatedEndpoint,
};
use windows_sys::Win32::Foundation::ERROR_IO_PENDING;
use windows_sys::Win32::Storage::FileSystem::{ReadFile, WriteFile};

use crate::callback_env::CallbackEnviron;
use crate::io::{IoCompletion, ThreadpoolIo};

/// Create a temp file with `content` and return its path.
fn temp_file_with(content: &[u8], tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "windows-threadpool-sys-io-{tag}-{}.tmp",
        std::process::id()
    ));
    std::fs::write(&path, content).expect("write temp file");
    path
}

/// Open a temp file as an overlapped endpoint for reading.
fn read_endpoint(path: &std::path::Path) -> UnassociatedEndpoint {
    UnassociatedEndpoint::open(path, true, false, 0).expect("open overlapped endpoint")
}

// --- creation ---

#[test]
fn new_with_no_env_succeeds() {
    let path = temp_file_with(b"payload", "new-no-env");
    let tp = ThreadpoolIo::new(read_endpoint(&path), |_| {}, None);
    assert!(tp.is_ok());
    drop(tp);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn new_with_default_env_succeeds() {
    let path = temp_file_with(b"payload", "new-env");
    let mut env = CallbackEnviron::new();
    let tp = ThreadpoolIo::new(read_endpoint(&path), |_| {}, Some(&mut env));
    assert!(tp.is_ok());
    drop(tp);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn new_object_starts_with_no_outstanding_operations() {
    let path = temp_file_with(b"payload", "new-outstanding");
    let tp = ThreadpoolIo::new(read_endpoint(&path), |_| {}, None).expect("create TP_IO");
    assert_eq!(tp.outstanding(), 0);
    drop(tp);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn drop_without_submitting_is_safe() {
    let path = temp_file_with(b"payload", "drop-clean");
    {
        let _tp = ThreadpoolIo::new(read_endpoint(&path), |_| {}, None).expect("create TP_IO");
    }
    let _ = std::fs::remove_file(&path);
}

// --- immediate failure balances the start ---

/// A native call that fails immediately delivers no callback, so `submit` must
/// balance its own `StartThreadpoolIo` and hand the operation back intact.
#[test]
fn immediate_failure_returns_the_operation_and_balances_the_start() {
    let path = temp_file_with(b"payload", "immediate-failure");
    let tp = ThreadpoolIo::new(read_endpoint(&path), |_| {}, None).expect("create TP_IO");

    let source = *b"denied";
    let src_ptr = source.as_ptr();
    let src_len = source.len() as u32;
    let mut written: u32 = 0;
    let written_ptr: *mut u32 = &mut written;

    let operation = Operation::new(());
    // SAFETY: issues exactly one overlapped WriteFile on a read-only handle,
    // which fails immediately with ERROR_ACCESS_DENIED and queues no callback.
    let submitted = unsafe {
        tp.submit(operation, |handle, overlapped| {
            let ok = WriteFile(
                handle.as_raw_handle(),
                src_ptr,
                src_len,
                written_ptr,
                overlapped,
            );
            if ok != 0 {
                return Ok(Issued::Pending);
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_IO_PENDING as i32) {
                return Ok(Issued::Pending);
            }
            Err(error)
        })
    };

    match submitted {
        Submitted::Failed { operation, error } => {
            assert_eq!(operation.state(), OperationState::Idle);
            assert!(error.raw_os_error().is_some(), "expected an OS error");
        }
        other => panic!("expected an immediate failure, got {other:?}"),
    }

    // The start was balanced by CancelThreadpoolIo, so nothing is outstanding.
    assert_eq!(tp.outstanding(), 0);

    drop(tp);
    let _ = std::fs::remove_file(&path);
}

/// Repeating the failure path must not drift the accounting.
#[test]
fn repeated_immediate_failures_leave_no_outstanding_operations() {
    let path = temp_file_with(b"payload", "repeat-failure");
    let tp = ThreadpoolIo::new(read_endpoint(&path), |_| {}, None).expect("create TP_IO");

    for _ in 0..10 {
        let operation = Operation::new(());
        // SAFETY: `issue` starts no operation and reports the failure, so no
        // callback will arrive -- exactly what the Err contract requires.
        let submitted = unsafe {
            tp.submit(operation, |_handle, _overlapped| {
                Err(io::Error::from_raw_os_error(5))
            })
        };
        assert!(matches!(submitted, Submitted::Failed { .. }));
        assert_eq!(tp.outstanding(), 0);
    }

    drop(tp);
    let _ = std::fs::remove_file(&path);
}

// --- end-to-end pending completion reclaimed from the callback ---

/// One real overlapped read: the pool delivers the callback, the callback claims
/// the operation, and rundown observes the start balanced.
#[test]
fn pending_read_completes_through_the_callback() {
    let content = b"thread pool overlapped read";
    let path = temp_file_with(content, "pending-read");

    let seen: Arc<Mutex<Vec<(u32, usize)>>> = Arc::new(Mutex::new(Vec::new()));
    let recorder = Arc::clone(&seen);

    let tp = ThreadpoolIo::new(
        read_endpoint(&path),
        move |completion: &IoCompletion| {
            // SAFETY: this object only ever carries `Operation<()>`, submitted
            // below, and each completion is claimed exactly once.
            let operation = unsafe { completion.claim::<()>() };
            assert_eq!(operation.state(), OperationState::Completed);
            recorder
                .lock()
                .expect("record completion")
                .push((completion.io_result(), completion.bytes_transferred()));
        },
        None,
    )
    .expect("create TP_IO");

    let mut buffer = [0_u8; 64];
    let buf_ptr = buffer.as_mut_ptr();
    let buf_len = buffer.len() as u32;
    let mut bytes: u32 = 0;
    let bytes_ptr: *mut u32 = &mut bytes;

    let mut operation = Operation::new(());
    operation.set_offset(0);

    // SAFETY: issues exactly one overlapped ReadFile into `buffer`, which stays
    // alive until this test blocks in `run_down()` below. The handle is not in
    // skip-on-success mode, so both synchronous success and ERROR_IO_PENDING
    // deliver a completion callback.
    let submitted = unsafe {
        tp.submit(operation, |handle, overlapped| {
            let ok = ReadFile(
                handle.as_raw_handle(),
                buf_ptr,
                buf_len,
                bytes_ptr,
                overlapped,
            );
            if ok != 0 {
                return Ok(Issued::Pending);
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_IO_PENDING as i32) {
                return Ok(Issued::Pending);
            }
            Err(error)
        })
    };

    assert!(
        matches!(submitted, Submitted::Pending(_)),
        "expected a pending submission, got {submitted:?}"
    );

    // The completion callback balances the start, so rundown returns only after
    // the callback has run and reclaimed the operation.
    tp.run_down();
    assert_eq!(tp.outstanding(), 0);

    let recorded = seen.lock().expect("read completions").clone();
    assert_eq!(recorded.len(), 1, "expected exactly one completion");
    let (io_result, transferred) = recorded[0];
    assert_eq!(io_result, 0, "expected a successful read");
    assert_eq!(transferred, content.len());
    assert_eq!(&buffer[..content.len()], content);

    drop(tp);
    let _ = std::fs::remove_file(&path);
}
