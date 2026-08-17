// Copyright (c) 2026 Mike Grier
//! End-to-end raw-IOCP test against a real, cancellable overlapped device.
//!
//! A named-pipe server with no client is used because `ConnectNamedPipe`
//! reliably pends and is cancellable with `CancelIoEx`, without touching the
//! real filesystem. This exercises association, the pending-submission path,
//! targeted cancellation, completion delivery, and reclamation end to end.

#![cfg(windows)]

use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

use windows_overlapped_io_sys::{
    CompletionPort, Operation, OperationState, Submitted, UnassociatedEndpoint,
};
use windows_sys::Win32::Foundation::{
    ERROR_IO_PENDING, ERROR_OPERATION_ABORTED, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_TYPE_BYTE, PIPE_WAIT,
};

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[test]
fn connect_named_pipe_is_cancellable_and_completes_as_aborted() {
    let name = format!(r"\\.\pipe\windows-overlapped-io-sys-{}", std::process::id());
    let wide_name = wide(&name);

    // SAFETY: standard creation of an overlapped named-pipe server instance.
    let raw = unsafe {
        CreateNamedPipeW(
            wide_name.as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
            PIPE_TYPE_BYTE | PIPE_WAIT,
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
        "CreateNamedPipeW failed: {}",
        io::Error::last_os_error()
    );
    // SAFETY: CreateNamedPipeW returned a fresh, exclusively owned handle.
    let owned = unsafe { OwnedHandle::from_raw_handle(raw) };

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
                return Ok(());
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_IO_PENDING as i32) {
                Ok(())
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
    let wide_name = wide(&name);

    // SAFETY: standard creation of an overlapped named-pipe server instance.
    let raw = unsafe {
        CreateNamedPipeW(
            wide_name.as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
            PIPE_TYPE_BYTE | PIPE_WAIT,
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
        "CreateNamedPipeW failed: {}",
        io::Error::last_os_error()
    );
    // SAFETY: CreateNamedPipeW returned a fresh, exclusively owned handle.
    let owned = unsafe { OwnedHandle::from_raw_handle(raw) };

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
                return Ok(());
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_IO_PENDING as i32) {
                Ok(())
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
