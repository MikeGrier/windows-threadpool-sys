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
use std::mem::{offset_of, size_of};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::ptr;
use std::thread;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    ERROR_BROKEN_PIPE, ERROR_INSUFFICIENT_BUFFER, ERROR_IO_PENDING, ERROR_NOT_ENOUGH_MEMORY,
    INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX, ReadFile};
use windows_sys::Win32::System::Ioctl::{
    FSCTL_GET_COMPRESSION, FSCTL_QUERY_VOLUME_NUMA_INFO, FSCTL_REQUEST_OPLOCK,
    OPLOCK_LEVEL_CACHE_READ, REQUEST_OPLOCK_CURRENT_VERSION, REQUEST_OPLOCK_INPUT_BUFFER,
    REQUEST_OPLOCK_INPUT_FLAG_REQUEST, REQUEST_OPLOCK_OUTPUT_BUFFER,
};
use windows_sys::Win32::System::Pipes::{CreateNamedPipeW, PIPE_TYPE_BYTE, PIPE_WAIT};

use super::{device_control, overlapped_call, u32_at, zeroed_output};
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
    device_control(file.as_raw_handle(), code, &[], buffer).map_err(|error| error.code())
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

// --- A control that takes input and pends on demand -----------------------------------------
//
// `FSCTL_REQUEST_OPLOCK` is both: it is sent a request, stays pending while the oplock is held, and
// completes when another handle writes to the file. So it is the one test that reaches `DeviceIoControl`
// itself through the pending path, and the one that shows the input buffer arriving.

/// `REQUEST_OPLOCK_INPUT_BUFFER` for a request of `level`, written field by field so no byte view of
/// a struct is needed.
fn oplock_request(level: u32) -> [u8; size_of::<REQUEST_OPLOCK_INPUT_BUFFER>()] {
    let mut input = [0_u8; size_of::<REQUEST_OPLOCK_INPUT_BUFFER>()];
    let mut put =
        |offset: usize, bytes: &[u8]| input[offset..offset + bytes.len()].copy_from_slice(bytes);
    put(
        offset_of!(REQUEST_OPLOCK_INPUT_BUFFER, StructureVersion),
        &u16::try_from(REQUEST_OPLOCK_CURRENT_VERSION)
            .expect("a u16")
            .to_ne_bytes(),
    );
    put(
        offset_of!(REQUEST_OPLOCK_INPUT_BUFFER, StructureLength),
        &u16::try_from(size_of::<REQUEST_OPLOCK_INPUT_BUFFER>())
            .expect("a u16")
            .to_ne_bytes(),
    );
    put(
        offset_of!(REQUEST_OPLOCK_INPUT_BUFFER, RequestedOplockLevel),
        &level.to_ne_bytes(),
    );
    put(
        offset_of!(REQUEST_OPLOCK_INPUT_BUFFER, Flags),
        &REQUEST_OPLOCK_INPUT_FLAG_REQUEST.to_ne_bytes(),
    );
    input
}

#[test]
fn a_request_with_input_pends_until_the_device_answers_and_reports_what_it_wrote() {
    let _allocating = handle_allocation().read().expect("not poisoned");
    let fixture = Fixture::new("dc-oplock");
    let file = overlapped(&fixture);
    let path = fixture.file();
    let writer = thread::spawn(move || {
        thread::sleep(WRITER_DELAY);
        let mut other = OpenOptions::new()
            .write(true)
            .open(path)
            .expect("open to write");
        // A write through another handle is what breaks a read oplock.
        other.write_all(b"x").expect("write");
        other
    });

    let mut output = [0_u8; size_of::<REQUEST_OPLOCK_OUTPUT_BUFFER>()];
    let started = Instant::now();
    let written = device_control(
        file.as_raw_handle(),
        FSCTL_REQUEST_OPLOCK,
        &oplock_request(OPLOCK_LEVEL_CACHE_READ),
        &mut output,
    );
    let waited = started.elapsed();

    assert_eq!(
        written.map_err(|error| error.code()),
        Ok(u32::try_from(output.len()).expect("a u32")),
        "the whole output structure was written"
    );
    assert!(
        waited >= WRITER_DELAY / 2,
        "returned after {waited:?}, so it did not wait for the oplock to break"
    );
    assert_eq!(
        u32_at(
            &output,
            offset_of!(REQUEST_OPLOCK_OUTPUT_BUFFER, OriginalOplockLevel)
        ),
        Some(OPLOCK_LEVEL_CACHE_READ),
        "the oplock that was granted is the one the input asked for"
    );
    assert_eq!(
        u32_at(
            &output,
            offset_of!(REQUEST_OPLOCK_OUTPUT_BUFFER, NewOplockLevel)
        ),
        Some(0),
        "and it broke to none"
    );
    drop(writer.join().expect("the writer thread"));
}

#[test]
fn input_that_does_not_arrive_is_refused_by_the_device() {
    // The control above passing shows the input reaches the device; this shows it is the input that
    // made it work, by sending the same control with none and seeing the device refuse.
    let _allocating = handle_allocation().read().expect("not poisoned");
    let fixture = Fixture::new("dc-oplock-none");
    let file = overlapped(&fixture);

    let mut output = [0_u8; size_of::<REQUEST_OPLOCK_OUTPUT_BUFFER>()];
    let outcome = device_control(file.as_raw_handle(), FSCTL_REQUEST_OPLOCK, &[], &mut output);

    assert!(
        outcome.is_err(),
        "a request with no input was accepted: {outcome:?}"
    );
}

// --- The shared reply helpers ----------------------------------------------------------------

#[test]
fn a_u32_is_read_at_an_offset_only_where_it_lies_entirely_within_the_buffer() {
    let bytes: Vec<u8> = (1_u8..=8).collect();

    assert_eq!(u32_at(&bytes, 0), Some(u32::from_ne_bytes([1, 2, 3, 4])));
    assert_eq!(u32_at(&bytes, 4), Some(u32::from_ne_bytes([5, 6, 7, 8])));
    assert_eq!(u32_at(&bytes, 1), Some(u32::from_ne_bytes([2, 3, 4, 5])));
    // One byte short, past the end, empty, and an offset whose end overflows.
    assert_eq!(u32_at(&bytes, 5), None);
    assert_eq!(u32_at(&bytes, 8), None);
    assert_eq!(u32_at(&[], 0), None);
    assert_eq!(u32_at(&bytes, usize::MAX), None);
    assert_eq!(u32_at(&bytes, usize::MAX - 1), None);
}

#[test]
fn a_reply_buffer_is_zeroed_and_the_size_the_machine_cannot_supply_is_out_of_memory() {
    assert_eq!(zeroed_output(0).expect("an empty buffer"), Vec::<u8>::new());
    assert_eq!(zeroed_output(40).expect("a small buffer"), vec![0_u8; 40]);

    // `isize::MAX` is the most any allocation can name, and no machine has it.
    let error =
        zeroed_output(usize::try_from(isize::MAX).expect("fits")).expect_err("no machine has it");
    assert_eq!(error.code(), ERROR_NOT_ENOUGH_MEMORY);
}
