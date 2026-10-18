// Copyright (c) 2026 Mike Grier
//! Tests for `connect_named_pipe`, `connect_named_pipe_overlapped` and `disconnect_named_pipe`.
//!
//! What is asserted is what the two pages say, `ms.date` 12/05/2018. A test that depends on an error
//! code says which sentence of the page names it.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsHandle, AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    ERROR_NO_DATA, ERROR_OPERATION_ABORTED, ERROR_PIPE_LISTENING, FALSE, TRUE, WAIT_OBJECT_0,
};
use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OVERLAPPED;
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

use super::{
    ConnectOutcome, ConnectStart, connect_named_pipe, connect_named_pipe_overlapped,
    disconnect_named_pipe,
};
use crate::{NamedPipeOptions, PipeMode, PipeOpenMode, create_named_pipe};

const FILE_SHARE_ALL: u32 = 7;

/// How long a client waits before connecting, so that the connect is already waiting when it does.
const CLIENT_DELAY: Duration = Duration::from_millis(200);

/// How long a test waits for something it has already caused.
const WAIT_MS: u32 = 5_000;

fn unique_name(tag: &str) -> String {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    format!(
        r"\\.\pipe\win-pipes-sys-connect-test-{}-{tag}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

fn pipe(name: &str, open_mode: PipeOpenMode, pipe_mode: PipeMode) -> OwnedHandle {
    let options = NamedPipeOptions {
        open_mode,
        pipe_mode,
        max_instances: 1,
        out_buffer_size: 4096,
        in_buffer_size: 4096,
        default_timeout_ms: 0,
    };
    create_named_pipe(name, &options, None).expect("create the pipe")
}

fn blocking_pipe(name: &str) -> OwnedHandle {
    pipe(
        name,
        PipeOpenMode::ACCESS_DUPLEX,
        PipeMode::TYPE_BYTE | PipeMode::WAIT,
    )
}

fn overlapped_pipe(name: &str) -> OwnedHandle {
    pipe(
        name,
        PipeOpenMode::ACCESS_DUPLEX | PipeOpenMode::OVERLAPPED,
        PipeMode::TYPE_BYTE | PipeMode::WAIT,
    )
}

fn client(name: &str) -> File {
    OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(FILE_SHARE_ALL)
        .open(name)
        .expect("a client connects")
}

fn code(error: &std::io::Error) -> u32 {
    u32::try_from(error.raw_os_error().expect("an OS error")).expect("a code that fits")
}

/// A manual-reset event, unsignalled: what the page says an overlapped connect's `OVERLAPPED` should
/// hold.
fn manual_reset_event() -> OwnedHandle {
    // SAFETY: no attributes, a manual-reset, initially unsignalled, unnamed event.
    let event = unsafe { CreateEventW(std::ptr::null(), TRUE, FALSE, std::ptr::null()) };
    assert!(!event.is_null(), "an event is made");
    // SAFETY: a handle this call owns and nothing else does.
    unsafe { OwnedHandle::from_raw_handle(event) }
}

fn overlapped_with(event: &OwnedHandle) -> OVERLAPPED {
    // SAFETY: all-zero is a valid `OVERLAPPED`.
    let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
    overlapped.hEvent = event.as_raw_handle();
    overlapped
}

// --- The synchronous connect ------------------------------------------------------------------

#[test]
fn a_client_that_connected_first_is_reported_as_already_connected() {
    let name = unique_name("first");
    let server = blocking_pipe(&name);
    let _client = client(&name);

    assert_eq!(
        connect_named_pipe(&server).expect("a good connection"),
        ConnectOutcome::AlreadyConnected
    );
}

#[test]
fn a_client_that_connects_after_the_call_is_a_success() {
    let name = unique_name("after");
    let server = blocking_pipe(&name);

    let connecting = {
        let name = name.clone();
        thread::spawn(move || {
            thread::sleep(CLIENT_DELAY);
            client(&name)
        })
    };

    assert_eq!(
        connect_named_pipe(&server).expect("the client arrives"),
        ConnectOutcome::Succeeded
    );
    connecting.join().expect("the client thread");
}

#[test]
fn connecting_again_with_the_client_still_open_is_already_connected() {
    // The page: "ConnectNamedPipe returns zero, and GetLastError returns ... ERROR_PIPE_CONNECTED if
    // [the previous client] has not closed its handle."
    let name = unique_name("again");
    let server = blocking_pipe(&name);
    let _client = client(&name);

    assert_eq!(
        connect_named_pipe(&server).expect("the first"),
        ConnectOutcome::AlreadyConnected
    );
    assert_eq!(
        connect_named_pipe(&server).expect("the second"),
        ConnectOutcome::AlreadyConnected
    );
}

#[test]
fn a_client_that_closed_is_no_data_until_the_server_disconnects() {
    // The page: "ConnectNamedPipe returns zero, and GetLastError returns ERROR_NO_DATA if the previous
    // client has closed its handle", and the server "must call DisconnectNamedPipe" first.
    let name = unique_name("reuse");
    let server = blocking_pipe(&name);
    let first = client(&name);
    assert_eq!(
        connect_named_pipe(&server).expect("the first client"),
        ConnectOutcome::AlreadyConnected
    );
    drop(first);

    let error = connect_named_pipe(&server).expect_err("the previous client has closed");
    assert_eq!(code(&error), ERROR_NO_DATA);

    disconnect_named_pipe(&server).expect("disconnect");

    // A disconnected instance takes a client only once the server is connecting again: a client that
    // opens before is told every instance is busy.
    let connecting = {
        let name = name.clone();
        thread::spawn(move || {
            thread::sleep(CLIENT_DELAY);
            client(&name)
        })
    };
    assert_eq!(
        connect_named_pipe(&server).expect("the second client"),
        ConnectOutcome::Succeeded
    );
    connecting.join().expect("the second client's thread");
}

#[test]
fn a_nonblocking_pipe_with_no_client_is_listening_and_with_one_is_already_connected() {
    // The page: in nonblocking mode "GetLastError returns ERROR_PIPE_LISTENING if no client is
    // connected, ERROR_PIPE_CONNECTED if a client is connected".
    let name = unique_name("nowait");
    let server = pipe(
        &name,
        PipeOpenMode::ACCESS_DUPLEX,
        PipeMode::TYPE_BYTE | PipeMode::NOWAIT,
    );

    let error = connect_named_pipe(&server).expect_err("no client is connected");
    assert_eq!(code(&error), ERROR_PIPE_LISTENING);

    let _client = client(&name);
    assert_eq!(
        connect_named_pipe(&server).expect("a client is connected"),
        ConnectOutcome::AlreadyConnected
    );
}

#[test]
fn a_handle_that_is_not_a_pipe_is_refused_by_the_call() {
    let path =
        std::env::temp_dir().join(format!("win-pipes-sys-connect-{}.tmp", std::process::id()));
    let file = File::create(&path).expect("create a file");

    let connect = connect_named_pipe(&file).expect_err("a file is not a pipe");
    assert!(connect.raw_os_error().is_some(), "{connect}");
    let disconnect = disconnect_named_pipe(&file).expect_err("a file is not a pipe");
    assert!(disconnect.raw_os_error().is_some(), "{disconnect}");

    drop(file);
    std::fs::remove_file(&path).expect("remove the file");
}

// --- The disconnect ---------------------------------------------------------------------------

#[test]
fn a_disconnected_client_fails_on_its_next_access() {
    let name = unique_name("disconnect");
    let server = blocking_pipe(&name);
    let mut client = client(&name);
    client
        .write_all(b"before")
        .expect("a connected client writes");

    disconnect_named_pipe(&server).expect("disconnect");

    assert!(
        client.write_all(b"after").is_err(),
        "the client was forced off"
    );
}

// --- The overlapped connect -------------------------------------------------------------------

#[test]
fn an_overlapped_connect_with_no_client_pends_and_completes_when_one_connects() {
    let name = unique_name("pending");
    let server = overlapped_pipe(&name);
    let event = manual_reset_event();
    let mut overlapped = overlapped_with(&event);

    // SAFETY: `overlapped` outlives the operation, which is waited for below before it goes out of
    // scope, and the pipe was created overlapped and stays open.
    let started = unsafe { connect_named_pipe_overlapped(server.as_handle(), &raw mut overlapped) };
    assert_eq!(started.expect("start"), ConnectStart::Pending);

    let _client = client(&name);

    // SAFETY: the event is open.
    let waited = unsafe { WaitForSingleObject(event.as_raw_handle(), WAIT_MS) };
    assert_eq!(waited, WAIT_OBJECT_0, "the connect completed");
    let mut transferred = 0u32;
    // SAFETY: the operation has completed, so `overlapped` is no longer written to.
    let completed = unsafe {
        GetOverlappedResult(
            server.as_raw_handle(),
            &raw const overlapped,
            &mut transferred,
            FALSE,
        )
    };
    assert_ne!(completed, FALSE, "the connect succeeded");
}

#[test]
fn an_overlapped_connect_with_a_client_already_there_finishes_at_once() {
    // The page: "If a client connects before the function is called, the function returns zero and
    // GetLastError returns ERROR_PIPE_CONNECTED."
    let name = unique_name("overlapped-first");
    let server = overlapped_pipe(&name);
    let _client = client(&name);
    let event = manual_reset_event();
    let mut overlapped = overlapped_with(&event);

    // SAFETY: the operation finishes without pending, so `overlapped` is not used after the call; the
    // pipe was created overlapped and stays open.
    let started = unsafe { connect_named_pipe_overlapped(server.as_handle(), &raw mut overlapped) };

    assert_eq!(
        started.expect("start"),
        ConnectStart::Finished(ConnectOutcome::AlreadyConnected)
    );
}

#[test]
fn a_pending_overlapped_connect_is_cancelled_and_completes_as_aborted() {
    let name = unique_name("cancel");
    let server = overlapped_pipe(&name);
    let event = manual_reset_event();
    let mut overlapped = overlapped_with(&event);

    // SAFETY: `overlapped` outlives the operation, which is cancelled and waited for below, and the
    // pipe was created overlapped and stays open.
    let started = unsafe { connect_named_pipe_overlapped(server.as_handle(), &raw mut overlapped) };
    assert_eq!(started.expect("start"), ConnectStart::Pending);

    let mut transferred = 0u32;
    // SAFETY: the cancel names the one operation, and the wait is to its completion.
    let completed = unsafe {
        assert_ne!(
            CancelIoEx(server.as_raw_handle(), &raw const overlapped),
            FALSE
        );
        GetOverlappedResult(
            server.as_raw_handle(),
            &raw const overlapped,
            &mut transferred,
            TRUE,
        )
    };
    assert_eq!(completed, FALSE, "a cancelled connect does not succeed");
    assert_eq!(
        code(&std::io::Error::last_os_error()),
        ERROR_OPERATION_ABORTED
    );
}

#[test]
fn an_overlapped_connect_on_a_handle_that_is_not_a_pipe_is_refused_by_the_call() {
    let path = std::env::temp_dir().join(format!(
        "win-pipes-sys-connect-overlapped-{}.tmp",
        std::process::id()
    ));
    std::fs::write(&path, b"x").expect("create a file");
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OVERLAPPED)
        .open(&path)
        .expect("open the file overlapped");
    let event = manual_reset_event();
    let mut overlapped = overlapped_with(&event);

    // SAFETY: the call is refused without starting an operation, so `overlapped` is not used after
    // it; the handle is open.
    let error = unsafe { connect_named_pipe_overlapped(file.as_handle(), &raw mut overlapped) }
        .expect_err("a file is not a pipe");
    assert!(error.raw_os_error().is_some(), "{error}");

    drop(file);
    std::fs::remove_file(&path).expect("remove the file");
}

#[test]
fn the_outcomes_are_distinct_and_comparable() {
    assert_ne!(ConnectOutcome::Succeeded, ConnectOutcome::AlreadyConnected);
    assert_ne!(
        ConnectStart::Pending,
        ConnectStart::Finished(ConnectOutcome::Succeeded)
    );
    assert_ne!(
        ConnectStart::Finished(ConnectOutcome::Succeeded),
        ConnectStart::Finished(ConnectOutcome::AlreadyConnected)
    );
}
