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
fn an_observer_taken_before_binding_outlives_the_object() {
    use std::os::windows::fs::OpenOptionsExt;

    let path = temp_file_with(b"payload", "observer-outlives");
    // Opening with no sharing fails while any handle to the file is open.
    let no_handle_is_open = || {
        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .is_ok()
    };
    let endpoint = read_endpoint(&path);
    let observer = endpoint.observer();
    let tp = ThreadpoolIo::new(endpoint, |_| {}, None).expect("create TP_IO");

    drop(tp);
    assert!(
        !no_handle_is_open(),
        "the observer must keep the handle open"
    );
    drop(observer);
    assert!(no_handle_is_open(), "the last holder must close the handle");
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

    // `start-cancelled` is asserted here rather than in the pending-read test
    // because this is the path that produces it: a start balanced without a
    // callback. Without the record, this operation's `started` would read as a
    // dispatch that never arrived -- which is exactly the shape of the defect
    // the trace is being used to chase.
    // Asserted in a child launched with the trace armed, not behind a `wants`
    // check that is false on every ordinary run. See
    // `trace::in_a_trace_armed_child`: this block was inert for its whole life,
    // which is the same defect found and fixed in three other places before
    // anybody swept for the rest of them.
    #[cfg(feature = "trace")]
    crate::trace::in_a_trace_armed_child(
        "io::tests::immediate_failure_returns_the_operation_and_balances_the_start",
        "io",
        || {
            assert!(
                crate::trace::counted("io", "start-cancelled") > 0,
                "`io` recorded no `start-cancelled`; the trace is narrowed to it and a start \
                 has just been balanced with no callback, so the call site is missing"
            );
        },
    );

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

    // The `io` object's whole life, asserted here because this test already
    // owns the only real overlapped exercise in the crate -- and after the drop
    // above, so the teardown stages are in the trace by the time it reads.
    //
    // Run in a child launched with the trace armed. This used to be a `wants`
    // check, which is false on every ordinary run and in CI, so it asserted
    // nothing for its whole life. See `trace::in_a_trace_armed_child`, and
    // `crate::trace::tests::every_pool_object_records_its_creation_establishment_callbacks_and_teardown`,
    // which carries the same reasoning for the other four objects.
    #[cfg(feature = "trace")]
    crate::trace::in_a_trace_armed_child(
        "io::tests::pending_read_completes_through_the_callback",
        "io",
        || {
            for event in [
                "created",
                "started",
                "trampoline-entered",
                "trampoline-left",
                "rundown-begin",
                "rundown-ended",
                "drop-begin",
                "drop-drained",
                "drop-closed",
            ] {
                assert!(
                    crate::trace::counted("io", event) > 0,
                    "`io` recorded no `{event}`; the trace is narrowed to it and an object has \
                     just been created, started, dispatched, run down and dropped, so the call \
                     site is missing"
                );
            }
        },
    );

    let _ = std::fs::remove_file(&path);
}

// --- observers alive across cancellation and rundown ---

mod observers {
    use std::io::{self, Write};
    use std::os::windows::io::{AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle};
    use std::sync::{Arc, Mutex};

    use windows_overlapped_io_sys::test_util::{OBSERVER_QUERIES, run_every_query};
    use windows_overlapped_io_sys::{
        EndpointObserver, Issued, Operation, Submitted, UnassociatedEndpoint,
    };
    use windows_sys::Win32::Foundation::{
        ERROR_IO_PENDING, ERROR_OPERATION_ABORTED, INVALID_HANDLE_VALUE, NO_ERROR,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX, ReadFile,
    };
    use windows_sys::Win32::System::IO::OVERLAPPED;
    use windows_sys::Win32::System::Pipes::{CreateNamedPipeW, PIPE_TYPE_BYTE};

    use super::{read_endpoint, temp_file_with};
    use crate::io::{IoCompletion, ThreadpoolIo};

