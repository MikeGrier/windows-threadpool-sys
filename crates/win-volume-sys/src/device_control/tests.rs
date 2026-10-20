// Copyright (c) 2026 Mike Grier

//! Tests for the one `DeviceIoControl` helper.
//!
//! `FSCTL_GET_COMPRESSION` takes no input and writes a `USHORT`, and every file on an NTFS or ReFS
//! volume answers it, so a test that needs the call to *succeed* asserts that and does not stand
//! down. `IOCTL_DISK_GET_DRIVE_GEOMETRY` on a file that is not a volume is a control that fails
//! with the device's own code, which is what the error tests need.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::mem::{offset_of, size_of};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::PathBuf;
use std::ptr;
use std::thread;
use std::time::{Duration, Instant};

use win_error_codes::Win32Error;
use win_pipes_sys::{NamedPipeOptions, PipeMode, PipeOpenMode, create_named_pipe};
use windows_sys::Win32::Foundation::{
    ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_NOT_ENOUGH_MEMORY, ERROR_VIRTDISK_NOT_VIRTUAL_DISK,
    FALSE, GetLastError, INVALID_HANDLE_VALUE, SetLastError, TRUE,
};
use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_OVERLAPPED, ReadFile};
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Ioctl::{
    FSCTL_GET_COMPRESSION, FSCTL_REQUEST_OPLOCK, IOCTL_DISK_GET_DRIVE_GEOMETRY,
    OPLOCK_LEVEL_CACHE_READ, REQUEST_OPLOCK_CURRENT_VERSION, REQUEST_OPLOCK_INPUT_BUFFER,
    REQUEST_OPLOCK_INPUT_FLAG_REQUEST, REQUEST_OPLOCK_OUTPUT_BUFFER,
};

use super::{device_control, length, overlapped_call, u32_at, zeroed_output};
use crate::test_support::ordinary_file;

/// The size of the `USHORT` `FSCTL_GET_COMPRESSION` writes.
const COMPRESSION_SIZE: usize = 2;

/// A temporary file that is removed when it goes, opened plain.
struct Scratch {
    file: File,
    path: PathBuf,
}

impl Scratch {
    fn new(tag: &str) -> Self {
        let (file, path) = ordinary_file(tag);
        Self { file, path }
    }

