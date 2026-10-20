// Copyright (c) 2026 Mike Grier
//! Tests for `PipeTimeout` and `wait_named_pipe`.
//!
//! What is asserted is what the page says, `ms.date` 08/05/2022. A test of something the page does not
//! say, or says otherwise than what was observed, is named an observation. Timing assertions are lower
//! bounds only, on a wait that must have elapsed.

use std::fs::{File, OpenOptions};
use std::os::windows::fs::OpenOptionsExt;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SEM_TIMEOUT};

use super::{PipeTimeout, wait_named_pipe};
use crate::{
    NamedPipeOptions, PipeMode, PipeOpenMode, SynchronousPipe, connect_named_pipe,
    create_named_pipe, disconnect_named_pipe,
};

const FILE_SHARE_ALL: u32 = 7;

fn unique_name(tag: &str) -> String {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    format!(
        r"\\.\pipe\win-pipes-sys-wait-test-{}-{tag}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

fn options(default_timeout_ms: u32) -> NamedPipeOptions {
    NamedPipeOptions {
        open_mode: PipeOpenMode::ACCESS_DUPLEX,
        pipe_mode: PipeMode::TYPE_BYTE | PipeMode::WAIT,
        max_instances: 1,
        out_buffer_size: 4096,
        in_buffer_size: 4096,
        default_timeout_ms,
    }
}

fn code(error: &std::io::Error) -> u32 {
    u32::try_from(error.raw_os_error().expect("an OS error")).expect("a code that fits")
}

fn open_client(name: &str) -> File {
    OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(FILE_SHARE_ALL)
        .open(name)
        .expect("a client")
}

/// A pipe with its one instance connected to a client, so that no instance is available.
fn busy_pipe(default_timeout_ms: u32) -> (String, SynchronousPipe, File) {
    let name = unique_name("busy");
    let server =
        SynchronousPipe::create(&name, &options(default_timeout_ms), None).expect("a pipe");
    let client = open_client(&name);
    (name, server, client)
}

#[test]
fn the_timeouts_are_the_pages_values() {
    assert_eq!(PipeTimeout::USE_DEFAULT.millis(), 0x0000_0000);
    assert_eq!(PipeTimeout::FOREVER.millis(), 0xffff_ffff);
    assert_eq!(PipeTimeout::NO_WAIT.millis(), 0x0000_0001);
}

#[test]
fn a_number_of_milliseconds_is_passed_as_it_is() {
    for milliseconds in [0u32, 1, 2, 10, 250, 1000, 60_000, u32::MAX - 1, u32::MAX] {
        assert_eq!(
            PipeTimeout::from_millis(milliseconds).millis(),
            milliseconds
        );
    }
}

#[test]
fn the_special_values_share_their_numbers_with_milliseconds() {
    // The values are the pages' and are in the same u32 as the milliseconds, so one millisecond is the
    // `CallNamedPipeW` page's "does not wait", and zero and `u32::MAX` are its other two.
    assert_eq!(PipeTimeout::from_millis(1), PipeTimeout::NO_WAIT);
    assert_eq!(PipeTimeout::from_millis(0), PipeTimeout::USE_DEFAULT);
    assert_eq!(PipeTimeout::from_millis(u32::MAX), PipeTimeout::FOREVER);
    assert_eq!(PipeTimeout::default(), PipeTimeout::USE_DEFAULT);
}

#[test]
fn a_name_with_no_instance_returns_at_once_whatever_the_timeout() {
    // The page: "If no instances of the specified named pipe exist, the WaitNamedPipe function returns
    // immediately, regardless of the time-out value." Forever is the value that would hang.
    for timeout in [
        PipeTimeout::USE_DEFAULT,
        PipeTimeout::from_millis(5_000),
        PipeTimeout::FOREVER,
    ] {
        let started = Instant::now();
        let error = wait_named_pipe(unique_name("none"), timeout).expect_err("no instance");
        assert_eq!(code(&error), ERROR_FILE_NOT_FOUND, "{timeout:?}");
        assert!(started.elapsed() < Duration::from_secs(4), "{timeout:?}");
    }
}

#[test]
fn a_busy_instance_times_out_with_sem_timeout_after_the_time() {
    // The page: "If the time-out interval expires, the WaitNamedPipe function will fail with the error
    // ERROR_SEM_TIMEOUT."
    let (name, _server, _client) = busy_pipe(0);
    for milliseconds in [50u32, 150, 300] {
        let started = Instant::now();
        let error = wait_named_pipe(&name, PipeTimeout::from_millis(milliseconds))
            .expect_err("no instance is available");
        assert_eq!(code(&error), ERROR_SEM_TIMEOUT);
        assert!(
            started.elapsed() >= Duration::from_millis(u64::from(milliseconds) * 2 / 3),
            "{milliseconds} ms: {:?}",
            started.elapsed()
        );
    }
}

#[test]
fn the_default_timeout_is_the_one_the_server_gave() {
    // The page: "NMPWAIT_USE_DEFAULT_WAIT ... The time-out interval is the default value specified by
    // the server process in the CreateNamedPipe function."
    let (name, _server, _client) = busy_pipe(300);
    let started = Instant::now();
    let error = wait_named_pipe(&name, PipeTimeout::USE_DEFAULT).expect_err("no instance");
    assert_eq!(code(&error), ERROR_SEM_TIMEOUT);
    assert!(started.elapsed() >= Duration::from_millis(200));
}

#[test]
fn a_waiter_is_released_when_an_instance_becomes_available() {
    // The order is made by the test rather than by sleeps: the instance is shown busy after its client
    // has gone, and only then is the server told to disconnect it and connect again, so the wait that
    // follows is released by that and nothing else.
    let (name, server, client) = busy_pipe(0);
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let server_thread = thread::spawn(move || {
        release_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the test releases the instance");
        disconnect_named_pipe(&server).expect("disconnect");
        connect_named_pipe(&server).expect("connect");
        server
    });
    drop(client);

    let busy = wait_named_pipe(&name, PipeTimeout::from_millis(50))
        .expect_err("the instance is not disconnected yet");
    assert_eq!(code(&busy), ERROR_SEM_TIMEOUT);

    release_tx.send(()).expect("the server thread waits");
    wait_named_pipe(&name, PipeTimeout::from_millis(5_000)).expect("an instance became available");

    // Let the server's connect finish, so that its thread does.
    let _reconnected = open_client(&name);
    let _server = server_thread.join().expect("the server thread");
}

#[test]
fn an_instance_that_has_not_started_to_connect_is_available_to_the_call() {
    // Observed, against the page, which describes available as "the pipe's server process has a
    // pending ConnectNamedPipe operation on the pipe": an instance that has been created and is not
    // connected to a client was reported available at once, before any connect was made.
    let name = unique_name("idle");
    let _server = create_named_pipe(&name, &options(0), None).expect("a pipe");
    wait_named_pipe(&name, PipeTimeout::from_millis(100)).expect("available");
}

#[test]
fn a_name_with_an_interior_nul_is_refused_before_the_call() {
    let error = wait_named_pipe("\\\\.\\pipe\\a\0b", PipeTimeout::FOREVER).expect_err("a NUL");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    assert_eq!(error.raw_os_error(), None, "the call was not made");
}

#[test]
fn success_is_not_a_promise_that_the_next_open_succeeds() {
    // The page: "A subsequent CreateFile call to the pipe can fail, because the instance was closed by
    // the server or opened by another client." The two clients here are the second case: the wait
    // succeeds for both while there is one instance, and only one open can have it.
    let name = unique_name("race");
    let _server = create_named_pipe(&name, &options(0), None).expect("a pipe");
    wait_named_pipe(&name, PipeTimeout::from_millis(100)).expect("available to the first");
    wait_named_pipe(&name, PipeTimeout::from_millis(100)).expect("available to the second");
    let _first = open_client(&name);
    assert!(
        OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(FILE_SHARE_ALL)
            .open(&name)
            .is_err(),
        "the one instance is taken"
    );
}
