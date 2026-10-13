// Copyright (c) 2026 Mike Grier
// Split from error_code/tests.rs at a8947da8.
//! What `error_code_of` reads out of an `io::Error`, and the fault the Windows path builds from it.

use std::io;

use durable_ioring::ErrorCode;

use super::{WINDOWS_IORING, error_code_of, windows_fault};

/// `ERROR_IO_DEVICE`.
const IO_DEVICE: u32 = 1117;

#[test]
fn an_error_carrying_a_win32_code_reads_as_that_code() {
    let error = io::Error::from_raw_os_error(IO_DEVICE.cast_signed());
    assert_eq!(
        error_code_of(&error),
        Some(ErrorCode::from_win32(IO_DEVICE))
    );
}

#[test]
fn an_error_carrying_no_code_reads_as_none() {
    assert_eq!(
        error_code_of(&io::Error::other("a provider's message")),
        None
    );
    assert_eq!(
        error_code_of(&io::Error::from(io::ErrorKind::PermissionDenied)),
        None
    );
}

#[test]
fn a_windows_fault_names_the_ring_and_carries_the_code_of_its_error() {
    let fault = windows_fault(io::Error::from_raw_os_error(IO_DEVICE.cast_signed()));
    assert_eq!(fault.source, WINDOWS_IORING);
    assert_eq!(fault.code, Some(ErrorCode::from_win32(IO_DEVICE)));
    assert_eq!(fault.error.raw_os_error(), Some(IO_DEVICE.cast_signed()));
}

#[test]
fn a_windows_fault_for_an_error_with_no_code_carries_none() {
    let fault = windows_fault(io::Error::other("no code"));
    assert_eq!(fault.source, WINDOWS_IORING);
    assert_eq!(fault.code, None);
}
