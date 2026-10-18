// Copyright (c) 2026 Mike Grier
//! End-to-end raw-IOCP test against a real, cancellable overlapped device.
//!
//! A named-pipe server with no client is used because `ConnectNamedPipe`
//! reliably pends and is cancellable with `CancelIoEx`, without touching the
//! real filesystem. This exercises association, the pending-submission path,
//! targeted cancellation, completion delivery, and reclamation end to end.

#![cfg(windows)]

use std::io;
use std::os::windows::io::{AsRawHandle, OwnedHandle};

use win_pipes_sys::{NamedPipeOptions, PipeMode, PipeOpenMode, create_named_pipe};
use windows_overlapped_io_sys::{
    CompletionPort, Issued, Operation, OperationState, Submitted, UnassociatedEndpoint,
};
use windows_sys::Win32::Foundation::{ERROR_IO_PENDING, ERROR_OPERATION_ABORTED};
use windows_sys::Win32::System::Pipes::ConnectNamedPipe;

/// An overlapped, duplex, byte-mode named-pipe server instance with no client, so that a connect
/// on it pends.
fn overlapped_server(name: &str) -> OwnedHandle {
    let options = NamedPipeOptions {
        open_mode: PipeOpenMode::ACCESS_DUPLEX | PipeOpenMode::OVERLAPPED,
        pipe_mode: PipeMode::TYPE_BYTE | PipeMode::WAIT,
        max_instances: 1,
        out_buffer_size: 4096,
        in_buffer_size: 4096,
        default_timeout_ms: 0,
    };
    create_named_pipe(name, &options, None).expect("create the overlapped pipe")
}

#[test]
fn connect_named_pipe_is_cancellable_and_completes_as_aborted() {
    let name = format!(r"\\.\pipe\windows-overlapped-io-sys-{}", std::process::id());
    let owned = overlapped_server(&name);

    let port = CompletionPort::new(0).expect("create port");
    // SAFETY: the pipe was created with FILE_FLAG_OVERLAPPED, is unassociated,
    // has no duplicates, and moves in exclusively.
    let unassociated = unsafe { UnassociatedEndpoint::assume_overlapped(owned) };
    let endpoint = port.associate(unassociated, 1).expect("associate");

    // SAFETY: the closure issues exactly one overlapped ConnectNamedPipe using
    // the provided OVERLAPPED pointer and classifies its outcome.
    let submitted = unsafe {
        endpoint.submit(Operation::new(()), |handle, overlapped| {
            let ok = ConnectNamedPipe(handle.as_raw_handle(), overlapped);
            if ok != 0 {
                return Ok(Issued::Pending);
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_IO_PENDING as i32) {
                Ok(Issued::Pending)
            } else {
                Err(error)
            }
        })
    };

    let Submitted::Pending(id) = submitted else {
        panic!("expected a pending connect, no client is present");
    };

    endpoint.cancel(id).expect("cancel");

    let completion = port
        .get(5_000)
        .expect("get")
        .expect("a completion after cancellation");
    assert_eq!(completion.overlapped_ptr(), id.as_ptr());
    assert_eq!(
        completion.error().and_then(io::Error::raw_os_error),
        Some(ERROR_OPERATION_ABORTED as i32)
    );

    // SAFETY: this completion is for the Operation<()> submitted above and is
    // claimed exactly once.
    let operation = unsafe { completion.claim::<()>() };
    assert_eq!(operation.state(), OperationState::Completed);
}

#[test]
fn run_down_drains_a_cancelled_operation() {
    let name = format!(
        r"\\.\pipe\windows-overlapped-io-sys-rundown-{}",
        std::process::id()
    );
    let owned = overlapped_server(&name);

    let port = CompletionPort::new(0).expect("create port");
    // SAFETY: fresh overlapped pipe, unassociated, unique, moved in exclusively.
    let unassociated = unsafe { UnassociatedEndpoint::assume_overlapped(owned) };
    let endpoint = port.associate(unassociated, 1).expect("associate");

    // SAFETY: the closure issues exactly one overlapped ConnectNamedPipe using
    // the provided OVERLAPPED pointer and classifies its outcome.
    let submitted = unsafe {
        endpoint.submit(Operation::new(()), |handle, overlapped| {
            let ok = ConnectNamedPipe(handle.as_raw_handle(), overlapped);
            if ok != 0 {
                return Ok(Issued::Pending);
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_IO_PENDING as i32) {
                Ok(Issued::Pending)
            } else {
                Err(error)
            }
        })
    };
    assert!(matches!(submitted, Submitted::Pending(_)));
    assert_eq!(port.outstanding(), 1);

    // Closing the endpoint cancels its pending operation; run_down then drains
    // the resulting completion and reclaims the storage.
    drop(endpoint);
    port.run_down().expect("run_down");
    assert_eq!(port.outstanding(), 0);
}

/// M1 (PR #20 review response): dropping an `AssociatedEndpoint` must not
/// close its handle out from under a still-outstanding operation. Unlike
/// `run_down_drains_a_cancelled_operation` above, this asserts the drain
/// happens *inside* `drop(endpoint)` itself, with no separate `run_down`
/// call at all.
#[test]
fn dropping_an_endpoint_drains_its_own_outstanding_operation_before_closing() {
    let name = format!(
        r"\\.\pipe\windows-overlapped-io-sys-endpoint-drop-{}",
        std::process::id()
    );
    let owned = overlapped_server(&name);

    let port = CompletionPort::new(0).expect("create port");
    // SAFETY: fresh overlapped pipe, unassociated, unique, moved in exclusively.
    let unassociated = unsafe { UnassociatedEndpoint::assume_overlapped(owned) };
    let endpoint = port.associate(unassociated, 1).expect("associate");

    // SAFETY: the closure issues exactly one overlapped ConnectNamedPipe using
    // the provided OVERLAPPED pointer and classifies its outcome.
    let submitted = unsafe {
        endpoint.submit(Operation::new(()), |handle, overlapped| {
            let ok = ConnectNamedPipe(handle.as_raw_handle(), overlapped);
            if ok != 0 {
                return Ok(Issued::Pending);
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_IO_PENDING as i32) {
                Ok(Issued::Pending)
            } else {
                Err(error)
            }
        })
    };
    assert!(matches!(submitted, Submitted::Pending(_)));
    assert_eq!(endpoint.outstanding(), 1);

    // The whole point under test: this must neither hang nor close the
    // handle while ConnectNamedPipe is still pending against it. A test
    // timeout (not a hang detector) is the only thing that would catch a
    // regression here, since a wedged drop blocks the whole test binary.
    drop(endpoint);

    // The drop above drained its own operation internally, with no explicit
    // run_down call anywhere in this test.
    assert_eq!(port.outstanding(), 0);
}