    /// The same file opened with `FILE_FLAG_OVERLAPPED`.
    fn overlapped(&self) -> File {
        OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_OVERLAPPED)
            .open(&self.path)
            .expect("open the file overlapped")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn code_of(error: &io::Error) -> u32 {
    Win32Error::from_io(error)
        .expect("an error from a control carries an OS code")
        .code()
}

fn control(file: &File, code: u32, buffer: &mut [u8]) -> Result<usize, u32> {
    device_control(file, code, &[], buffer).map_err(|error| code_of(&error))
}

fn compression(file: &File, buffer: &mut [u8]) -> Result<usize, u32> {
    control(file, FSCTL_GET_COMPRESSION, buffer)
}

fn geometry(file: &File, buffer: &mut [u8]) -> Result<usize, u32> {
    control(file, IOCTL_DISK_GET_DRIVE_GEOMETRY, buffer)
}

#[test]
fn a_control_that_succeeds_reports_the_bytes_the_device_wrote() {
    let scratch = Scratch::new("dc-written");
    let mut buffer = [0xAA_u8; COMPRESSION_SIZE];

    assert_eq!(
        compression(&scratch.file, &mut buffer),
        Ok(2),
        "the answer is a USHORT"
    );
}

#[test]
fn an_overlapped_handle_gets_the_same_success_and_the_same_bytes() {
    let scratch = Scratch::new("dc-overlapped-ok");
    let overlapped = scratch.overlapped();

    let mut from_plain = [0xAA_u8; COMPRESSION_SIZE];
    assert_eq!(compression(&scratch.file, &mut from_plain), Ok(2));

    for _ in 0..8 {
        let mut from_overlapped = [0xAA_u8; COMPRESSION_SIZE];
        assert_eq!(compression(&overlapped, &mut from_overlapped), Ok(2));
        assert_eq!(from_overlapped, from_plain);
    }
}

#[test]
fn a_buffer_smaller_than_the_answer_is_an_error_and_not_a_success() {
    let scratch = Scratch::new("dc-compression-small");

    // Which code a control uses for this is its own; what is checked is that a buffer with no
    // room for the answer is never reported as one.
    let outcome = compression(&scratch.file, &mut [0_u8; 0]);
    assert!(outcome.is_err(), "no room for the answer: {outcome:?}");
}

#[test]
fn a_plain_handle_and_an_overlapped_one_get_the_same_refusal() {
    let scratch = Scratch::new("dc-both");
    let overlapped = scratch.overlapped();

    let from_plain = geometry(&scratch.file, &mut [0_u8; 64]);
    let from_overlapped = geometry(&overlapped, &mut [0_u8; 64]);

    assert!(from_plain.is_err(), "an ordinary file has no geometry");
    assert_eq!(from_plain, from_overlapped);
}

#[test]
fn an_overlapped_handle_is_never_answered_with_the_pending_code() {
    // A call still running is waited for, so the caller sees the result and not `ERROR_IO_PENDING`.
    // A real control was measured to pend only while a suite ran in parallel, so this reaches the
    // pending path on some runs and not others. The pending path itself is pinned deterministically
    // by the pipe tests below.
    let scratch = Scratch::new("dc-pending");
    let overlapped = scratch.overlapped();
    for _ in 0..8 {
        assert_ne!(
            geometry(&overlapped, &mut [0_u8; 64]),
            Err(ERROR_IO_PENDING)
        );
        assert_ne!(
            compression(&overlapped, &mut [0_u8; COMPRESSION_SIZE]),
            Err(ERROR_IO_PENDING)
        );
    }
}

#[test]
fn a_refused_control_reports_the_code_a_direct_call_gets() {
    // Not only that an error comes back, but that it is the code the call set: a direct call on a
    // plain handle, with no OVERLAPPED, gives the code to compare against.
    let scratch = Scratch::new("dc-code");
    let mut direct_buffer = [0_u8; 64];
    let mut written = 0_u32;
    // SAFETY: the file is open, the buffer is writable for its length, and no input or OVERLAPPED
    // is passed to a handle that was not opened overlapped.
    let direct_ok = unsafe {
        DeviceIoControl(
            scratch.file.as_raw_handle(),
            IOCTL_DISK_GET_DRIVE_GEOMETRY,
            ptr::null(),
            0,
            direct_buffer.as_mut_ptr().cast(),
            u32::try_from(direct_buffer.len()).expect("small"),
            &raw mut written,
            ptr::null_mut(),
        )
    };
    assert_eq!(direct_ok, FALSE, "an ordinary file has no geometry");
    // SAFETY: read straight after the call that set it.
    let direct = unsafe { GetLastError() };

    let helper = geometry(&scratch.file, &mut [0_u8; 64]).expect_err("refused");

    assert_eq!(helper, direct);
    assert_ne!(helper, ERROR_IO_PENDING);
}

#[test]
fn the_output_buffer_is_written_through_and_nothing_beyond_it() {
    let scratch = Scratch::new("dc-bounds");
    let mut guarded = [0x5A_u8; 8];
    let (head, rest) = guarded.split_at_mut(COMPRESSION_SIZE);

    assert_eq!(compression(&scratch.file, head), Ok(2));
    assert!(
        rest.iter().all(|byte| *byte == 0x5A),
        "the device wrote past the buffer it was given"
    );
}

// --- What the helper hands the call, and what it makes of the call's failure -------------------

#[test]
fn the_call_is_given_the_buffer_its_length_and_an_event_that_suppresses_completion_packets() {
    let mut output = [0_u8; 24];
    let output_address = output.as_ptr().addr();
    let mut seen = None;

    let outcome = overlapped_call(
        INVALID_HANDLE_VALUE,
        &mut output,
        |buffer, length, written, overlapped| {
            // SAFETY: the four pointers are the ones `overlapped_call` guarantees valid for the call.
            unsafe {
                seen = Some((
                    buffer.addr(),
                    length,
                    (*overlapped).hEvent.addr() & 1,
                    (*overlapped).Internal,
                ));
                *written = 5;
            }
            TRUE
        },
    );

    assert_eq!(outcome.expect("the call succeeded"), 5);
    assert_eq!(
        seen,
        Some((output_address, 24, 1, 0)),
        "the buffer, its whole length, the event's low bit set, and a zeroed OVERLAPPED"
    );
}

#[test]
fn a_call_that_fails_reports_the_code_it_set_exactly_for_every_code() {
    // The defect `win-error-codes` exists to remove: a code of 2^31 or more must survive the trip
    // through io::Error. ERROR_VIRTDISK_NOT_VIRTUAL_DISK is one a real call returned.
    for code in [
        1_u32,
        2,
        5,
        87,
        122,
        1117,
        0xFFFF,
        0x7FFF_FFFF,
        0x8000_0000,
        ERROR_VIRTDISK_NOT_VIRTUAL_DISK,
        u32::MAX,
    ] {
        let error = overlapped_call(INVALID_HANDLE_VALUE, &mut [0_u8; 4], |_, _, _, _| {
            // SAFETY: `SetLastError` writes only this thread's own error slot.
            unsafe { SetLastError(code) };
            FALSE
        })
        .expect_err("the call failed");

        assert_eq!(
            error.raw_os_error(),
            Some(code.cast_signed()),
            "code {code:#010X}"
        );
        assert_eq!(code_of(&error), code, "code {code:#010X}");
    }
}

#[test]
fn a_failure_that_is_not_pending_is_returned_and_not_waited_for() {
    // If it were waited for, this would hang on an event nothing signals.
    let started = Instant::now();
    let error = overlapped_call(INVALID_HANDLE_VALUE, &mut [0_u8; 4], |_, _, _, _| {
        // SAFETY: as above.
        unsafe { SetLastError(5) };
        FALSE
    })
    .expect_err("the call failed");

    assert_eq!(code_of(&error), 5);
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn a_call_that_reports_success_returns_the_bytes_it_wrote_and_nothing_else() {
    for count in [0_u32, 1, 4, 100] {
        let mut output = [0_u8; 128];
        let written = overlapped_call(INVALID_HANDLE_VALUE, &mut output, |_, _, written, _| {
            // SAFETY: `written` is valid for the call.
            unsafe { *written = count };
            TRUE
        })
        .expect("success");

        assert_eq!(written, count);
    }
}

#[test]
fn a_length_over_what_a_u32_counts_is_refused_and_one_that_fits_is_not() {
    assert_eq!(length(0, "x").expect("zero"), 0);
    assert_eq!(length(100, "x").expect("small"), 100);
    assert_eq!(
        length(u32::MAX as usize, "x").expect("the largest"),
        u32::MAX
    );

    if let Ok(over) = usize::try_from(u64::from(u32::MAX) + 1) {
        let error = length(over, "a buffer too long").expect_err("too long for a u32");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("a buffer too long"), "{error}");
    }
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
    let name = format!(r"\\.\pipe\wvs-dc-{}-{label}", std::process::id());
    let options = NamedPipeOptions {
        open_mode: PipeOpenMode::ACCESS_DUPLEX | PipeOpenMode::OVERLAPPED,
        pipe_mode: PipeMode::TYPE_BYTE | PipeMode::WAIT,
        max_instances: 1,
        out_buffer_size: 64,
        in_buffer_size: 64,
        default_timeout_ms: 0,
    };
    let server =
        File::from(create_named_pipe(&name, &options, None).expect("create the named pipe"));
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
    .map_err(|error| code_of(&error))
}

#[test]
fn a_call_that_stays_pending_is_waited_for_and_reports_the_bytes_it_wrote() {
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
    let (server, client) = pipe_pair("immediate");
    (&client).write_all(b"xyz").expect("write to the pipe");

    let mut output = [0_u8; 8];
    assert_eq!(read_through_the_helper(&server, &mut output), Ok(3));
    assert_eq!(&output[..3], b"xyz");
}

#[test]
fn a_call_that_fails_after_pending_reports_its_own_code() {
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
// completes when another handle writes to the file. So it is the one test that reaches
// `DeviceIoControl` itself through the pending path, and the one that shows the input arriving, and
// the one that tells a call made with an `OVERLAPPED` from one made without.

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
    let scratch = Scratch::new("dc-oplock");
    let file = scratch.overlapped();
    let path = scratch.path.clone();
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
        &file,
        FSCTL_REQUEST_OPLOCK,
        &oplock_request(OPLOCK_LEVEL_CACHE_READ),
        &mut output,
    );
    let waited = started.elapsed();

    assert_eq!(
        written.map_err(|error| code_of(&error)),
        Ok(output.len()),
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
    let scratch = Scratch::new("dc-oplock-none");
    let file = scratch.overlapped();

    let mut output = [0_u8; size_of::<REQUEST_OPLOCK_OUTPUT_BUFFER>()];
    let outcome = device_control(&file, FSCTL_REQUEST_OPLOCK, &[], &mut output);

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
    let error = zeroed_output(isize::MAX as usize).expect_err("no machine has that much");
    assert_eq!(code_of(&error), ERROR_NOT_ENOUGH_MEMORY);
}
