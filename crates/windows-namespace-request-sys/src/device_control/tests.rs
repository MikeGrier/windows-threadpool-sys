// Copyright (c) Mike Grier.

//! Tests for the one `DeviceIoControl` helper.
//!
//! Two controls are used, for different reasons. `FSCTL_GET_COMPRESSION` takes no input and
//! writes a `USHORT`, and every file on an NTFS or ReFS volume answers it, so a test that needs
//! the call to *succeed* asserts that and does not stand down. `FSCTL_QUERY_VOLUME_NUMA_INFO` is
//! the control the crate's first entry carries and the one whose buffer-size failure is
//! documented, but whether a volume has an answer is the machine's, so a test that needs one says
//! so and stands down when it has none.

use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::ptr;
use std::thread;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    ERROR_BROKEN_PIPE, ERROR_INSUFFICIENT_BUFFER, ERROR_IO_PENDING, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX, ReadFile};
use windows_sys::Win32::System::Ioctl::{FSCTL_GET_COMPRESSION, FSCTL_QUERY_VOLUME_NUMA_INFO};
use windows_sys::Win32::System::Pipes::{CreateNamedPipeW, PIPE_TYPE_BYTE, PIPE_WAIT};

use super::{device_control, overlapped_call};
use crate::handle::tests::{Fixture, handle_allocation};

/// The size of the `USHORT` `FSCTL_GET_COMPRESSION` writes.
const COMPRESSION_SIZE: usize = 2;

fn overlapped(fixture: &Fixture) -> File {
    OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OVERLAPPED)
        .open(fixture.file())
        .expect("open the file overlapped")
}

fn control(file: &File, code: u32, buffer: &mut [u8]) -> Result<u32, u32> {
    device_control(file.as_raw_handle(), code, buffer).map_err(|error| error.code())
}

fn numa(file: &File, buffer: &mut [u8]) -> Result<u32, u32> {
    control(file, FSCTL_QUERY_VOLUME_NUMA_INFO, buffer)
}

fn compression(file: &File, buffer: &mut [u8]) -> Result<u32, u32> {
    control(file, FSCTL_GET_COMPRESSION, buffer)
}

#[test]
fn a_control_that_succeeds_reports_the_bytes_the_device_wrote() {
    let _allocating = handle_allocation().read().expect("not poisoned");
    let fixture = Fixture::new("dc-written");
    let file = fixture.open_file();
    let mut buffer = [0xAA_u8; COMPRESSION_SIZE];

    assert_eq!(
        compression(&file, &mut buffer),
        Ok(2),
        "the answer is a USHORT"
    );
}

#[test]
fn an_overlapped_handle_gets_the_same_success_and_the_same_bytes() {
    let _allocating = handle_allocation().read().expect("not poisoned");
    let fixture = Fixture::new("dc-overlapped-ok");
    let plain = fixture.open_file();
    let overlapped = overlapped(&fixture);

    let mut from_plain = [0xAA_u8; COMPRESSION_SIZE];
    assert_eq!(compression(&plain, &mut from_plain), Ok(2));

    for _ in 0..8 {
        let mut from_overlapped = [0xAA_u8; COMPRESSION_SIZE];
        assert_eq!(compression(&overlapped, &mut from_overlapped), Ok(2));
        assert_eq!(from_overlapped, from_plain);
    }
}

#[test]
fn a_buffer_smaller_than_the_answer_is_an_error_and_not_a_success() {
    let _allocating = handle_allocation().read().expect("not poisoned");
    let fixture = Fixture::new("dc-compression-small");
    let file = fixture.open_file();

    // Which code a control uses for this is its own; what is checked is that a buffer with no
    // room for the answer is never reported as one.
    let outcome = compression(&file, &mut [0_u8; 0]);
    assert!(outcome.is_err(), "no room for the answer: {outcome:?}");
}

#[test]
fn a_buffer_smaller_than_the_documented_answer_is_the_documented_error() {
    let _allocating = handle_allocation().read().expect("not poisoned");
    let fixture = Fixture::new("dc-small");
    let file = fixture.open_file();
    if numa(&file, &mut [0_u8; 4]).is_err() {
        eprintln!("skipped: this volume has no answer, so a short buffer shows nothing");
        return;
    }

    for size in [0_usize, 1, 3] {
        let mut buffer = vec![0_u8; size];
        assert_eq!(
            numa(&file, &mut buffer),
            Err(ERROR_INSUFFICIENT_BUFFER),
            "a buffer of {size} bytes"
        );
    }
}

#[test]
fn a_plain_handle_and_an_overlapped_one_get_the_same_answer_to_a_control_with_a_failure_mode() {
    let _allocating = handle_allocation().read().expect("not poisoned");
    let fixture = Fixture::new("dc-both");
    let plain = fixture.open_file();
    let overlapped = overlapped(&fixture);

    let mut from_plain = [0_u8; 4];
    let mut from_overlapped = [0_u8; 4];
    let plain_outcome = numa(&plain, &mut from_plain);
    let overlapped_outcome = numa(&overlapped, &mut from_overlapped);

    assert_eq!(plain_outcome, overlapped_outcome);
    if plain_outcome.is_ok() {
        assert_eq!(from_plain, from_overlapped);
    }
}

