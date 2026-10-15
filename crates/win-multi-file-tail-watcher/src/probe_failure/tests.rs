// Copyright (c) 2026 Mike Grier
use std::io;

use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_FILE_INVALID, ERROR_FILE_NOT_FOUND, ERROR_GEN_FAILURE,
    ERROR_INVALID_HANDLE, ERROR_INVALID_PARAMETER, ERROR_NETNAME_DELETED, ERROR_NOT_READY,
    ERROR_OPERATION_ABORTED, ERROR_SHARING_VIOLATION, ERROR_VC_DISCONNECTED,
};

use super::{FailureClass, TERMINAL_CODES, classify, win32_code};

fn win32(code: u32) -> io::Error {
    io::Error::from_raw_os_error(code.cast_signed())
}

#[test]
fn an_invalid_handle_is_terminal() {
    assert_eq!(
        classify(&win32(ERROR_INVALID_HANDLE)),
        FailureClass::Terminal
    );
}

#[test]
fn an_invalidated_file_is_terminal() {
    assert_eq!(classify(&win32(ERROR_FILE_INVALID)), FailureClass::Terminal);
}

#[test]
fn every_listed_code_is_terminal_and_none_is_listed_twice() {
    for (index, &code) in TERMINAL_CODES.iter().enumerate() {
        assert_eq!(classify(&win32(code)), FailureClass::Terminal, "{code}");
        assert!(!TERMINAL_CODES[..index].contains(&code), "{code} twice");
    }
}

#[test]
fn codes_that_may_pass_are_transient() {
    for code in [
        ERROR_ACCESS_DENIED,
        ERROR_FILE_NOT_FOUND,
        ERROR_GEN_FAILURE,
        ERROR_INVALID_PARAMETER,
        ERROR_NETNAME_DELETED,
        ERROR_NOT_READY,
        ERROR_OPERATION_ABORTED,
        ERROR_SHARING_VIOLATION,
        ERROR_VC_DISCONNECTED,
    ] {
        assert_eq!(classify(&win32(code)), FailureClass::Transient, "{code}");
    }
}

#[test]
fn the_codes_either_side_of_a_terminal_one_are_transient() {
    for terminal in TERMINAL_CODES {
        for code in [terminal - 1, terminal + 1] {
            if !TERMINAL_CODES.contains(&code) {
                assert_eq!(classify(&win32(code)), FailureClass::Transient, "{code}");
            }
        }
    }
}

#[test]
fn zero_and_the_extremes_are_transient() {
    for code in [0u32, 1, u32::MAX, i32::MAX.cast_unsigned(), 0x8000_0000] {
        assert_eq!(classify(&win32(code)), FailureClass::Transient, "{code:#x}");
    }
}

#[test]
fn an_error_with_no_code_is_transient() {
    assert_eq!(
        classify(&io::Error::other("no code")),
        FailureClass::Transient
    );
    assert_eq!(
        classify(&io::Error::from(io::ErrorKind::NotFound)),
        FailureClass::Transient
    );
    assert_eq!(
        classify(&io::Error::new(
            io::ErrorKind::InvalidInput,
            "still no code"
        )),
        FailureClass::Transient
    );
}

#[test]
fn a_hresult_is_not_read_as_a_win32_code() {
    let hresult_invalid_handle = 0x8007_0006u32;
    assert_eq!(
        classify(&win32(hresult_invalid_handle)),
        FailureClass::Transient
    );
}

#[test]
fn only_terminal_is_terminal() {
    assert!(FailureClass::Terminal.is_terminal());
    assert!(!FailureClass::Transient.is_terminal());
}

#[test]
fn the_code_is_read_back_unchanged() {
    for code in [5u32, 6, 1006, 0x8007_0006, u32::MAX] {
        assert_eq!(win32_code(&win32(code)), Some(code));
    }
}

#[test]
fn an_error_with_no_code_reads_back_as_none() {
    assert_eq!(win32_code(&io::Error::other("no code")), None);
    assert_eq!(win32_code(&io::Error::from(io::ErrorKind::TimedOut)), None);
}
