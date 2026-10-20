// Copyright (c) 2026 Mike Grier
//! Tests for `transact_named_pipe`, `transact_named_pipe_overlapped` and `call_named_pipe`.
//!
//! What is asserted is what the two pages say, `ms.date` 12/05/2018 and 04/19/2021. A test of something
//! they do not say is named an observation. Timing assertions are lower bounds only.

use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsHandle, OwnedHandle};
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_PIPE_BUSY, FALSE, GENERIC_READ, GENERIC_WRITE,
    TRUE, WAIT_OBJECT_0,
};
use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OVERLAPPED;
use windows_sys::Win32::System::IO::{GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

use super::{
    TransactStart, Transaction, call_named_pipe, input_len, output_len, transact_named_pipe,
    transact_named_pipe_overlapped,
};
use crate::{
    NamedPipeOptions, PipeClientOptions, PipeMode, PipeOpenMode, PipeStateUpdate, PipeTimeout,
    SynchronousPipe, connect_named_pipe, create_named_pipe, get_named_pipe_info,
    set_named_pipe_handle_state,
};

use std::os::windows::io::FromRawHandle;

const FILE_SHARE_ALL: u32 = 7;
const WAIT_MS: u32 = 5_000;

fn unique_name(tag: &str) -> String {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    format!(
        r"\\.\pipe\win-pipes-sys-transact-test-{}-{tag}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

fn message_options() -> NamedPipeOptions {
    NamedPipeOptions {
        open_mode: PipeOpenMode::ACCESS_DUPLEX,
        pipe_mode: PipeMode::TYPE_MESSAGE | PipeMode::READMODE_MESSAGE | PipeMode::WAIT,
        max_instances: 4,
        out_buffer_size: 4096,
        in_buffer_size: 4096,
        default_timeout_ms: 0,
    }
}

fn byte_options() -> NamedPipeOptions {
    NamedPipeOptions {
        pipe_mode: PipeMode::TYPE_BYTE | PipeMode::READMODE_BYTE | PipeMode::WAIT,
        ..message_options()
    }
}

fn set_mode(pipe: &impl AsHandle, mode: PipeMode) -> std::io::Result<()> {
    set_named_pipe_handle_state(
        pipe,
        &PipeStateUpdate {
            mode: Some(mode),
            ..PipeStateUpdate::default()
        },
    )
}

fn code(error: &std::io::Error) -> u32 {
    u32::try_from(error.raw_os_error().expect("an OS error")).expect("a code that fits")
}

/// A message pipe with its client open, in message-read mode: the server end and the client end.
/// A local client opens in byte mode, so the mode is set, as the `TransactNamedPipe` page says.
fn connected_pair(name: &str, flags: u32) -> (OwnedHandle, File) {
    let server = create_named_pipe(name, &message_options(), None).expect("a pipe");
    let client = OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(FILE_SHARE_ALL)
        .custom_flags(flags)
        .open(name)
        .expect("a client");
    set_mode(&client, PipeMode::READMODE_MESSAGE).expect("message-read mode");
    (server, client)
}

/// A client end the synchronous transaction takes: opened without `FILE_FLAG_OVERLAPPED`.
fn synchronous_client(name: &str) -> SynchronousPipe {
    SynchronousPipe::open_client(
        name,
        &PipeClientOptions {
            desired_access: GENERIC_READ | GENERIC_WRITE,
            share_mode: FILE_SHARE_ALL,
            flags_and_attributes: 0,
        },
        None,
    )
    .expect("a client")
}

/// As [`connected_pair`], with the client a [`SynchronousPipe`].
fn synchronous_pair(name: &str) -> (OwnedHandle, SynchronousPipe) {
    let server = create_named_pipe(name, &message_options(), None).expect("a pipe");
    let client = synchronous_client(name);
    set_mode(&client, PipeMode::READMODE_MESSAGE).expect("message-read mode");
    (server, client)
}

/// A server thread that reads one request and writes `reply(request)` as one message, after `delay`.
fn serve_once(
    server: OwnedHandle,
    delay: Duration,
    reply: impl FnOnce(&[u8]) -> Vec<u8> + Send + 'static,
) -> JoinHandle<File> {
    thread::spawn(move || {
        let mut server = File::from(server);
        let mut request = vec![0u8; 70_000];
        let read = server.read(&mut request).expect("server read");
        thread::sleep(delay);
        server
            .write_all(&reply(&request[..read]))
            .expect("server write");
        server
    })
}

/// A manual-reset event, an `OVERLAPPED` that signals it, and the handle that owns the event.
struct PendingIo {
    overlapped: Box<OVERLAPPED>,
    event: OwnedHandle,
}

impl PendingIo {
    fn new() -> Self {
        // SAFETY: a plain call with no pointers; a null handle is checked.
        let event = unsafe { CreateEventW(ptr::null(), TRUE, FALSE, ptr::null()) };
        assert!(!event.is_null(), "{}", std::io::Error::last_os_error());
        // SAFETY: the event is a new handle that nothing else owns.
        let event = unsafe { OwnedHandle::from_raw_handle(event) };
        // SAFETY: an all-zero OVERLAPPED is the documented initial state.
        let mut overlapped: Box<OVERLAPPED> = Box::new(unsafe { std::mem::zeroed() });
        overlapped.hEvent = std::os::windows::io::AsRawHandle::as_raw_handle(&event);
        Self { overlapped, event }
    }

    fn signalled(&self, milliseconds: u32) -> bool {
        // SAFETY: the event is open.
        unsafe {
            WaitForSingleObject(
                std::os::windows::io::AsRawHandle::as_raw_handle(&self.event),
                milliseconds,
            ) == WAIT_OBJECT_0
        }
    }

    /// The result of the operation: the bytes it transferred and the error, if the operation failed.
    fn result(&mut self, pipe: &impl AsHandle) -> (u32, Option<u32>) {
        let mut transferred = 0u32;
        // SAFETY: the handle is open and the OVERLAPPED is the one the operation was given.
        let ok = unsafe {
            GetOverlappedResult(
                std::os::windows::io::AsRawHandle::as_raw_handle(&pipe.as_handle()),
                &*self.overlapped,
                &mut transferred,
                FALSE,
            )
        };
        let error = (ok == FALSE).then(|| code(&std::io::Error::last_os_error()));
        (transferred, error)
    }
}

// --- The helpers ----------------------------------------------------------------------------

#[test]
fn an_input_length_that_fits_is_passed_and_one_that_does_not_is_refused() {
    for len in [0usize, 1, 2, 4095, 65_536, u32::MAX as usize] {
        assert_eq!(input_len(len).expect("it fits") as usize, len);
    }
    #[cfg(target_pointer_width = "64")]
    for len in [u32::MAX as usize + 1, usize::MAX] {
        let error = input_len(len).expect_err("it does not fit");
        assert_eq!(error.kind(), ErrorKind::InvalidInput);
    }
}

#[test]
fn an_output_length_that_does_not_fit_is_clamped() {
    for len in [0usize, 1, 2, 4095, 65_536, u32::MAX as usize] {
        assert_eq!(output_len(len) as usize, len);
    }
    #[cfg(target_pointer_width = "64")]
    for len in [u32::MAX as usize + 1, usize::MAX] {
        assert_eq!(output_len(len), u32::MAX);
    }
}

// --- TransactNamedPipe ----------------------------------------------------------------------

#[test]
fn a_transaction_writes_the_request_and_reads_the_reply() {
    for size in [1usize, 2, 7, 64, 1000, 4000] {
        let (server, client) = synchronous_pair(&unique_name("roundtrip"));
        let request: Vec<u8> = (0..size).map(|index| (index % 251) as u8).collect();
        let expected: Vec<u8> = request.iter().rev().copied().collect();
        let serving = serve_once(server, Duration::ZERO, |request| {
            request.iter().rev().copied().collect()
        });

        let mut reply = vec![0u8; 5000];
        let done = transact_named_pipe(&client, &request, &mut reply).expect("a transaction");
        assert_eq!(
            done,
            Transaction {
                bytes_read: size,
                more_data: false
            }
        );
        assert_eq!(&reply[..size], &expected[..], "size {size}");
        serving.join().expect("the server");
    }
}

#[test]
fn a_reply_longer_than_the_buffer_is_more_data_and_the_rest_stays_in_the_pipe() {
    // The page: "If the message to be read is longer than the buffer specified by the nOutBufferSize
    // parameter, TransactNamedPipe returns FALSE and the GetLastError function returns
    // ERROR_MORE_DATA. The remainder of the message can be read by a subsequent call to ReadFile".
    let (server, client) = synchronous_pair(&unique_name("more"));
    let serving = serve_once(server, Duration::ZERO, |_| (0..100u8).collect());

    let mut reply = [0u8; 10];
    let done = transact_named_pipe(&client, b"req", &mut reply).expect("a transaction");
    assert!(done.more_data);
    assert_eq!(done.bytes_read, 10, "observed: the buffer was filled");
    assert_eq!(&reply, &(0..10u8).collect::<Vec<_>>()[..]);

    let mut rest = [0u8; 200];
    let read = File::from(client.into_owned())
        .read(&mut rest)
        .expect("the remainder");
    assert_eq!(&rest[..read], &(10..100u8).collect::<Vec<_>>()[..]);
    serving.join().expect("the server");
}

#[test]
fn a_buffer_exactly_the_size_of_the_reply_is_not_more_data() {
    for (buffer, more) in [(50usize, false), (49, true), (51, false)] {
        let (server, client) = synchronous_pair(&unique_name("boundary"));
        let serving = serve_once(server, Duration::ZERO, |_| vec![9u8; 50]);
        let mut reply = vec![0u8; buffer];
        let done = transact_named_pipe(&client, b"r", &mut reply).expect("a transaction");
        assert_eq!(done.more_data, more, "buffer {buffer}");
        assert_eq!(done.bytes_read, buffer.min(50));
        serving.join().expect("the server");
    }
}

#[test]
fn a_handle_in_byte_read_mode_is_the_calls_error() {
    // The page: the call "fails if ... the pipe handle is not in message-read mode". A local client
    // opens in byte mode.
    let name = unique_name("bytemode");
    let _server = create_named_pipe(&name, &message_options(), None).expect("a pipe");
    let client = synchronous_client(&name);
    let error = transact_named_pipe(&client, b"req", &mut [0u8; 16]).expect_err("byte mode");
    assert!(error.raw_os_error().is_some());
}

#[test]
fn a_pipe_that_is_not_a_message_pipe_is_the_calls_error() {
    // The page: the call "fails if the server did not create the pipe as a message-type pipe".
    let name = unique_name("bytepipe");
    let _server = create_named_pipe(&name, &byte_options(), None).expect("a pipe");
    let client = synchronous_client(&name);
    assert!(set_mode(&client, PipeMode::READMODE_MESSAGE).is_err());
    assert!(transact_named_pipe(&client, b"req", &mut [0u8; 16]).is_err());
}

#[test]
fn a_query_on_the_same_pipe_during_a_transaction_does_not_end_it_early() {
    // The hazard the safe form exists to exclude: on a handle opened overlapped, another operation on
    // the handle can make a transaction with no `OVERLAPPED` report completion before its reply, and
    // the reply then arrives in a buffer it has given back. A `SynchronousPipe` is not opened so, and
    // a query made through a duplicate of it while the transaction waits does not end it: the whole
    // reply is read. The server replies only after the query has been started.
    let (server, client) = synchronous_pair(&unique_name("concurrent"));
    let querying = client.try_clone().expect("a duplicate of the client");
    let (started_tx, started_rx) = std::sync::mpsc::channel::<()>();
    let serving = thread::spawn(move || {
        let mut server = File::from(server);
        let mut request = [0u8; 64];
        let read = server.read(&mut request).expect("server read");
        assert_eq!(&request[..read], b"request");
        started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the query started");
        thread::sleep(Duration::from_millis(100));
        server.write_all(b"RESPONSE").expect("server write");
        server
    });
    let query = thread::spawn(move || {
        started_tx.send(()).expect("the server listens");
        get_named_pipe_info(&querying).expect("the pipe's information")
    });

    let mut reply = [0u8; 64];
    let done = transact_named_pipe(&client, b"request", &mut reply).expect("a transaction");

    assert_eq!(
        done,
        Transaction {
            bytes_read: 8,
            more_data: false
        }
    );
    assert_eq!(&reply[..8], b"RESPONSE");
    query.join().expect("the query");
    serving.join().expect("the server");
}

// --- TransactNamedPipe, overlapped ----------------------------------------------------------

#[test]
fn an_overlapped_transaction_pends_until_the_reply_and_then_signals() {
    let (server, client) = connected_pair(&unique_name("overlapped"), FILE_FLAG_OVERLAPPED);
    let serving = serve_once(server, Duration::from_millis(200), |request| {
        request.to_ascii_uppercase()
    });

    let mut io = PendingIo::new();
    let request = *b"overlapped";
    let mut reply = [0u8; 64];
    let started = Instant::now();
    // SAFETY: the buffers and the OVERLAPPED outlive the operation, which this test waits out; the
    // handle was opened overlapped and stays open.
    let start = unsafe {
        transact_named_pipe_overlapped(
            client.as_handle(),
            request.as_ptr(),
            request.len() as u32,
            reply.as_mut_ptr(),
            reply.len() as u32,
            &mut *io.overlapped,
        )
    }
    .expect("a start");

    assert_eq!(start, TransactStart::Pending, "the server has not replied");
    assert!(!io.signalled(0), "pending: the event is not signalled");
    assert!(io.signalled(WAIT_MS), "the reply signals it");
    assert!(started.elapsed() >= Duration::from_millis(150));

    let (transferred, error) = io.result(&client);
    assert_eq!(error, None);
    assert_eq!(&reply[..transferred as usize], b"OVERLAPPED");
    serving.join().expect("the server");
}

#[test]
fn an_overlapped_transaction_with_a_short_buffer_completes_as_more_data() {
    // Whether the call says so at once or when the operation completes is not asserted: the result of
    // the operation is `ERROR_MORE_DATA` either way, with the buffer filled.
    let (server, client) = connected_pair(&unique_name("overlappedmore"), FILE_FLAG_OVERLAPPED);
    let serving = serve_once(server, Duration::from_millis(50), |_| vec![5u8; 100]);

    let mut io = PendingIo::new();
    let mut reply = [0u8; 10];
    // SAFETY: as in the test above.
    let start = unsafe {
        transact_named_pipe_overlapped(
            client.as_handle(),
            b"r".as_ptr(),
            1,
            reply.as_mut_ptr(),
            reply.len() as u32,
            &mut *io.overlapped,
        )
    }
    .expect("a start");
    assert!(matches!(
        start,
        TransactStart::Pending | TransactStart::MoreData
    ));
    assert!(io.signalled(WAIT_MS));
    let (transferred, error) = io.result(&client);
    assert_eq!(error, Some(ERROR_MORE_DATA));
    assert_eq!(transferred, 10);
    assert_eq!(reply, [5u8; 10]);
    serving.join().expect("the server");
}

// --- CallNamedPipeW -------------------------------------------------------------------------

/// A server thread for one call: connect, read one request, write `reply(request)`, and wait for the
/// client to close.
fn serve_call(name: &str, reply: impl FnOnce(&[u8]) -> Vec<u8> + Send + 'static) -> JoinHandle<()> {
    let server = SynchronousPipe::create(name, &message_options(), None).expect("a pipe");
    thread::spawn(move || {
        connect_named_pipe(&server).expect("connect");
        let mut file = File::from(server.into_owned());
        let mut request = vec![0u8; 70_000];
        let read = file.read(&mut request).expect("server read");
        file.write_all(&reply(&request[..read]))
            .expect("server write");
        // Held open until the client has closed its end.
        let _ = file.read(&mut [0u8; 1]);
    })
}

#[test]
fn a_call_connects_writes_reads_and_closes() {
    for size in [1usize, 5, 64, 1000, 4000] {
        let name = unique_name("call");
        let serving = serve_call(&name, |request| request.to_ascii_uppercase());
        let request: Vec<u8> = (0..size).map(|index| b'a' + (index % 26) as u8).collect();
        let mut reply = vec![0u8; 5000];

        let done = call_named_pipe(
            &name,
            &request,
            &mut reply,
            PipeTimeout::from_millis(WAIT_MS),
        )
        .expect("a call");
        assert_eq!(
            done,
            Transaction {
                bytes_read: size,
                more_data: false
            }
        );
        assert_eq!(
            &reply[..size],
            &request.to_ascii_uppercase()[..],
            "size {size}"
        );
        serving.join().expect("the server");
    }
}

#[test]
fn a_reply_longer_than_the_buffer_is_more_data_and_the_rest_is_gone() {
    // The page: "If the message written to the pipe by the server process is longer than nOutBufferSize,
    // CallNamedPipe returns FALSE, and GetLastError returns ERROR_MORE_DATA. The remainder of the
    // message is discarded, because CallNamedPipe closes the handle to the pipe before returning."
    let name = unique_name("callmore");
    let serving = serve_call(&name, |_| (0..100u8).collect());
    let mut reply = [0u8; 10];
    let done = call_named_pipe(&name, b"req", &mut reply, PipeTimeout::from_millis(WAIT_MS))
        .expect("a call");
    assert!(done.more_data);
    assert_eq!(done.bytes_read, 10);
    assert_eq!(&reply, &(0..10u8).collect::<Vec<_>>()[..]);
    serving.join().expect("the server");
}

#[test]
fn a_call_to_a_byte_pipe_is_the_calls_error() {
    // The page: "CallNamedPipe fails if the pipe is a byte-type pipe."
    let name = unique_name("callbyte");
    let server = SynchronousPipe::create(&name, &byte_options(), None).expect("a pipe");
    let serving = thread::spawn(move || {
        let _ = connect_named_pipe(&server);
        thread::sleep(Duration::from_millis(300));
    });
    let error = call_named_pipe(&name, b"x", &mut [0u8; 8], PipeTimeout::from_millis(1000))
        .expect_err("a byte pipe");
    assert!(error.raw_os_error().is_some());
    serving.join().expect("the server");
}

#[test]
fn a_call_to_a_name_with_no_instance_is_not_found() {
    let started = Instant::now();
    let error = call_named_pipe(
        unique_name("callnone"),
        b"x",
        &mut [0u8; 8],
        PipeTimeout::from_millis(WAIT_MS),
    )
    .expect_err("no instance");
    assert_eq!(code(&error), ERROR_FILE_NOT_FOUND);
    assert!(
        started.elapsed() < Duration::from_millis(WAIT_MS.into()),
        "no wait"
    );
}

#[test]
fn a_call_waits_for_an_instance_for_the_time_given_and_then_fails() {
    // The page: the call waits "if an instance of the pipe is not available" for `timeout`. That the
    // error is `ERROR_PIPE_BUSY` and not the wait's time-out is an observation.
    let name = unique_name("callbusy");
    let _server = create_named_pipe(&name, &message_options_one_instance(), None).expect("a pipe");
    let _taken = OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(FILE_SHARE_ALL)
        .open(&name)
        .expect("a client");

    let started = Instant::now();
    let error = call_named_pipe(&name, b"x", &mut [0u8; 8], PipeTimeout::from_millis(300))
        .expect_err("no instance is available");
    assert_eq!(code(&error), ERROR_PIPE_BUSY);
    assert!(
        started.elapsed() >= Duration::from_millis(200),
        "{:?}",
        started.elapsed()
    );
}

fn message_options_one_instance() -> NamedPipeOptions {
    NamedPipeOptions {
        max_instances: 1,
        ..message_options()
    }
}

#[test]
fn no_wait_does_not_wait() {
    // The page: "NMPWAIT_NOWAIT ... Does not wait for the named pipe. If the named pipe is not
    // available, the function returns an error."
    let name = unique_name("callnowait");
    let _server = create_named_pipe(&name, &message_options_one_instance(), None).expect("a pipe");
    let _taken = OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(FILE_SHARE_ALL)
        .open(&name)
        .expect("a client");
    let started = Instant::now();
    let error = call_named_pipe(&name, b"x", &mut [0u8; 8], PipeTimeout::NO_WAIT)
        .expect_err("not available");
    assert_eq!(code(&error), ERROR_PIPE_BUSY);
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn a_name_with_an_interior_nul_is_refused_before_the_call() {
    let error = call_named_pipe(
        "\\\\.\\pipe\\a\0b",
        b"x",
        &mut [0u8; 8],
        PipeTimeout::NO_WAIT,
    )
    .expect_err("a NUL");
    assert_eq!(error.kind(), ErrorKind::InvalidInput);
    assert_eq!(error.raw_os_error(), None, "the call was not made");
}
