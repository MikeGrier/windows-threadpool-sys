// Copyright (c) 2026 Mike Grier
//! Tests for `get_named_pipe_info`, `get_named_pipe_handle_state` and `set_named_pipe_handle_state`.
//!
//! What is asserted is what the three pages say. A test of something a page does not say is named
//! an observation, made on the measuring machine, and asserts only what was observed.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsHandle, OwnedHandle};
use std::sync::atomic::{AtomicU32, Ordering};

use windows_sys::Win32::Foundation::{ERROR_INVALID_PARAMETER, GENERIC_READ, GENERIC_WRITE};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_READ_ATTRIBUTES, FILE_WRITE_ATTRIBUTES, SECURITY_ANONYMOUS, SECURITY_IMPERSONATION,
};

use super::{
    PipeHandleState, PipeInfoFlags, PipeStateQuery, PipeStateUpdate, get_named_pipe_handle_state,
    get_named_pipe_info, set_named_pipe_handle_state,
};
use crate::{NamedPipeOptions, PipeMode, PipeOpenMode, create_named_pipe, create_pipe};

const FILE_SHARE_ALL: u32 = 7;

fn unique_name(tag: &str) -> String {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    format!(
        r"\\.\pipe\win-pipes-sys-state-test-{}-{tag}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

fn byte_options() -> NamedPipeOptions {
    NamedPipeOptions {
        open_mode: PipeOpenMode::ACCESS_DUPLEX,
        pipe_mode: PipeMode::TYPE_BYTE | PipeMode::READMODE_BYTE | PipeMode::WAIT,
        max_instances: 1,
        out_buffer_size: 4096,
        in_buffer_size: 8192,
        default_timeout_ms: 0,
    }
}

fn message_options() -> NamedPipeOptions {
    NamedPipeOptions {
        pipe_mode: PipeMode::TYPE_MESSAGE | PipeMode::READMODE_MESSAGE | PipeMode::WAIT,
        ..byte_options()
    }
}

fn client(name: &str) -> File {
    OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(FILE_SHARE_ALL)
        .open(name)
        .expect("a client")
}

fn code(error: &io::Error) -> u32 {
    u32::try_from(error.raw_os_error().expect("an OS error")).expect("a code that fits")
}

fn query_state() -> PipeStateQuery {
    PipeStateQuery {
        state: true,
        ..PipeStateQuery::default()
    }
}

fn state_of(pipe: &impl AsHandle) -> PipeMode {
    get_named_pipe_handle_state(pipe, &query_state())
        .expect("the state")
        .state
        .expect("the state asked for")
}

fn set_mode(pipe: &impl AsHandle, mode: PipeMode) -> io::Result<()> {
    set_named_pipe_handle_state(
        pipe,
        &PipeStateUpdate {
            mode: Some(mode),
            ..PipeStateUpdate::default()
        },
    )
}

/// A connected pair whose client opened with impersonation access and wrote a byte the server has
/// read, which is what makes the client's user name retrievable.
fn pair_with_a_read_byte() -> (OwnedHandle, File, File) {
    pair_that_has_read_a_byte(SECURITY_IMPERSONATION)
}

/// As [`pair_with_a_read_byte`], with the client opening at the given security quality of service.
fn pair_that_has_read_a_byte(qos: u32) -> (OwnedHandle, File, File) {
    let name = unique_name("user");
    let server = create_named_pipe(&name, &byte_options(), None).expect("a pipe");
    let mut client = OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(FILE_SHARE_ALL)
        .security_qos_flags(qos)
        .open(&name)
        .expect("a client");
    client.write_all(b"x").expect("client write");
    let mut reader = File::from(server.try_clone().expect("a duplicate"));
    reader.read_exact(&mut [0u8; 1]).expect("server read");
    (server, client, reader)
}

// --- The flags ------------------------------------------------------------------------------

#[test]
fn the_info_flags_are_the_pages_values() {
    for (flags, value) in [
        (PipeInfoFlags::CLIENT_END, 0x0),
        (PipeInfoFlags::SERVER_END, 0x1),
        (PipeInfoFlags::TYPE_BYTE, 0x0),
        (PipeInfoFlags::TYPE_MESSAGE, 0x4),
    ] {
        assert_eq!(flags.bits(), value, "{flags:?}");
        assert_eq!(PipeInfoFlags::from_bits(value), flags);
    }
}

#[test]
fn the_info_flags_answer_what_a_zero_value_cannot() {
    let both = PipeInfoFlags::SERVER_END | PipeInfoFlags::TYPE_MESSAGE;
    assert!(both.is_server_end());
    assert!(both.is_message_type());
    assert!(both.contains(PipeInfoFlags::SERVER_END));
    assert!(
        both.contains(PipeInfoFlags::TYPE_BYTE),
        "a zero value is in every value"
    );

    let neither = PipeInfoFlags::default();
    assert!(!neither.is_server_end());
    assert!(!neither.is_message_type());
    assert!(!PipeInfoFlags::SERVER_END.is_message_type());
    assert!(!PipeInfoFlags::TYPE_MESSAGE.is_server_end());
}

#[test]
fn a_bit_with_no_name_is_held_in_the_flags() {
    assert_eq!(PipeInfoFlags::from_bits(0xf0f0_0001).bits(), 0xf0f0_0001);
    assert!(PipeInfoFlags::from_bits(0xf0f0_0001).is_server_end());
}

// --- GetNamedPipeInfo -----------------------------------------------------------------------

#[test]
fn the_server_end_of_a_byte_pipe_reports_what_it_was_made_with() {
    let server = create_named_pipe(unique_name("info"), &byte_options(), None).expect("a pipe");
    let info = get_named_pipe_info(&server).expect("the info");
    assert!(info.flags.is_server_end());
    assert!(!info.flags.is_message_type());
    assert_eq!(info.out_buffer_size, 4096);
    assert_eq!(info.in_buffer_size, 8192);
    assert_eq!(info.max_instances, 1);
}

#[test]
fn a_message_pipe_reports_its_type() {
    let server =
        create_named_pipe(unique_name("message"), &message_options(), None).expect("a pipe");
    let info = get_named_pipe_info(&server).expect("the info");
    assert_eq!(
        info.flags,
        PipeInfoFlags::SERVER_END | PipeInfoFlags::TYPE_MESSAGE
    );
}

#[test]
fn the_client_end_is_not_the_server_end() {
    let name = unique_name("client");
    let server = create_named_pipe(&name, &message_options(), None).expect("a pipe");
    let client = client(&name);
    let client_info = get_named_pipe_info(&client).expect("the client's info");
    assert!(!client_info.flags.is_server_end());
    assert!(
        client_info.flags.is_message_type(),
        "the type is the pipe's"
    );
    assert!(
        get_named_pipe_info(&server)
            .expect("the server's info")
            .flags
            .is_server_end()
    );
}

#[test]
fn the_instance_limit_is_reported_as_made() {
    for limit in [1u32, 2, 3, 100, NamedPipeOptions::UNLIMITED_INSTANCES] {
        let options = NamedPipeOptions {
            max_instances: limit,
            ..byte_options()
        };
        let server = create_named_pipe(unique_name("limit"), &options, None).expect("a pipe");
        assert_eq!(
            get_named_pipe_info(&server)
                .expect("the info")
                .max_instances,
            limit
        );
    }
}

#[test]
fn a_zero_buffer_size_is_reported_as_zero() {
    // The page: "If the buffer size is zero, the buffer is allocated as needed."
    let options = NamedPipeOptions {
        out_buffer_size: 0,
        in_buffer_size: 0,
        ..byte_options()
    };
    let server = create_named_pipe(unique_name("zero"), &options, None).expect("a pipe");
    let info = get_named_pipe_info(&server).expect("the info");
    assert_eq!((info.out_buffer_size, info.in_buffer_size), (0, 0));
}

#[test]
fn an_anonymous_pipes_handles_are_accepted() {
    // The page: the handle "can also be a handle to an anonymous pipe". Which end each reports as is
    // an observation: the read end reports as the server end.
    let pipe = create_pipe(0, None).expect("a pipe");
    let read = get_named_pipe_info(&pipe.read).expect("the read end");
    let write = get_named_pipe_info(&pipe.write).expect("the write end");
    assert!(read.flags.is_server_end());
    assert!(!write.flags.is_server_end());
    assert!(!read.flags.is_message_type() && !write.flags.is_message_type());
}

#[test]
fn a_handle_that_is_not_a_pipe_is_the_calls_error() {
    let file = File::open(std::env::current_exe().expect("this program")).expect("open it");
    let error = get_named_pipe_info(&file).expect_err("not a pipe");
    assert!(error.raw_os_error().is_some());
}

// --- GetNamedPipeHandleStateW ---------------------------------------------------------------

#[test]
fn an_empty_query_succeeds_and_returns_nothing() {
    // The page: the call "returns successfully even if all of the pointers passed to it are NULL".
    let server = create_named_pipe(unique_name("empty"), &byte_options(), None).expect("a pipe");
    let state =
        get_named_pipe_handle_state(&server, &PipeStateQuery::default()).expect("the state");
    assert_eq!(state, PipeHandleState::default());
}

#[test]
fn the_state_is_blocking_and_byte_read_unless_the_flags_are_set() {
    let byte = create_named_pipe(unique_name("byte"), &byte_options(), None).expect("a pipe");
    assert_eq!(state_of(&byte), PipeMode::default());

    let message = NamedPipeOptions {
        pipe_mode: PipeMode::TYPE_MESSAGE | PipeMode::READMODE_MESSAGE | PipeMode::NOWAIT,
        ..byte_options()
    };
    let message = create_named_pipe(unique_name("message"), &message, None).expect("a pipe");
    let state = state_of(&message);
    assert!(state.contains(PipeMode::READMODE_MESSAGE));
    assert!(state.contains(PipeMode::NOWAIT));
    assert_eq!(state, PipeMode::READMODE_MESSAGE | PipeMode::NOWAIT);
}

#[test]
fn only_what_is_asked_for_is_returned() {
    let server = create_named_pipe(unique_name("some"), &byte_options(), None).expect("a pipe");
    let query = PipeStateQuery {
        current_instances: true,
        ..PipeStateQuery::default()
    };
    let state = get_named_pipe_handle_state(&server, &query).expect("the state");
    assert_eq!(state.current_instances, Some(1));
    assert_eq!(state.state, None);
    assert_eq!(state.max_collection_count, None);
    assert_eq!(state.collect_data_timeout_ms, None);
    assert_eq!(state.user_name, None);
}

#[test]
fn the_current_instance_count_follows_the_instances_made() {
    let name = unique_name("instances");
    let options = NamedPipeOptions {
        max_instances: 3,
        ..byte_options()
    };
    let query = PipeStateQuery {
        current_instances: true,
        ..PipeStateQuery::default()
    };
    let first = create_named_pipe(&name, &options, None).expect("the first");
    let count = |pipe: &OwnedHandle| {
        get_named_pipe_handle_state(pipe, &query)
            .expect("the state")
            .current_instances
    };
    assert_eq!(count(&first), Some(1));
    let second = create_named_pipe(&name, &options, None).expect("the second");
    assert_eq!(count(&first), Some(2));
    assert_eq!(count(&second), Some(2));
    let third = create_named_pipe(&name, &options, None).expect("the third");
    assert_eq!(count(&third), Some(3));
    drop(third);
    assert_eq!(count(&first), Some(2), "it can vary during the lifetime");
}

#[test]
fn the_collection_values_are_refused_on_the_same_computer() {
    // The page: both "must be NULL if the specified pipe handle is to the server end of a named pipe
    // or if client and server processes are on the same computer".
    let name = unique_name("collect");
    let server = create_named_pipe(&name, &byte_options(), None).expect("a pipe");
    let client = client(&name);
    for (pipe, which) in [(&server as &dyn AsHandle, "server"), (&client, "client")] {
        for query in [
            PipeStateQuery {
                max_collection_count: true,
                ..PipeStateQuery::default()
            },
            PipeStateQuery {
                collect_data_timeout: true,
                ..PipeStateQuery::default()
            },
        ] {
            let error = get_named_pipe_handle_state(&pipe, &query)
                .expect_err("the call refuses the request");
            assert_eq!(code(&error), ERROR_INVALID_PARAMETER, "{which}");
        }
    }
}

#[test]
fn the_user_name_is_refused_for_the_client_end() {
    // The page: it "must be NULL if the specified pipe handle is to the client end".
    let name = unique_name("clientname");
    let _server = create_named_pipe(&name, &byte_options(), None).expect("a pipe");
    let client = client(&name);
    let query = PipeStateQuery {
        user_name_capacity: Some(512),
        ..PipeStateQuery::default()
    };
    assert!(get_named_pipe_handle_state(&client, &query).is_err());
}

#[test]
fn the_user_name_is_refused_for_a_client_that_did_not_open_with_impersonation_access() {
    // The page: the server "can only retrieve this information if the client opened the pipe with
    // SECURITY_IMPERSONATION access". A client that asks for anonymous access is the one that did
    // not; that a client that asks for nothing is not refused after it writes, and is refused before,
    // is an observation.
    let query = PipeStateQuery {
        user_name_capacity: Some(512),
        ..PipeStateQuery::default()
    };

    let name = unique_name("early");
    let server = create_named_pipe(&name, &byte_options(), None).expect("a pipe");
    let _client = client(&name);
    assert!(get_named_pipe_handle_state(&server, &query).is_err());

    let (server, _client, _reader) = pair_that_has_read_a_byte(SECURITY_ANONYMOUS);
    assert!(get_named_pipe_handle_state(&server, &query).is_err());

    let (server, _client, _reader) = pair_that_has_read_a_byte(SECURITY_IMPERSONATION);
    assert!(get_named_pipe_handle_state(&server, &query).is_ok());
}

#[test]
fn the_user_name_is_returned_once_data_has_been_read() {
    let (server, _client, _reader) = pair_with_a_read_byte();
    let query = PipeStateQuery {
        user_name_capacity: Some(512),
        ..PipeStateQuery::default()
    };
    let name = get_named_pipe_handle_state(&server, &query)
        .expect("the state")
        .user_name
        .expect("the name asked for");
    assert!(!name.is_empty());
}

#[test]
fn the_user_name_needs_a_capacity_that_holds_it_and_its_terminator() {
    // Observed: a capacity of the name's length fails and one more succeeds, which is what a
    // NUL-terminated string needs; the page does not give the error.
    use std::os::windows::ffi::OsStrExt;

    let (server, _client, _reader) = pair_with_a_read_byte();
    let ask = |capacity: u32| {
        let query = PipeStateQuery {
            user_name_capacity: Some(capacity),
            ..PipeStateQuery::default()
        };
        get_named_pipe_handle_state(&server, &query)
    };
    let name = ask(512).expect("the state").user_name.expect("the name");
    let units = u32::try_from(name.encode_wide().count()).expect("a short name");

    for capacity in 0..=units {
        assert!(ask(capacity).is_err(), "capacity {capacity} of {units}");
    }
    for capacity in [units + 1, units + 2, 256, 513] {
        let again = ask(capacity).expect("a capacity that holds it");
        assert_eq!(
            again.user_name.as_deref(),
            Some(name.as_os_str()),
            "{capacity}"
        );
    }
}

#[test]
fn a_query_for_everything_a_local_server_end_allows_gives_all_of_it() {
    let (server, _client, _reader) = pair_with_a_read_byte();
    let query = PipeStateQuery {
        state: true,
        current_instances: true,
        user_name_capacity: Some(512),
        ..PipeStateQuery::default()
    };
    let state = get_named_pipe_handle_state(&server, &query).expect("the state");
    assert_eq!(state.state, Some(PipeMode::default()));
    assert_eq!(state.current_instances, Some(1));
    assert!(state.user_name.is_some());
}

// --- SetNamedPipeHandleState ----------------------------------------------------------------

#[test]
fn an_update_that_changes_nothing_succeeds() {
    // The page makes every optional input optional, so an update with none is the call with three
    // NULLs; that it succeeds is an observation.
    let server = create_named_pipe(unique_name("noop"), &byte_options(), None).expect("a pipe");
    set_named_pipe_handle_state(&server, &PipeStateUpdate::default()).expect("a no-op");
    assert_eq!(state_of(&server), PipeMode::default());
}

#[test]
fn the_read_mode_of_a_message_pipe_is_changed_and_changed_back() {
    let server =
        create_named_pipe(unique_name("readmode"), &message_options(), None).expect("a pipe");
    assert_eq!(state_of(&server), PipeMode::READMODE_MESSAGE);
    set_mode(&server, PipeMode::READMODE_BYTE).expect("to byte");
    assert_eq!(state_of(&server), PipeMode::READMODE_BYTE);
    set_mode(&server, PipeMode::READMODE_MESSAGE).expect("to message");
    assert_eq!(state_of(&server), PipeMode::READMODE_MESSAGE);
}

#[test]
fn the_wait_mode_is_changed_and_changed_back() {
    let server = create_named_pipe(unique_name("wait"), &byte_options(), None).expect("a pipe");
    set_mode(&server, PipeMode::NOWAIT).expect("nonblocking");
    assert_eq!(state_of(&server), PipeMode::NOWAIT);
    set_mode(&server, PipeMode::WAIT).expect("blocking");
    assert_eq!(state_of(&server), PipeMode::WAIT);
}

#[test]
fn every_read_and_wait_combination_on_a_message_pipe_reads_back_as_set() {
    let server =
        create_named_pipe(unique_name("combos"), &message_options(), None).expect("a pipe");
    for read in [PipeMode::READMODE_BYTE, PipeMode::READMODE_MESSAGE] {
        for wait in [PipeMode::WAIT, PipeMode::NOWAIT] {
            set_mode(&server, read | wait).expect("a combination");
            assert_eq!(state_of(&server), read | wait);
        }
    }
}

#[test]
fn the_message_read_mode_is_refused_for_a_byte_pipe() {
    // The page: "The function fails if this flag is specified for a byte-type pipe."
    let server = create_named_pipe(unique_name("refused"), &byte_options(), None).expect("a pipe");
    let error = set_mode(&server, PipeMode::READMODE_MESSAGE).expect_err("a byte pipe");
    assert_eq!(code(&error), ERROR_INVALID_PARAMETER);
    assert_eq!(state_of(&server), PipeMode::default(), "nothing changed");
}

#[test]
fn a_bit_that_is_not_a_read_or_wait_mode_is_the_calls_refusal() {
    // Observed: the type and the remote-client modes are not modes this call takes.
    let server = create_named_pipe(unique_name("bits"), &message_options(), None).expect("a pipe");
    for bad in [PipeMode::TYPE_MESSAGE, PipeMode::REJECT_REMOTE_CLIENTS] {
        let error = set_mode(&server, bad).expect_err("not a mode of this call");
        assert_eq!(code(&error), ERROR_INVALID_PARAMETER, "{bad:?}");
    }
}

#[test]
fn the_collection_values_are_refused_on_the_same_computer_when_set() {
    // The page: both "must be NULL ... if client and server processes are on the same machine".
    let name = unique_name("setcollect");
    let server = create_named_pipe(&name, &byte_options(), None).expect("a pipe");
    let client = client(&name);
    for pipe in [&server as &dyn AsHandle, &client] {
        for update in [
            PipeStateUpdate {
                max_collection_count: Some(1),
                ..PipeStateUpdate::default()
            },
            PipeStateUpdate {
                collect_data_timeout_ms: Some(1),
                ..PipeStateUpdate::default()
            },
        ] {
            let error = set_named_pipe_handle_state(&pipe, &update).expect_err("refused");
            assert_eq!(code(&error), ERROR_INVALID_PARAMETER);
        }
    }
}

#[test]
fn a_handles_state_is_its_own() {
    // Observed: a handle's read mode is not shared with the other end's, and a client of a message
    // pipe begins in byte-read mode while the server end began in message-read mode.
    let name = unique_name("own");
    let server = create_named_pipe(&name, &message_options(), None).expect("a pipe");
    let client = client(&name);
    assert_eq!(state_of(&server), PipeMode::READMODE_MESSAGE);
    assert_eq!(state_of(&client), PipeMode::READMODE_BYTE);

    set_mode(&client, PipeMode::READMODE_MESSAGE).expect("the client's");
    set_mode(&server, PipeMode::READMODE_BYTE).expect("the server's");
    assert_eq!(state_of(&client), PipeMode::READMODE_MESSAGE);
    assert_eq!(state_of(&server), PipeMode::READMODE_BYTE);
}

#[test]
fn an_anonymous_pipes_handles_take_a_state() {
    // The page: the handle "can also be a handle to an anonymous pipe".
    let pipe = create_pipe(0, None).expect("a pipe");
    set_mode(&pipe.read, PipeMode::NOWAIT).expect("the read end");
    assert_eq!(state_of(&pipe.read), PipeMode::NOWAIT);
    set_mode(&pipe.write, PipeMode::NOWAIT).expect("the write end");
    assert_eq!(state_of(&pipe.write), PipeMode::NOWAIT);
}

#[test]
fn a_read_only_client_needs_write_attributes_access_to_set_the_state() {
    // The page: the handle "must have GENERIC_READ and FILE_WRITE_ATTRIBUTES access for a read-only
    // pipe". Two instances, because a connected instance is busy to the second client.
    let name = unique_name("readonly");
    let options = NamedPipeOptions {
        open_mode: PipeOpenMode::ACCESS_OUTBOUND,
        max_instances: 2,
        ..byte_options()
    };
    let _server = create_named_pipe(&name, &options, None).expect("a pipe");
    let _second = create_named_pipe(&name, &options, None).expect("a second instance");

    let without = OpenOptions::new()
        .access_mode(GENERIC_READ)
        .share_mode(FILE_SHARE_ALL)
        .open(&name)
        .expect("a reader");
    assert!(set_mode(&without, PipeMode::NOWAIT).is_err());

    let with = OpenOptions::new()
        .access_mode(GENERIC_READ | FILE_WRITE_ATTRIBUTES)
        .share_mode(FILE_SHARE_ALL)
        .open(&name)
        .expect("a reader with attributes access");
    set_mode(&with, PipeMode::NOWAIT).expect("allowed");
    assert_eq!(state_of(&with), PipeMode::NOWAIT);
}

#[test]
fn a_write_only_client_with_read_attributes_access_gets_the_info_and_the_state() {
    // The page: the handle "must have GENERIC_WRITE and FILE_READ_ATTRIBUTES access for a write-only
    // pipe". Only the satisfied direction is asserted: the unsatisfied one was not observed to fail.
    let name = unique_name("writeonly");
    let options = NamedPipeOptions {
        open_mode: PipeOpenMode::ACCESS_INBOUND,
        ..byte_options()
    };
    let _server = create_named_pipe(&name, &options, None).expect("a pipe");
    let with = OpenOptions::new()
        .access_mode(GENERIC_WRITE | FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_ALL)
        .open(&name)
        .expect("a writer with attributes access");
    assert!(
        !get_named_pipe_info(&with)
            .expect("allowed")
            .flags
            .is_server_end()
    );
    assert_eq!(state_of(&with), PipeMode::default());
}
