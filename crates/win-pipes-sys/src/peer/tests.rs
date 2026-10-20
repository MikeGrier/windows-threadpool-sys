// Copyright (c) 2026 Mike Grier
//! Tests for the pipe peer queries: the process and session identifiers of each end, and the client's
//! computer name.
//!
//! What is asserted of the five calls is what their pages say, `ms.date` 12/05/2018 and 08/05/2022,
//! which is little: each returns the identifier or name "for the specified named pipe". The identifiers
//! are checked against the documented routes to the same facts, the process's own id and
//! `ProcessIdToSessionId`, and against a second process. A test of anything the pages do not say is
//! named an observation.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::windows::fs::OpenOptionsExt;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

use windows_sys::Win32::Foundation::{ERROR_NOT_FOUND, ERROR_PIPE_LOCAL};
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;

use super::{
    computer_name_buffer, get_named_pipe_client_computer_name, get_named_pipe_client_process_id,
    get_named_pipe_client_session_id, get_named_pipe_server_process_id,
    get_named_pipe_server_session_id,
};
use crate::{
    NamedPipeOptions, PipeMode, PipeOpenMode, SynchronousPipe, connect_named_pipe,
    create_named_pipe, create_pipe,
};

const FILE_SHARE_ALL: u32 = 7;

fn unique_name(tag: &str) -> String {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    format!(
        r"\\.\pipe\win-pipes-sys-peer-test-{}-{tag}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

fn options() -> NamedPipeOptions {
    NamedPipeOptions {
        open_mode: PipeOpenMode::ACCESS_DUPLEX,
        pipe_mode: PipeMode::TYPE_BYTE | PipeMode::WAIT,
        max_instances: 4,
        out_buffer_size: 4096,
        in_buffer_size: 4096,
        default_timeout_ms: 0,
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

/// The session `ProcessIdToSessionId` says the process is in.
fn session_of(process: u32) -> u32 {
    let mut session = 0u32;
    // SAFETY: `session` is a local that outlives the call.
    let ok = unsafe { ProcessIdToSessionId(process, &mut session) };
    assert_ne!(ok, 0, "{}", std::io::Error::last_os_error());
    session
}

/// A connected pair in this process: the server end and the client end.
fn connected_pair() -> (std::os::windows::io::OwnedHandle, File) {
    let name = unique_name("pair");
    let server = create_named_pipe(&name, &options(), None).expect("a pipe");
    let client = open_client(&name);
    (server, client)
}

/// A pipe whose client is another process: `cmd` reading a line from the pipe, which the server writes
/// when it is done. The client has opened the pipe when `connect_named_pipe` returns.
fn pipe_with_a_child_client() -> (std::os::windows::io::OwnedHandle, Child) {
    let name = unique_name("child");
    let server = SynchronousPipe::create(&name, &options(), None).expect("a pipe");
    let child = Command::new("cmd")
        .args(["/c", &format!("set /p line=<{name}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .expect("a child");
    connect_named_pipe(&server).expect("the child connects");
    (server.into_owned(), child)
}

fn finish(server: std::os::windows::io::OwnedHandle, mut child: Child) {
    File::from(server)
        .write_all(b"done\r\n")
        .expect("release the child");
    assert!(child.wait().expect("the child").success());
}

// --- The buffer ------------------------------------------------------------------------------

#[test]
fn the_computer_name_buffer_is_whole_units_and_never_more_than_asked() {
    for (capacity, units) in [
        (0u32, 0usize),
        (1, 0),
        (2, 1),
        (3, 1),
        (4, 2),
        (31, 15),
        (32, 16),
        (1024, 512),
        (1025, 512),
    ] {
        let (buffer, bytes) = computer_name_buffer(capacity);
        assert_eq!(buffer.len(), units, "capacity {capacity}");
        assert_eq!(bytes as usize, units * 2, "capacity {capacity}");
        assert!(bytes <= capacity);
        assert!(buffer.iter().all(|&unit| unit == 0));
    }
}

// --- The identifiers --------------------------------------------------------------------------

#[test]
fn the_server_end_reports_this_process_as_both_ends() {
    let (server, _client) = connected_pair();
    let me = std::process::id();
    assert_eq!(
        get_named_pipe_client_process_id(&server).expect("client pid"),
        me
    );
    assert_eq!(
        get_named_pipe_server_process_id(&server).expect("server pid"),
        me
    );
}

#[test]
fn the_sessions_are_the_one_this_process_is_in() {
    let (server, _client) = connected_pair();
    let session = session_of(std::process::id());
    assert_eq!(
        get_named_pipe_client_session_id(&server).expect("client session"),
        session
    );
    assert_eq!(
        get_named_pipe_server_session_id(&server).expect("server session"),
        session
    );
}

#[test]
fn the_client_end_reports_the_same_four() {
    // Observed: the pages say the handle "must be created by the CreateNamedPipe function", and the four
    // calls succeeded on a client's handle as well.
    let (_server, client) = connected_pair();
    let me = std::process::id();
    let session = session_of(me);
    assert_eq!(
        get_named_pipe_client_process_id(&client).expect("client pid"),
        me
    );
    assert_eq!(
        get_named_pipe_server_process_id(&client).expect("server pid"),
        me
    );
    assert_eq!(
        get_named_pipe_client_session_id(&client).expect("client session"),
        session
    );
    assert_eq!(
        get_named_pipe_server_session_id(&client).expect("server session"),
        session
    );
}

#[test]
fn a_client_in_another_process_is_told_apart_from_the_server() {
    let (server, child) = pipe_with_a_child_client();
    let me = std::process::id();

    let client = get_named_pipe_client_process_id(&server).expect("client pid");
    assert_eq!(client, child.id(), "the client is the child");
    assert_ne!(client, me);
    assert_eq!(
        get_named_pipe_server_process_id(&server).expect("server pid"),
        me
    );
    assert_eq!(
        get_named_pipe_client_session_id(&server).expect("client session"),
        session_of(child.id())
    );
    finish(server, child);
}

#[test]
fn the_client_end_of_a_child_pipe_sees_this_process_as_the_server() {
    // The child is the client, so the server's id is read from the server's own handle here; the
    // mirror, a server in another process, is not made.
    let (server, child) = pipe_with_a_child_client();
    assert_eq!(
        get_named_pipe_server_process_id(&server).expect("server pid"),
        std::process::id()
    );
    finish(server, child);
}

#[test]
fn an_instance_with_no_client_has_a_server_and_no_client() {
    // Observed: the client's identifiers are the call's `ERROR_NOT_FOUND` and the server's is this
    // process.
    let name = unique_name("unconnected");
    let server = create_named_pipe(&name, &options(), None).expect("a pipe");
    assert_eq!(
        code(&get_named_pipe_client_process_id(&server).expect_err("no client")),
        ERROR_NOT_FOUND
    );
    assert_eq!(
        code(&get_named_pipe_client_session_id(&server).expect_err("no client")),
        ERROR_NOT_FOUND
    );
    assert_eq!(
        get_named_pipe_server_process_id(&server).expect("server pid"),
        std::process::id()
    );
    assert_eq!(
        get_named_pipe_server_session_id(&server).expect("server session"),
        session_of(std::process::id())
    );
}

#[test]
fn an_anonymous_pipes_handles_are_accepted() {
    // Observed, and not on the pages: both ends of an anonymous pipe report this process.
    let pipe = create_pipe(0, None).expect("a pipe");
    let me = std::process::id();
    assert_eq!(
        get_named_pipe_client_process_id(&pipe.read).expect("client pid"),
        me
    );
    assert_eq!(
        get_named_pipe_server_process_id(&pipe.write).expect("server pid"),
        me
    );
}

#[test]
fn a_handle_that_is_not_a_pipe_is_the_calls_error() {
    let file = File::open(std::env::current_exe().expect("this program")).expect("open it");
    assert!(get_named_pipe_client_process_id(&file).is_err());
    assert!(get_named_pipe_client_session_id(&file).is_err());
    assert!(get_named_pipe_server_process_id(&file).is_err());
    assert!(get_named_pipe_server_session_id(&file).is_err());
    assert!(get_named_pipe_client_computer_name(&file, 1024).is_err());
}

#[test]
fn many_instances_each_report_their_own_client() {
    let name = unique_name("many");
    let servers: Vec<_> = (0..4)
        .map(|_| create_named_pipe(&name, &options(), None).expect("an instance"))
        .collect();
    let clients: Vec<_> = (0..4).map(|_| open_client(&name)).collect();
    let me = std::process::id();
    for server in &servers {
        assert_eq!(
            get_named_pipe_client_process_id(server).expect("client pid"),
            me
        );
    }
    drop(clients);
}

// --- The computer name ------------------------------------------------------------------------

#[test]
fn a_local_pipe_has_no_client_computer_name() {
    // Observed: for a client on the same computer the call fails with `ERROR_PIPE_LOCAL`, whatever the
    // capacity, from either end's handle. A name returned for a remote client is not reached here, and
    // the decoding it goes through is tested on its own in `name`.
    let (server, client) = connected_pair();
    for capacity in [0u32, 1, 2, 30, 32, 64, 512, 1024, 4096] {
        for error in [
            get_named_pipe_client_computer_name(&server, capacity).expect_err("a local client"),
            get_named_pipe_client_computer_name(&client, capacity).expect_err("a local client"),
        ] {
            assert_eq!(code(&error), ERROR_PIPE_LOCAL, "capacity {capacity}");
        }
    }
}
