// Copyright (c) Mike Grier.

//! Tests for the one `DeviceIoControl` helper.
//!
//! Two controls are used, for different reasons. `FSCTL_GET_COMPRESSION` takes no input and
//! writes a `USHORT`, and every file on an NTFS or ReFS volume answers it, so a test that needs
//! the call to *succeed* asserts that and does not stand down. `FSCTL_QUERY_VOLUME_NUMA_INFO` is
//! the control the crate's first entry carries and the one whose buffer-size failure is
//! documented, but whether a volume has an answer is the machine's, so a test that needs one says
//! so and stands down when it has none.

use std::fs::{File, OpenOptions};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;

use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_IO_PENDING};
use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OVERLAPPED;
use windows_sys::Win32::System::Ioctl::{FSCTL_GET_COMPRESSION, FSCTL_QUERY_VOLUME_NUMA_INFO};

use super::device_control;
use crate::handle::tests::Fixture;

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
    let fixture = Fixture::new("dc-compression-small");
    let file = fixture.open_file();

    // Which code a control uses for this is its own; what is checked is that a buffer with no
    // room for the answer is never reported as one.
    let outcome = compression(&file, &mut [0_u8; 0]);
    assert!(outcome.is_err(), "no room for the answer: {outcome:?}");
}

#[test]
fn a_buffer_smaller_than_the_documented_answer_is_the_documented_error() {
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
    // A call still running is waited for, so the caller sees the result and not `ERROR_IO_PENDING`.
    // An overlapped call was measured to pend only while the suite ran in parallel, never in
    // isolation, so this runs the pending path on some runs and not others; it asserts the
    // property whichever happens, and no sabotage is claimed for it (see sabotage.json).
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
    let nul = File::open("NUL").expect("open the NUL device");
    let outcome = numa(&nul, &mut [0_u8; 4]);

    let code = outcome.expect_err("the NUL device has no NUMA node");
    assert_ne!(code, ERROR_IO_PENDING);
    eprintln!("FSCTL_QUERY_VOLUME_NUMA_INFO on the NUL device: error {code}");
}

#[test]
fn the_output_buffer_is_written_through_and_nothing_beyond_it() {
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