#[test]
fn an_overlapped_handle_is_never_answered_with_the_pending_code() {
    let _allocating = handle_allocation().read().expect("not poisoned");
    // A call still running is waited for, so the caller sees the result and not `ERROR_IO_PENDING`.
    // A real control was measured to pend only while the suite ran in parallel, so this reaches
    // the pending path on some runs and not others. The pending path itself is pinned
    // deterministically by the pipe tests below.
    let fixture = Fixture::new("dc-pending");
    let overlapped = overlapped(&fixture);
    for _ in 0..8 {
        assert_ne!(numa(&overlapped, &mut [0_u8; 4]), Err(ERROR_IO_PENDING));
        assert_ne!(
            compression(&overlapped, &mut [0_u8; COMPRESSION_SIZE]),
            Err(ERROR_IO_PENDING)
        );
    }
}

#[test]
fn a_device_that_is_not_a_volume_reports_its_own_code() {
    let _allocating = handle_allocation().read().expect("not poisoned");
    let nul = File::open("NUL").expect("open the NUL device");
    let outcome = numa(&nul, &mut [0_u8; 4]);

    let code = outcome.expect_err("the NUL device has no NUMA node");
    assert_ne!(code, ERROR_IO_PENDING);
    eprintln!("FSCTL_QUERY_VOLUME_NUMA_INFO on the NUL device: error {code}");
}

#[test]
fn the_output_buffer_is_written_through_and_nothing_beyond_it() {
    let _allocating = handle_allocation().read().expect("not poisoned");
    let fixture = Fixture::new("dc-bounds");
    let file = fixture.open_file();
    let mut guarded = [0x5A_u8; 8];
    let (head, rest) = guarded.split_at_mut(COMPRESSION_SIZE);

    assert_eq!(compression(&file, head), Ok(2));
    assert!(
        rest.iter().all(|byte| *byte == 0x5A),
        "the device wrote past the buffer it was given"
    );
}

// --- The pending path -------------------------------------------------------------------------
//
// No file-system control pends on demand, so these hand `overlapped_call` the one call that does:
// an overlapped `ReadFile` on a named pipe, which stays pending until the other end writes. It
// takes the same four arguments a control does, and its completion, its byte count and its
// failures are documented behaviour of `ReadFile` and `GetOverlappedResult`.

/// How long the writer waits, so the read is already pending when it writes.
const WRITER_DELAY: Duration = Duration::from_millis(200);

/// A connected pair: an overlapped server end, and a client end.
fn pipe_pair(label: &str) -> (File, File) {
    let name = format!(r"\\.\pipe\wnrs-dc-{}-{label}", std::process::id());
    let wide: Vec<u16> = OsStr::new(&name).encode_wide().chain(Some(0)).collect();

    // SAFETY: `wide` is a NUL-terminated name that outlives the call, and no security
    // attributes are passed.
    let server = unsafe {
        CreateNamedPipeW(
            wide.as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
            PIPE_TYPE_BYTE | PIPE_WAIT,
            1,
            64,
            64,
            0,
            ptr::null(),
        )
    };
    assert_ne!(server, INVALID_HANDLE_VALUE, "create the named pipe");
    // SAFETY: a successful `CreateNamedPipeW` returns a handle nothing else owns.
    let server = File::from(unsafe { OwnedHandle::from_raw_handle(server) });
    let client = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&name)
        .expect("connect to the pipe");
    (server, client)
}

fn read_through_the_helper(server: &File, output: &mut [u8]) -> Result<u32, u32> {
    let handle = server.as_raw_handle();
    overlapped_call(handle, output, |buffer, length, written, overlapped| {
        // SAFETY: the pipe is open, and the four pointers are the ones `overlapped_call`
        // guarantees outlive the operation.
        unsafe { ReadFile(handle, buffer.cast(), length, written, overlapped) }
    })
    .map_err(|error| error.code())
}

#[test]
fn a_call_that_stays_pending_is_waited_for_and_reports_the_bytes_it_wrote() {
    let _allocating = handle_allocation().read().expect("not poisoned");
    let (server, client) = pipe_pair("pending");
    let writer = thread::spawn(move || {
        thread::sleep(WRITER_DELAY);
        (&client).write_all(b"abc").expect("write to the pipe");
        client
    });

    let mut output = [0xAA_u8; 8];
    let started = Instant::now();
    let outcome = read_through_the_helper(&server, &mut output);
    let waited = started.elapsed();

    assert_eq!(outcome, Ok(3), "the bytes the completed read wrote");
    assert_eq!(&output[..3], b"abc");
    assert!(
        output[3..].iter().all(|byte| *byte == 0xAA),
        "nothing past what was written is touched"
    );
    assert!(
        waited >= WRITER_DELAY / 2,
        "returned after {waited:?}, so it did not wait for the writer"
    );
    drop(writer.join().expect("the writer thread"));
}

#[test]
fn a_call_that_completes_at_once_is_not_waited_for_and_reports_its_bytes() {
    let _allocating = handle_allocation().read().expect("not poisoned");
    let (server, client) = pipe_pair("immediate");
    (&client).write_all(b"xyz").expect("write to the pipe");

    let mut output = [0_u8; 8];
    assert_eq!(read_through_the_helper(&server, &mut output), Ok(3));
    assert_eq!(&output[..3], b"xyz");
}

#[test]
fn a_call_that_fails_after_pending_reports_its_own_code() {
    let _allocating = handle_allocation().read().expect("not poisoned");
    let (server, client) = pipe_pair("broken");
    let closer = thread::spawn(move || {
        thread::sleep(WRITER_DELAY);
        drop(client);
    });

    let outcome = read_through_the_helper(&server, &mut [0_u8; 8]);

    assert_eq!(
        outcome,
        Err(ERROR_BROKEN_PIPE),
        "the pipe's own code, unaltered"
    );
    closer.join().expect("the closer thread");
}
