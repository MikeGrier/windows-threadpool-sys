// Copyright (c) 2026 Mike Grier
//! Tests for `create_named_pipe`.
//!
//! What is asserted of the call is what its page says, `ms.date` 08/03/2022. A test that depends
//! on an error code says whether the page names it; a refusal the page does not name is asserted
//! only to be the call's own, which has an OS code.

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::windows::ffi::OsStringExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, OwnedHandle};
use std::sync::atomic::{AtomicU32, Ordering};

use win_authorization_sys::SecurityAttributes;
use win_handles_sys::{HandleFlags, get_handle_information};
use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_INVALID_PARAMETER, ERROR_IO_PENDING,
    ERROR_OPERATION_ABORTED, TRUE,
};
use windows_sys::Win32::Storage::FileSystem::ReadFile;
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Pipes::{
    GetNamedPipeHandleStateW, GetNamedPipeInfo, PIPE_SERVER_END,
};

use super::{NamedPipeOptions, PipeMode, PipeOpenMode, create_named_pipe};

const FILE_SHARE_ALL: u32 = 7;

/// A pipe name no other test, and no earlier run, is using.
fn unique_name(tag: &str) -> String {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    format!(
        r"\\.\pipe\win-pipes-sys-test-{}-{tag}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

fn byte_duplex() -> NamedPipeOptions {
    NamedPipeOptions {
        open_mode: PipeOpenMode::ACCESS_DUPLEX,
        pipe_mode: PipeMode::TYPE_BYTE | PipeMode::READMODE_BYTE | PipeMode::WAIT,
        max_instances: 1,
        out_buffer_size: 4096,
        in_buffer_size: 4096,
        default_timeout_ms: 0,
    }
}

fn client(name: &str, read: bool, write: bool) -> std::io::Result<File> {
    OpenOptions::new()
        .read(read)
        .write(write)
        .share_mode(FILE_SHARE_ALL)
        .open(name)
}

fn code(error: &std::io::Error) -> u32 {
    u32::try_from(error.raw_os_error().expect("an OS error")).expect("a code that fits")
}

// --- The modes ------------------------------------------------------------------------------

#[test]
fn the_open_modes_are_the_pages_values() {
    for (mode, value) in [
        (PipeOpenMode::ACCESS_INBOUND, 0x0000_0001),
        (PipeOpenMode::ACCESS_OUTBOUND, 0x0000_0002),
        (PipeOpenMode::ACCESS_DUPLEX, 0x0000_0003),
        (PipeOpenMode::FIRST_PIPE_INSTANCE, 0x0008_0000),
        (PipeOpenMode::WRITE_THROUGH, 0x8000_0000),
        (PipeOpenMode::OVERLAPPED, 0x4000_0000),
        (PipeOpenMode::WRITE_DAC, 0x0004_0000),
        (PipeOpenMode::WRITE_OWNER, 0x0008_0000),
        (PipeOpenMode::ACCESS_SYSTEM_SECURITY, 0x0100_0000),
    ] {
        assert_eq!(mode.bits(), value, "{mode:?}");
        assert_eq!(PipeOpenMode::from_bits(value), mode);
    }
}

#[test]
fn the_pipe_modes_are_the_pages_values() {
    for (mode, value) in [
        (PipeMode::TYPE_BYTE, 0x0),
        (PipeMode::TYPE_MESSAGE, 0x4),
        (PipeMode::READMODE_BYTE, 0x0),
        (PipeMode::READMODE_MESSAGE, 0x2),
        (PipeMode::WAIT, 0x0),
        (PipeMode::NOWAIT, 0x1),
        (PipeMode::ACCEPT_REMOTE_CLIENTS, 0x0),
        (PipeMode::REJECT_REMOTE_CLIENTS, 0x8),
    ] {
        assert_eq!(mode.bits(), value, "{mode:?}");
        assert_eq!(PipeMode::from_bits(value), mode);
    }
}

#[test]
fn first_instance_and_write_owner_are_one_bit_by_the_pages_own_values() {
    assert_eq!(PipeOpenMode::FIRST_PIPE_INSTANCE, PipeOpenMode::WRITE_OWNER);
}

#[test]
fn modes_combine_and_are_asked_about_by_bit() {
    let open = PipeOpenMode::ACCESS_DUPLEX | PipeOpenMode::OVERLAPPED | PipeOpenMode::WRITE_DAC;
    assert!(open.contains(PipeOpenMode::OVERLAPPED));
    assert!(open.contains(PipeOpenMode::WRITE_DAC));
    assert!(
        open.contains(PipeOpenMode::ACCESS_INBOUND),
        "duplex has both"
    );
    assert!(!open.contains(PipeOpenMode::WRITE_THROUGH));
    assert_eq!(PipeOpenMode::default().bits(), 0);

    let pipe = PipeMode::TYPE_MESSAGE | PipeMode::NOWAIT;
    assert!(pipe.contains(PipeMode::TYPE_MESSAGE));
    assert!(pipe.contains(PipeMode::NOWAIT));
    assert!(!pipe.contains(PipeMode::READMODE_MESSAGE));
    assert!(
        pipe.contains(PipeMode::WAIT),
        "a zero mode is contained in every mode"
    );
    assert_eq!(PipeMode::default().bits(), 0);
}

#[test]
fn a_bit_with_no_name_is_held() {
    assert_eq!(PipeOpenMode::from_bits(0x1234_5678).bits(), 0x1234_5678);
    assert_eq!(PipeMode::from_bits(0xff00_00ff).bits(), 0xff00_00ff);
}

#[test]
fn the_unlimited_instance_count_is_255() {
    assert_eq!(NamedPipeOptions::UNLIMITED_INSTANCES, 255);
}

// --- Creating a pipe ------------------------------------------------------------------------

#[test]
fn a_client_reaches_a_created_pipe_and_data_flows_both_ways() {
    let name = unique_name("duplex");
    let mut server = File::from(create_named_pipe(&name, &byte_duplex(), None).expect("a pipe"));
    let mut client = client(&name, true, true).expect("a client");

    client.write_all(b"to the server").expect("client write");
    let mut received = [0u8; 13];
    server.read_exact(&mut received).expect("server read");
    assert_eq!(&received, b"to the server");

    server.write_all(b"to the client").expect("server write");
    let mut received = [0u8; 13];
    client.read_exact(&mut received).expect("client read");
    assert_eq!(&received, b"to the client");
}

#[test]
fn an_inbound_pipe_takes_a_client_that_writes_and_refuses_one_that_reads() {
    let name = unique_name("inbound");
    let options = NamedPipeOptions {
        open_mode: PipeOpenMode::ACCESS_INBOUND,
        ..byte_duplex()
    };
    let mut server = File::from(create_named_pipe(&name, &options, None).expect("a pipe"));

    let refused = client(&name, true, true).expect_err("a client that reads and writes");
    assert_eq!(code(&refused), ERROR_ACCESS_DENIED);

    let mut writer = client(&name, false, true).expect("a client that writes");
    writer.write_all(b"in").expect("client write");
    let mut received = [0u8; 2];
    server.read_exact(&mut received).expect("server read");
    assert_eq!(&received, b"in");
}

#[test]
fn an_outbound_pipe_takes_a_client_that_reads_and_refuses_one_that_writes() {
    let name = unique_name("outbound");
    let options = NamedPipeOptions {
        open_mode: PipeOpenMode::ACCESS_OUTBOUND,
        ..byte_duplex()
    };
    let mut server = File::from(create_named_pipe(&name, &options, None).expect("a pipe"));

    let refused = client(&name, true, true).expect_err("a client that reads and writes");
    assert_eq!(code(&refused), ERROR_ACCESS_DENIED);

    let mut reader = client(&name, true, false).expect("a client that reads");
    server.write_all(b"out").expect("server write");
    let mut received = [0u8; 3];
    reader.read_exact(&mut received).expect("client read");
    assert_eq!(&received, b"out");
}

#[test]
fn the_first_instance_flag_refuses_a_second_instance_with_access_denied() {
    let name = unique_name("first");
    let options = NamedPipeOptions {
        open_mode: PipeOpenMode::ACCESS_DUPLEX | PipeOpenMode::FIRST_PIPE_INSTANCE,
        max_instances: 2,
        ..byte_duplex()
    };
    let _first = create_named_pipe(&name, &options, None).expect("the first instance");

    let second = create_named_pipe(&name, &options, None).expect_err("a second instance");
    assert_eq!(code(&second), ERROR_ACCESS_DENIED);
}

#[test]
fn without_the_first_instance_flag_a_second_instance_is_made() {
    let name = unique_name("second");
    let options = NamedPipeOptions {
        max_instances: 2,
        ..byte_duplex()
    };
    let _first = create_named_pipe(&name, &options, None).expect("the first instance");
    let _second = create_named_pipe(&name, &options, None).expect("a second instance");
}

#[test]
fn the_instance_count_bounds_the_instances_made() {
    for limit in [1u32, 2, 3] {
        let name = unique_name("limit");
        let options = NamedPipeOptions {
            max_instances: limit,
            ..byte_duplex()
        };
        let made: Vec<OwnedHandle> = (0..limit)
            .map(|_| {
                create_named_pipe(&name, &options, None).expect("an instance within the limit")
            })
            .collect();
        assert_eq!(made.len(), limit as usize);
        let error = create_named_pipe(&name, &options, None).expect_err("one more than the limit");
        assert!(error.raw_os_error().is_some(), "{limit}: {error}");
    }
}

#[test]
fn more_than_the_unlimited_count_is_invalid_parameter_as_the_page_says() {
    let options = NamedPipeOptions {
        max_instances: NamedPipeOptions::UNLIMITED_INSTANCES + 1,
        ..byte_duplex()
    };
    let error = create_named_pipe(unique_name("many"), &options, None).expect_err("256 instances");
    assert_eq!(code(&error), ERROR_INVALID_PARAMETER);
}

#[test]
fn the_unlimited_count_itself_is_accepted() {
    let options = NamedPipeOptions {
        max_instances: NamedPipeOptions::UNLIMITED_INSTANCES,
        ..byte_duplex()
    };
    create_named_pipe(unique_name("unlimited"), &options, None).expect("255 instances");
}

#[test]
fn a_byte_pipe_read_as_messages_is_refused_by_the_call() {
    // The page: PIPE_TYPE_BYTE "cannot be used with PIPE_READMODE_MESSAGE". Left to the call.
    let options = NamedPipeOptions {
        pipe_mode: PipeMode::TYPE_BYTE | PipeMode::READMODE_MESSAGE,
        ..byte_duplex()
    };
    let error = create_named_pipe(unique_name("badmode"), &options, None).expect_err("refused");
    assert!(error.raw_os_error().is_some(), "{error}");
}

#[test]
fn a_dropped_server_end_deletes_the_instance() {
    let name = unique_name("drop");
    let server = create_named_pipe(&name, &byte_duplex(), None).expect("a pipe");
    drop(server);

    let error = client(&name, true, true).expect_err("the instance is gone");
    assert_eq!(code(&error), ERROR_FILE_NOT_FOUND);
}

// --- What the pipe mode did -----------------------------------------------------------------

/// What `GetNamedPipeInfo` reports as the pipe's flags.
fn info_flags(pipe: &OwnedHandle) -> u32 {
    let mut flags = 0u32;
    // SAFETY: the handle is open for the call and `flags` is a valid out pointer; the other four
    // are optional and null.
    let ok = unsafe {
        GetNamedPipeInfo(
            pipe.as_raw_handle(),
            &mut flags,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    assert_ne!(
        ok,
        0,
        "GetNamedPipeInfo: {}",
        std::io::Error::last_os_error()
    );
    flags
}

/// What `GetNamedPipeHandleStateW` reports as the handle's state.
fn handle_state(pipe: &OwnedHandle) -> u32 {
    let mut state = 0u32;
    // SAFETY: the handle is open for the call and `state` is a valid out pointer; the rest are
    // optional and null, and a null user name needs a zero length.
    let ok = unsafe {
        GetNamedPipeHandleStateW(
            pipe.as_raw_handle(),
            &mut state,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
        )
    };
    assert_ne!(
        ok,
        0,
        "GetNamedPipeHandleStateW: {}",
        std::io::Error::last_os_error()
    );
    state
}

#[test]
fn the_type_and_the_read_and_wait_modes_reach_the_pipe() {
    let message = NamedPipeOptions {
        pipe_mode: PipeMode::TYPE_MESSAGE | PipeMode::READMODE_MESSAGE | PipeMode::NOWAIT,
        ..byte_duplex()
    };
    let pipe = create_named_pipe(unique_name("message"), &message, None).expect("a message pipe");
    assert_eq!(
        info_flags(&pipe),
        PIPE_SERVER_END | PipeMode::TYPE_MESSAGE.bits()
    );
    assert_eq!(
        handle_state(&pipe),
        (PipeMode::READMODE_MESSAGE | PipeMode::NOWAIT).bits()
    );

    let byte = create_named_pipe(unique_name("byte"), &byte_duplex(), None).expect("a byte pipe");
    assert_eq!(info_flags(&byte), PIPE_SERVER_END);
    assert_eq!(handle_state(&byte), 0);
}

// --- The security attributes ----------------------------------------------------------------

#[test]
fn no_attributes_gives_a_handle_that_cannot_be_inherited() {
    let pipe = create_named_pipe(unique_name("noattr"), &byte_duplex(), None).expect("a pipe");
    assert!(
        !get_handle_information(&pipe)
            .expect("flags")
            .contains(HandleFlags::INHERIT)
    );
}

#[test]
fn attributes_that_inherit_give_a_handle_that_can_be() {
    let attributes = SecurityAttributes::new(None, true);
    let pipe = create_named_pipe(unique_name("inherit"), &byte_duplex(), Some(&attributes))
        .expect("a pipe");
    assert!(
        get_handle_information(&pipe)
            .expect("flags")
            .contains(HandleFlags::INHERIT)
    );
}

#[test]
fn attributes_that_do_not_inherit_give_a_handle_that_cannot_be() {
    let attributes = SecurityAttributes::new(None, false);
    let pipe = create_named_pipe(unique_name("noinherit"), &byte_duplex(), Some(&attributes))
        .expect("a pipe");
    assert!(
        !get_handle_information(&pipe)
            .expect("flags")
            .contains(HandleFlags::INHERIT)
    );
}

// --- Overlapped -----------------------------------------------------------------------------

#[test]
fn an_overlapped_pipe_pends_a_read_and_cancels_it() {
    let name = unique_name("overlapped");
    let options = NamedPipeOptions {
        open_mode: PipeOpenMode::ACCESS_DUPLEX | PipeOpenMode::OVERLAPPED,
        ..byte_duplex()
    };
    let server = create_named_pipe(&name, &options, None).expect("an overlapped pipe");
    let _client = client(&name, true, true).expect("a client");

    let mut buffer = [0u8; 8];
    // SAFETY: all-zero is a valid `OVERLAPPED`.
    let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
    // SAFETY: the handle is open; `buffer` and `overlapped` live until the operation has finished,
    // which is waited for below before either goes out of scope.
    let started = unsafe {
        ReadFile(
            server.as_raw_handle(),
            buffer.as_mut_ptr().cast(),
            buffer.len() as u32,
            std::ptr::null_mut(),
            &raw mut overlapped,
        )
    };
    assert_eq!(
        started, 0,
        "no data is there, so the read does not complete at once"
    );
    assert_eq!(
        code(&std::io::Error::last_os_error()),
        ERROR_IO_PENDING,
        "an overlapped handle starts the read and returns"
    );

    let mut transferred = 0u32;
    // SAFETY: as above; the cancel names the one operation, and the wait is to its completion.
    unsafe {
        assert_ne!(CancelIoEx(server.as_raw_handle(), &raw const overlapped), 0);
        let finished = GetOverlappedResult(
            server.as_raw_handle(),
            &raw const overlapped,
            &mut transferred,
            TRUE,
        );
        assert_eq!(finished, 0, "a cancelled read does not transfer");
    }
    assert_eq!(
        code(&std::io::Error::last_os_error()),
        ERROR_OPERATION_ABORTED
    );
}

// --- The name -------------------------------------------------------------------------------

#[test]
fn a_name_with_an_interior_nul_is_refused_before_the_call() {
    let error = create_named_pipe("\\\\.\\pipe\\a\0b", &byte_duplex(), None).expect_err("a NUL");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    assert_eq!(error.raw_os_error(), None, "the call was not made");
}

#[test]
fn a_name_that_is_not_a_pipe_name_is_the_calls_own_refusal() {
    let error = create_named_pipe(r"C:\not-a-pipe", &byte_duplex(), None).expect_err("not a pipe");
    assert!(error.raw_os_error().is_some(), "{error}");
}

#[test]
fn a_name_that_is_not_valid_utf16_is_passed_as_it_is() {
    // A lone surrogate, which a `String` cannot hold and an `OsString` can: the name reaches the call
    // unchanged and not replaced by U+FFFD, so the same wide name opens the pipe.
    let mut units: Vec<u16> = unique_name("wtf").encode_utf16().collect();
    units.push(0xD800);
    let name = OsString::from_wide(&units);

    let _server = create_named_pipe(&name, &byte_duplex(), None).expect("a pipe with that name");
    OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(FILE_SHARE_ALL)
        .open(&name)
        .expect("a client opens the same wide name");

    let replaced = String::from_utf16_lossy(&units);
    assert!(
        client(&replaced, true, true).is_err(),
        "the replacement character is a different name"
    );
}
