// Copyright (c) 2026 Mike Grier
//! `ErrorCode`'s arithmetic, and what it reads out of an `io::Error`.

use std::io;

use super::ErrorCode;

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
fn a_win32_code_is_wrapped_as_hresult_from_win32_wraps_it() {
    assert_eq!(
        ErrorCode::from_win32(IO_DEVICE).hresult().cast_unsigned(),
        0x8007_045D
    );
    assert_eq!(
        ErrorCode::from_win32(ACCESS_DENIED)
            .hresult()
            .cast_unsigned(),
        0x8007_0005
    );
}

#[test]
fn every_win32_code_round_trips() {
    for code in [1, ACCESS_DENIED, 87, IO_DEVICE, 0x7FFF, 0xFFFF] {
        assert_eq!(ErrorCode::from_win32(code).win32(), Some(code), "{code}");
    }
}

#[test]
fn a_value_already_an_hresult_is_taken_as_it_is() {
    assert_eq!(ErrorCode::from_win32(0).hresult(), 0);
    assert_eq!(
        ErrorCode::from_win32(OTHER_FACILITY),
        ErrorCode::from_hresult(OTHER_FACILITY.cast_signed())
    );
}

#[test]
fn only_the_low_sixteen_bits_of_a_win32_code_survive_the_wrapping() {
    assert_eq!(
        // Bit 20, outside the facility value 7 occupies, so only the mask can remove it.
        ErrorCode::from_win32(0x0010_0000 | IO_DEVICE),
        ErrorCode::from_win32(IO_DEVICE)
    );
}

#[test]
fn an_hresult_of_win32_facility_names_its_win32_code() {
    assert_eq!(
        ErrorCode::from_hresult(E_INVALIDARG.cast_signed()).win32(),
        Some(87)
    );
}

#[test]
fn an_hresult_wrapping_no_win32_code_names_none() {
    assert_eq!(
        ErrorCode::from_hresult(OTHER_FACILITY.cast_signed()).win32(),
        None
    );
    assert_eq!(ErrorCode::from_hresult(0).win32(), None, "S_OK is no error");
}

#[test]
fn an_error_carrying_a_win32_code_reads_as_that_code() {
    let error = io::Error::from_raw_os_error(IO_DEVICE.cast_signed());
    assert_eq!(
        ErrorCode::of(&error),
        Some(ErrorCode::from_win32(IO_DEVICE))
    );
}

#[test]
fn an_error_carrying_no_code_reads_as_none() {
    assert_eq!(
        ErrorCode::of(&io::Error::other("a provider's message")),
        None
    );
    assert_eq!(
        ErrorCode::of(&io::Error::from(io::ErrorKind::PermissionDenied)),
        None
    );
}

#[test]
fn a_code_prints_as_its_hresult_in_hex() {
    let code = ErrorCode::from_win32(IO_DEVICE);
    assert_eq!(code.to_string(), "0x8007045D");
    assert_eq!(format!("{code:?}"), "ErrorCode(0x8007045D)");
}
