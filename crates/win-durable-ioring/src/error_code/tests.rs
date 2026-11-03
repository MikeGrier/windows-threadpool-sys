// Copyright (c) 2026 Mike Grier
// Split from error_code/tests.rs at 4fbcb343.
//! What `error_code_of` reads out of an `io::Error`, the fault the Windows path builds from it, and
//! how a Win32 code is wrapped into an `HRESULT` and read back (DI-9.7.11).

use std::io;

use durable_ioring::ErrorCode;
use win_error_codes::{HResult, Win32Error};

use super::{WINDOWS_IORING, error_code_from_win32, error_code_of, win32_code_of, windows_fault};

/// `ERROR_IO_DEVICE`.
const IO_DEVICE: u32 = 1117;
/// `ERROR_ACCESS_DENIED`.
const ACCESS_DENIED: u32 = 5;
/// `E_INVALIDARG`, which Windows defines in the Win32 facility: `HRESULT_FROM_WIN32` of
/// `ERROR_INVALID_PARAMETER` (87).
const E_INVALIDARG: u32 = 0x8007_0057;
/// A failure `HRESULT` of a facility other than Win32, so it wraps no Win32 code.
const OTHER_FACILITY: u32 = 0x8046_0005;

#[test]
fn an_error_carrying_a_win32_code_reads_as_that_code() {
    let error = io::Error::from_raw_os_error(IO_DEVICE.cast_signed());
    assert_eq!(
        error_code_of(&error),
        Some(error_code_from_win32(IO_DEVICE))
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
    assert_eq!(fault.code, Some(error_code_from_win32(IO_DEVICE)));
    assert_eq!(
        fault
            .error
            .downcast_ref::<io::Error>()
            .and_then(io::Error::raw_os_error),
        Some(IO_DEVICE.cast_signed())
    );
}

#[test]
fn a_windows_fault_for_an_error_with_no_code_carries_none() {
    let fault = windows_fault(io::Error::other("no code"));
    assert_eq!(fault.source, WINDOWS_IORING);
    assert_eq!(fault.code, None);
}

#[test]
fn a_win32_code_is_wrapped_as_hresult_from_win32_wraps_it() {
    assert_eq!(
        error_code_from_win32(IO_DEVICE).hresult().cast_unsigned(),
        0x8007_045D
    );
    assert_eq!(
        error_code_from_win32(ACCESS_DENIED)
            .hresult()
            .cast_unsigned(),
        0x8007_0005
    );
}

#[test]
fn every_win32_code_from_one_to_sixteen_bits_round_trips() {
    for code in [1, ACCESS_DENIED, 87, IO_DEVICE, 0x7FFF, 0xFFFF] {
        assert_eq!(
            win32_code_of(error_code_from_win32(code)),
            Some(code),
            "{code}"
        );
    }
}

#[test]
fn a_value_already_an_hresult_is_taken_as_it_is() {
    assert_eq!(error_code_from_win32(0).hresult(), 0);
    assert_eq!(
        error_code_from_win32(OTHER_FACILITY),
        ErrorCode::from_hresult(OTHER_FACILITY.cast_signed())
    );
}

#[test]
fn only_the_low_sixteen_bits_of_a_win32_code_survive_the_wrapping() {
    assert_eq!(
        // Bit 20, outside the facility value 7 occupies, so only the mask can remove it.
        error_code_from_win32(0x0010_0000 | IO_DEVICE),
        error_code_from_win32(IO_DEVICE)
    );
}

#[test]
fn an_hresult_of_win32_facility_names_its_win32_code() {
    assert_eq!(
        win32_code_of(ErrorCode::from_hresult(E_INVALIDARG.cast_signed())),
        Some(87)
    );
}

#[test]
fn an_hresult_wrapping_no_win32_code_names_none() {
    assert_eq!(
        win32_code_of(ErrorCode::from_hresult(OTHER_FACILITY.cast_signed())),
        None
    );
    assert_eq!(
        win32_code_of(ErrorCode::from_hresult(0)),
        None,
        "S_OK is no error"
    );
}

#[test]
fn the_wrapping_is_the_one_win_error_codes_defines() {
    // The rule is that crate's, and this crate states none of its own, so the two agree on every
    // code tried, the three regimes of it among them: zero, a value that is not positive as an
    // `i32`, and the rest.
    for code in [
        0,
        1,
        ACCESS_DENIED,
        IO_DEVICE,
        0xFFFF,
        0x0001_0000,
        0x0010_045D,
        0x7FFF_FFFF,
        0x8000_0000,
        0xC03A_0015,
        u32::MAX,
    ] {
        assert_eq!(
            error_code_from_win32(code).hresult(),
            HResult::from_win32(Win32Error::from_code(code)).raw(),
            "{code:#X}"
        );
    }
    for hresult in [0, 1, E_INVALIDARG, OTHER_FACILITY, 0x8007_FFFF, 0x8008_0005] {
        assert_eq!(
            win32_code_of(ErrorCode::from_hresult(hresult.cast_signed())),
            HResult::from_raw(hresult.cast_signed())
                .win32()
                .map(Win32Error::code),
            "{hresult:#X}"
        );
    }
}