    /// A connected server pipe end carrying no data, so a read on it pends until
    /// cancelled, and the client end, through which a write shows whether the
    /// server end is still open.
    fn pending_pipe(tag: &str) -> (UnassociatedEndpoint, std::fs::File) {
        let name = format!(
            r"\\.\pipe\windows-threadpool-sys-io-observer-{tag}-{}",
            std::process::id()
        );
        let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        // SAFETY: a NUL-terminated name and no security attributes.
        let raw = unsafe {
            CreateNamedPipeW(
                wide.as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
                PIPE_TYPE_BYTE,
                1,
                4096,
                4096,
                0,
                std::ptr::null(),
            )
        };
        assert_ne!(
            raw,
            INVALID_HANDLE_VALUE,
            "CreateNamedPipeW: {}",
            io::Error::last_os_error()
        );
        // SAFETY: a fresh overlapped pipe, unassociated, unduplicated, moved in exclusively.
        let endpoint =
            unsafe { UnassociatedEndpoint::assume_overlapped(OwnedHandle::from_raw_handle(raw)) };
        let client = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&name)
            .expect("connect the client end");
        (endpoint, client)
    }

    /// Whether the server end is still open: a client write fails once it closes.
    fn server_is_open(client: &mut std::fs::File) -> bool {
        client.write(b"x").is_ok()
    }

    /// Issue one overlapped `ReadFile` on a handle not in skip-on-success mode.
    ///
    /// SAFETY: `buffer` must hold `len` writable bytes until the read completes.
    unsafe fn issue_read(
        handle: BorrowedHandle<'_>,
        overlapped: *mut OVERLAPPED,
        buffer: *mut u8,
        len: u32,
    ) -> io::Result<Issued> {
        // SAFETY: forwarded from this function's own contract.
        let ok = unsafe {
            ReadFile(
                handle.as_raw_handle(),
                buffer,
                len,
                std::ptr::null_mut(),
                overlapped,
            )
        };
        if ok != 0 {
            return Ok(Issued::Pending);
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(ERROR_IO_PENDING as i32) {
            return Ok(Issued::Pending);
        }
        Err(error)
    }

    /// An object whose callback claims each `Operation<()>` and records its result.
    fn recording_object(endpoint: UnassociatedEndpoint) -> (ThreadpoolIo, Arc<Mutex<Vec<u32>>>) {
        let results = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&results);
        let tp = ThreadpoolIo::new(
            endpoint,
            move |completion: &IoCompletion| {
                // SAFETY: only Operation<()> is submitted on these objects, claimed once.
                let _operation = unsafe { completion.claim::<()>() };
                seen.lock()
                    .expect("record a completion")
                    .push(completion.io_result());
            },
            None,
        )
        .expect("create TP_IO");
        (tp, results)
    }

    #[test]
    fn an_observer_from_the_object_reads_the_length_after_it_drops() {
        let path = temp_file_with(b"payload", "observer-length");
        let tp = ThreadpoolIo::new(read_endpoint(&path), |_| {}, None).expect("create TP_IO");
        let observer = tp.observer();

        drop(tp);
        assert_eq!(observer.file_len().expect("file_len"), 7);
        drop(observer);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn cancelling_with_observers_alive_aborts_the_read_and_they_still_read() {
        let (endpoint, mut client) = pending_pipe("cancel");
        let before = endpoint.observer();
        let (tp, results) = recording_object(endpoint);
        let after = tp.observer();
        assert!(before.same_handle(&after));

        let mut buffer = [0_u8; 8];
        let buf_ptr = buffer.as_mut_ptr();
        // SAFETY: one read on a pipe carrying no data, so it pends; `buffer`
        // outlives it because run_down below waits for its callback.
        let submitted =
            unsafe { tp.submit(Operation::new(()), |h, ov| issue_read(h, ov, buf_ptr, 8)) };
        let Submitted::Pending(id) = submitted else {
            panic!("expected a pending read, got {submitted:?}");
        };
        after.file_len().expect("a query while the read pends");
        assert_eq!(tp.outstanding(), 1);

        tp.cancel(id).expect("cancel");
        tp.run_down();
        assert_eq!(*results.lock().expect("results"), [ERROR_OPERATION_ABORTED]);

        drop(tp);
        assert!(
            server_is_open(&mut client),
            "the observers must keep the handle open"
        );
        before.file_len().expect("a query after the object drops");
        after.file_len().expect("a query after the object drops");
        drop((before, after));
        assert!(
            !server_is_open(&mut client),
            "the last holder must close the handle"
        );
    }

    #[test]
    fn dropping_with_a_read_pending_drains_it_while_an_observer_keeps_the_handle_open() {
        let (endpoint, mut client) = pending_pipe("drop");
        let (tp, results) = recording_object(endpoint);
        let observer = tp.observer();

        let mut buffer = [0_u8; 8];
        let buf_ptr = buffer.as_mut_ptr();
        // SAFETY: one read on a pipe carrying no data, so it pends; `buffer`
        // outlives it because the drop below waits for its callback.
        let submitted =
            unsafe { tp.submit(Operation::new(()), |h, ov| issue_read(h, ov, buf_ptr, 8)) };
        assert!(matches!(submitted, Submitted::Pending(_)));

        // Drop must cancel explicitly: with the observer holding the handle, nothing closes it.
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(tp))).is_err();
        assert_eq!(
            panicked,
            cfg!(feature = "fail-fast"),
            "only `fail-fast` turns a skipped rundown into a panic"
        );
        assert_eq!(*results.lock().expect("results"), [ERROR_OPERATION_ABORTED]);

        assert!(
            server_is_open(&mut client),
            "the observer must keep the handle open"
        );
        observer.file_len().expect("a query after the drain");
        drop(observer);
        assert!(
            !server_is_open(&mut client),
            "the last holder must close the handle"
        );
    }

    /// Issue every observer query `rounds` times, checking each round issued them all.
    fn query_repeatedly(observer: &EndpointObserver, rounds: usize) {
        for _ in 0..rounds {
            assert_eq!(
                run_every_query(observer).len(),
                OBSERVER_QUERIES.len(),
                "a round issues every query"
            );
        }
    }

    #[test]
    fn every_query_while_a_read_pends_leaves_it_to_complete() {
        let (endpoint, mut client) = pending_pipe("every-query-complete");
        let (tp, results) = recording_object(endpoint);
        let observer = tp.observer();

        let mut buffer = [0_u8; 8];
        let buf_ptr = buffer.as_mut_ptr();
        // SAFETY: one read on a pipe carrying no data, so it pends; `buffer`
        // outlives it because run_down below waits for its callback.
        let submitted =
            unsafe { tp.submit(Operation::new(()), |h, ov| issue_read(h, ov, buf_ptr, 8)) };
        assert!(matches!(submitted, Submitted::Pending(_)));
        query_repeatedly(&observer, 4);
        assert_eq!(tp.outstanding(), 1);
        assert!(
            results.lock().expect("results").is_empty(),
            "the queries completed nothing"
        );

        client.write_all(b"data").expect("client write");
        tp.run_down();
        assert_eq!(*results.lock().expect("results"), [NO_ERROR]);
        assert_eq!(&buffer[..4], b"data");
        query_repeatedly(&observer, 1);
    }

    #[test]
    fn every_query_while_a_read_pends_leaves_it_to_cancel() {
        let (endpoint, _client) = pending_pipe("every-query-cancel");
        let (tp, results) = recording_object(endpoint);
        let observer = tp.observer();

        let mut buffer = [0_u8; 8];
        let buf_ptr = buffer.as_mut_ptr();
        // SAFETY: one read on a pipe carrying no data, so it pends; `buffer`
        // outlives it because run_down below waits for its callback.
        let submitted =
            unsafe { tp.submit(Operation::new(()), |h, ov| issue_read(h, ov, buf_ptr, 8)) };
        let Submitted::Pending(id) = submitted else {
            panic!("expected a pending read, got {submitted:?}");
        };
        query_repeatedly(&observer, 4);
        assert_eq!(tp.outstanding(), 1);

        tp.cancel(id).expect("cancel");
        tp.run_down();
        assert_eq!(*results.lock().expect("results"), [ERROR_OPERATION_ABORTED]);
        query_repeatedly(&observer, 1);
    }
}
