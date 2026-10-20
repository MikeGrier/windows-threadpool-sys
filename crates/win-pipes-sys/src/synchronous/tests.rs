// Copyright (c) Mike Grier.
//! Tests for `SynchronousPipe` and `PipeClientOptions`.

use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::os::windows::io::{AsHandle, AsRawHandle, OwnedHandle};
use std::sync::atomic::{AtomicU32, Ordering};

use win_authorization_sys::SecurityAttributes;
use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY, GENERIC_READ, GENERIC_WRITE,
    GetHandleInformation, HANDLE_FLAG_INHERIT,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_NORMAL, FILE_FLAG_OVERLAPPED, FILE_FLAG_WRITE_THROUGH,
};

use super::{PipeClientOptions, SynchronousPipe};
use crate::{
    ConnectOutcome, NamedPipeOptions, PipeMode, PipeOpenMode, PipeTimeout, connect_named_pipe,
    create_pipe, wait_named_pipe,
};

const FILE_SHARE_ALL: u32 = 7;

fn unique_name(tag: &str) -> String {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    format!(
        r"\\.\pipe\win-pipes-sys-synchronous-test-{}-{tag}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

fn options(open_mode: PipeOpenMode) -> NamedPipeOptions {
    NamedPipeOptions {
        open_mode,
        pipe_mode: PipeMode::TYPE_BYTE | PipeMode::WAIT,
        max_instances: 1,
        out_buffer_size: 4096,
        in_buffer_size: 4096,
        default_timeout_ms: 0,
    }
}

fn client_options(flags_and_attributes: u32) -> PipeClientOptions {
    PipeClientOptions {
        desired_access: GENERIC_READ | GENERIC_WRITE,
        share_mode: FILE_SHARE_ALL,
        flags_and_attributes,
    }
}

fn code(error: &std::io::Error) -> u32 {
    u32::try_from(error.raw_os_error().expect("an OS error")).expect("a code that fits")
}

fn assert_refused_by_the_crate(error: &std::io::Error) {
    assert_eq!(error.kind(), ErrorKind::InvalidInput, "{error}");
    assert_eq!(error.raw_os_error(), None, "the call was not made: {error}");
}

fn inheritable(handle: &impl AsHandle) -> bool {
    let mut flags = 0u32;
    // SAFETY: the handle is open for the borrow and `flags` is a valid out-pointer.
    let read = unsafe { GetHandleInformation(handle.as_handle().as_raw_handle(), &raw mut flags) };
    assert_ne!(read, 0, "{}", std::io::Error::last_os_error());
    flags & HANDLE_FLAG_INHERIT != 0
}

// --- create ---------------------------------------------------------------------------------

#[test]
fn create_refuses_an_overlapped_open_mode_before_the_call() {
    for open_mode in [
        PipeOpenMode::ACCESS_DUPLEX | PipeOpenMode::OVERLAPPED,
        PipeOpenMode::ACCESS_INBOUND | PipeOpenMode::OVERLAPPED,
        PipeOpenMode::ACCESS_OUTBOUND | PipeOpenMode::OVERLAPPED,
        PipeOpenMode::ACCESS_DUPLEX | PipeOpenMode::OVERLAPPED | PipeOpenMode::WRITE_THROUGH,
        PipeOpenMode::ACCESS_DUPLEX | PipeOpenMode::OVERLAPPED | PipeOpenMode::FIRST_PIPE_INSTANCE,
    ] {
        let name = unique_name("create-refused");
        let error = SynchronousPipe::create(&name, &options(open_mode), None)
            .expect_err("an overlapped open mode");
        assert_refused_by_the_crate(&error);
        let missing = wait_named_pipe(&name, PipeTimeout::NO_WAIT).expect_err("no instance");
        assert_eq!(code(&missing), ERROR_FILE_NOT_FOUND, "no pipe was created");
    }
}

#[test]
fn create_takes_every_open_mode_without_the_overlapped_flag() {
    for open_mode in [
        PipeOpenMode::ACCESS_DUPLEX,
        PipeOpenMode::ACCESS_INBOUND,
        PipeOpenMode::ACCESS_OUTBOUND,
        PipeOpenMode::ACCESS_DUPLEX | PipeOpenMode::WRITE_THROUGH,
        PipeOpenMode::ACCESS_DUPLEX | PipeOpenMode::FIRST_PIPE_INSTANCE,
    ] {
        let name = unique_name("create");
        let _server = SynchronousPipe::create(&name, &options(open_mode), None)
            .unwrap_or_else(|error| panic!("{open_mode:?}: {error}"));
        wait_named_pipe(&name, PipeTimeout::NO_WAIT).expect("the instance exists");
    }
}

#[test]
fn create_returns_the_errors_of_the_call() {
    // A second first instance: the `FILE_FLAG_FIRST_PIPE_INSTANCE` case of `CreateNamedPipeW`.
    let name = unique_name("second-first");
    let first = PipeOpenMode::ACCESS_DUPLEX | PipeOpenMode::FIRST_PIPE_INSTANCE;
    let _server = SynchronousPipe::create(&name, &options(first), None).expect("the first");
    let error = SynchronousPipe::create(&name, &options(first), None).expect_err("not first");
    assert!(
        error.raw_os_error().is_some(),
        "the call's own error: {error}"
    );
}

#[test]
fn create_refuses_a_name_with_an_interior_nul() {
    let error = SynchronousPipe::create(
        "\\\\.\\pipe\\a\0b",
        &options(PipeOpenMode::ACCESS_DUPLEX),
        None,
    )
    .expect_err("a NUL");
    assert_refused_by_the_crate(&error);
}

// --- open_client ----------------------------------------------------------------------------

#[test]
fn open_client_refuses_the_overlapped_flag_before_the_call() {
    let name = unique_name("client-refused");
    let _server = SynchronousPipe::create(&name, &options(PipeOpenMode::ACCESS_DUPLEX), None)
        .expect("a pipe");
    for flags in [
        FILE_FLAG_OVERLAPPED,
        FILE_FLAG_OVERLAPPED | FILE_ATTRIBUTE_NORMAL,
        FILE_FLAG_OVERLAPPED | FILE_FLAG_WRITE_THROUGH,
    ] {
        let error = SynchronousPipe::open_client(&name, &client_options(flags), None)
            .expect_err("the overlapped flag");
        assert_refused_by_the_crate(&error);
    }
    // The pipe's one instance was not taken by a refused open.
    let _client = SynchronousPipe::open_client(&name, &client_options(0), None)
        .expect("the instance is still free");
}

#[test]
fn open_client_takes_other_flags_and_attributes() {
    for flags in [0, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_WRITE_THROUGH] {
        let name = unique_name("client-flags");
        let _server = SynchronousPipe::create(&name, &options(PipeOpenMode::ACCESS_DUPLEX), None)
            .expect("a pipe");
        let _client = SynchronousPipe::open_client(&name, &client_options(flags), None)
            .unwrap_or_else(|error| panic!("{flags:#x}: {error}"));
    }
}

#[test]
fn open_client_refuses_a_name_with_an_interior_nul() {
    let error = SynchronousPipe::open_client("\\\\.\\pipe\\a\0b", &client_options(0), None)
        .expect_err("a NUL");
    assert_refused_by_the_crate(&error);
}

#[test]
fn open_client_of_a_name_with_no_instance_is_the_calls_error() {
    let error = SynchronousPipe::open_client(unique_name("none"), &client_options(0), None)
        .expect_err("no instance");
    assert_eq!(code(&error), ERROR_FILE_NOT_FOUND);
}

#[test]
fn open_client_when_every_instance_is_connected_is_pipe_busy() {
    let name = unique_name("busy");
    let _server = SynchronousPipe::create(&name, &options(PipeOpenMode::ACCESS_DUPLEX), None)
        .expect("a pipe");
    let _first = SynchronousPipe::open_client(&name, &client_options(0), None).expect("first");
    let error = SynchronousPipe::open_client(&name, &client_options(0), None)
        .expect_err("the one instance is taken");
    assert_eq!(code(&error), ERROR_PIPE_BUSY);
}

#[test]
fn open_client_passes_the_desired_access() {
    // A client opened for reading only cannot write: the access was the one given.
    let name = unique_name("access");
    let _server = SynchronousPipe::create(&name, &options(PipeOpenMode::ACCESS_DUPLEX), None)
        .expect("a pipe");
    let read_only = PipeClientOptions {
        desired_access: GENERIC_READ,
        ..client_options(0)
    };
    let client = SynchronousPipe::open_client(&name, &read_only, None).expect("a client");
    let error = File::from(client.into_owned())
        .write_all(b"x")
        .expect_err("no write access");
    assert_eq!(code(&error), ERROR_ACCESS_DENIED);
}

#[test]
fn open_client_passes_the_security_attributes() {
    for inherit in [false, true] {
        let name = unique_name("inherit");
        let _server = SynchronousPipe::create(&name, &options(PipeOpenMode::ACCESS_DUPLEX), None)
            .expect("a pipe");
        let security = SecurityAttributes::new(None, inherit);
        let client = SynchronousPipe::open_client(&name, &client_options(0), Some(&security))
            .expect("a client");
        assert_eq!(inheritable(&client), inherit);
    }
    let name = unique_name("no-security");
    let _server = SynchronousPipe::create(&name, &options(PipeOpenMode::ACCESS_DUPLEX), None)
        .expect("a pipe");
    let client = SynchronousPipe::open_client(&name, &client_options(0), None).expect("a client");
    assert!(!inheritable(&client), "no attributes: not inheritable");
}

// --- the two ends together ------------------------------------------------------------------

#[test]
fn a_created_server_and_an_opened_client_carry_data_both_ways() {
    let name = unique_name("data");
    let server = SynchronousPipe::create(&name, &options(PipeOpenMode::ACCESS_DUPLEX), None)
        .expect("a pipe");
    let client = SynchronousPipe::open_client(&name, &client_options(0), None).expect("a client");
    assert_eq!(
        connect_named_pipe(&server).expect("connect"),
        ConnectOutcome::AlreadyConnected
    );
    let mut server = File::from(server.into_owned());
    let mut client = File::from(OwnedHandle::from(client));
    client.write_all(b"ping").expect("client write");
    let mut received = [0u8; 4];
    server.read_exact(&mut received).expect("server read");
    assert_eq!(&received, b"ping");
    server.write_all(b"pong").expect("server write");
    client.read_exact(&mut received).expect("client read");
    assert_eq!(&received, b"pong");
}

#[test]
fn a_clone_is_a_second_handle_to_the_same_end() {
    let name = unique_name("clone");
    let server = SynchronousPipe::create(&name, &options(PipeOpenMode::ACCESS_DUPLEX), None)
        .expect("a pipe");
    let client = SynchronousPipe::open_client(&name, &client_options(0), None).expect("a client");
    let clone = client.try_clone().expect("a clone");
    assert_ne!(
        clone.as_handle().as_raw_handle(),
        client.as_handle().as_raw_handle()
    );
    drop(client);
    let mut server = File::from(server.into_owned());
    File::from(clone.into_owned())
        .write_all(b"via the clone")
        .expect("the clone writes");
    let mut received = [0u8; 13];
    server.read_exact(&mut received).expect("server read");
    assert_eq!(&received, b"via the clone");
}

#[test]
fn the_handle_given_out_is_the_one_held() {
    let name = unique_name("owned");
    let server = SynchronousPipe::create(&name, &options(PipeOpenMode::ACCESS_DUPLEX), None)
        .expect("a pipe");
    let raw = server.as_handle().as_raw_handle();
    assert_eq!(server.into_owned().as_raw_handle(), raw);

    let client_name = unique_name("owned-from");
    let _server =
        SynchronousPipe::create(&client_name, &options(PipeOpenMode::ACCESS_DUPLEX), None)
            .expect("a pipe");
    let client =
        SynchronousPipe::open_client(&client_name, &client_options(0), None).expect("a client");
    let raw = client.as_handle().as_raw_handle();
    assert_eq!(OwnedHandle::from(client).as_raw_handle(), raw);
}

#[test]
fn an_anonymous_pipe_end_is_taken_unchecked() {
    let pipe = create_pipe(0, None).expect("a pipe");
    let raw = pipe.write.as_raw_handle();
    // SAFETY: an anonymous pipe's end. "Anonymous Pipe Operations": "Asynchronous (overlapped) read
    // and write operations are not supported by anonymous pipes."
    let write = unsafe { SynchronousPipe::from_owned_unchecked(pipe.write) };
    assert_eq!(write.as_handle().as_raw_handle(), raw);
    File::from(write.into_owned())
        .write_all(b"anon")
        .expect("write");
    let mut received = [0u8; 4];
    File::from(pipe.read)
        .read_exact(&mut received)
        .expect("read");
    assert_eq!(&received, b"anon");
}
