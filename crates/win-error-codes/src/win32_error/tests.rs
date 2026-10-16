// Copyright (c) Mike Grier.

//! Tests for `Win32Error`.
//!
//! The codes below are chosen to cover the places a conversion goes wrong: zero, the ordinary
//! small codes, the 16-bit edge, the 31-bit edge, and codes that need the top bit, which std holds
//! as negative numbers and which the earlier per-crate types lost or panicked on.

use std::collections::HashSet;

use super::Win32Error;

/// `ERROR_SUCCESS`.
const SUCCESS: u32 = 0;
/// `ERROR_INVALID_FUNCTION`, the first nonzero code.
const INVALID_FUNCTION: u32 = 1;
/// `ERROR_FILE_NOT_FOUND`.
const FILE_NOT_FOUND: u32 = 2;
/// `ERROR_ACCESS_DENIED`.
const ACCESS_DENIED: u32 = 5;
/// `ERROR_INVALID_PARAMETER`.
const INVALID_PARAMETER: u32 = 87;
/// `ERROR_INSUFFICIENT_BUFFER`.
const INSUFFICIENT_BUFFER: u32 = 122;
/// `ERROR_IO_PENDING`.
const IO_PENDING: u32 = 997;
/// `ERROR_IO_DEVICE`.
const IO_DEVICE: u32 = 1117;
/// The largest value a 16-bit code holds.
const LARGEST_16_BIT: u32 = 0xFFFF;
/// The first value above 16 bits.
const FIRST_ABOVE_16_BIT: u32 = 0x1_0000;
/// The largest value an `i32` holds, so the last code std keeps as a non-negative number.
const LARGEST_NON_NEGATIVE: u32 = 0x7FFF_FFFF;
/// The first code std keeps as a negative number.
const FIRST_NEGATIVE: u32 = 0x8000_0000;
/// `ERROR_VIRTDISK_NOT_VIRTUAL_DISK`, a `WIN32_ERROR` above 2^31 that a real call returned.
const NOT_A_VIRTUAL_DISK: u32 = 0xC03A_0015;
/// Every bit set.
const ALL_BITS: u32 = u32::MAX;

const SAMPLES: [u32; 14] = [
    SUCCESS,
    INVALID_FUNCTION,
    FILE_NOT_FOUND,
    ACCESS_DENIED,
    INVALID_PARAMETER,
    INSUFFICIENT_BUFFER,
    IO_PENDING,
    IO_DEVICE,
    LARGEST_16_BIT,
    FIRST_ABOVE_16_BIT,
    LARGEST_NON_NEGATIVE,
    FIRST_NEGATIVE,
    NOT_A_VIRTUAL_DISK,
    ALL_BITS,
];

#[test]
fn a_code_is_carried_unaltered() {
    for code in SAMPLES {
        assert_eq!(Win32Error::from_code(code).code(), code);
    }
}

#[test]
fn the_constructor_and_accessor_work_in_a_constant() {
    const ERROR: Win32Error = Win32Error::from_code(NOT_A_VIRTUAL_DISK);

    const {
        assert!(ERROR.code() == NOT_A_VIRTUAL_DISK);
        assert!(Win32Error::from_code(SUCCESS).is_success());
        assert!(!ERROR.is_success());
        assert!(Win32Error::from_code(SUCCESS).into_result().is_ok());
        assert!(ERROR.into_result().is_err());
    }
}

#[test]
fn only_zero_is_success() {
    for code in SAMPLES {
        assert_eq!(
            Win32Error::from_code(code).is_success(),
            code == SUCCESS,
            "code {code}"
        );
    }
}

#[test]
fn a_return_becomes_a_result() {
    assert_eq!(Win32Error::from_code(SUCCESS).into_result(), Ok(()));

    for code in SAMPLES.into_iter().filter(|code| *code != SUCCESS) {
        let error = Win32Error::from_code(code);
        assert_eq!(error.into_result(), Err(error), "code {code}");
    }
}

#[test]
fn equal_codes_are_equal_and_distinct_codes_are_not() {
    let distinct: HashSet<Win32Error> = SAMPLES.into_iter().map(Win32Error::from_code).collect();

    assert_eq!(distinct.len(), SAMPLES.len());
    for code in SAMPLES {
        assert_eq!(Win32Error::from_code(code), Win32Error::from_code(code));
    }
    assert_ne!(
        Win32Error::from_code(LARGEST_NON_NEGATIVE),
        Win32Error::from_code(FIRST_NEGATIVE)
    );
}

#[test]
fn the_error_is_a_std_error_with_no_source() {
    let error: Box<dyn std::error::Error> = Box::new(Win32Error::from_code(ACCESS_DENIED));

    assert!(error.source().is_none());
    assert!(error.to_string().contains('5'));
}

#[test]
fn the_display_names_the_code_in_decimal_and_in_hex() {
    // Decimal, because that is how `GetLastError` is read; hex, because `winerror.h` writes the
    // codes above 2^31 that way.
    let text = Win32Error::from_code(NOT_A_VIRTUAL_DISK).to_string();

    assert!(text.contains("3225026581"), "{text}");
    assert!(text.contains("0xC03A0015"), "{text}");

    let small = Win32Error::from_code(ACCESS_DENIED).to_string();
    assert!(small.contains("0x00000005"), "{small}");
}

#[cfg(windows)]
mod io_bridge {
    use std::io;

    use windows_sys::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_INSUFFICIENT_BUFFER,
        ERROR_INVALID_FUNCTION, ERROR_INVALID_PARAMETER, ERROR_IO_DEVICE, ERROR_IO_PENDING,
        ERROR_VIRTDISK_NOT_VIRTUAL_DISK, SetLastError,
    };

    use super::*;

    #[test]
    fn the_test_constants_are_the_codes_windows_defines() {
        // A typo in a literal above would make every other test here check the wrong number.
        assert_eq!(INVALID_FUNCTION, ERROR_INVALID_FUNCTION);
        assert_eq!(FILE_NOT_FOUND, ERROR_FILE_NOT_FOUND);
        assert_eq!(ACCESS_DENIED, ERROR_ACCESS_DENIED);
        assert_eq!(INVALID_PARAMETER, ERROR_INVALID_PARAMETER);
        assert_eq!(INSUFFICIENT_BUFFER, ERROR_INSUFFICIENT_BUFFER);
        assert_eq!(IO_PENDING, ERROR_IO_PENDING);
        assert_eq!(IO_DEVICE, ERROR_IO_DEVICE);
        assert_eq!(NOT_A_VIRTUAL_DISK, ERROR_VIRTDISK_NOT_VIRTUAL_DISK);
    }

    #[test]
    fn the_io_error_keeps_the_code_exactly_for_every_code() {
        // The defect this crate exists to remove: a code of 2^31 or more fell back to
        // `io::Error::other` in one crate and panicked in another, so `raw_os_error` was `None`
        // or never reached.
        for code in SAMPLES {
            let io_error = Win32Error::from_code(code).to_io_error();

            assert_eq!(
                io_error.raw_os_error(),
                Some(code.cast_signed()),
                "code {code:#010X}"
            );
        }
    }

    #[test]
    fn a_code_survives_the_round_trip_through_io_error() {
        for code in SAMPLES {
            let error = Win32Error::from_code(code);

            assert_eq!(
                Win32Error::from_io(&error.to_io_error()),
                Some(error),
                "code {code:#010X}"
            );
        }
    }

    #[test]
    fn a_code_above_two_to_the_thirty_first_is_a_negative_number_to_std() {
        assert_eq!(
            Win32Error::from_code(NOT_A_VIRTUAL_DISK)
                .to_io_error()
                .raw_os_error(),
            Some(-1_069_940_715)
        );
        assert_eq!(
            Win32Error::from_code(ALL_BITS).to_io_error().raw_os_error(),
            Some(-1)
        );
        assert_eq!(
            Win32Error::from_io(&io::Error::from_raw_os_error(-1)),
            Some(Win32Error::from_code(ALL_BITS))
        );
    }

    #[test]
    fn the_io_error_is_an_os_error_whose_kind_is_the_one_std_gives_the_code() {
        // Not `Other`: the conversion is a re-presentation, and std's own kind mapping applies.
        assert_eq!(
            Win32Error::from_code(FILE_NOT_FOUND).to_io_error().kind(),
            io::ErrorKind::NotFound
        );
        assert_eq!(
            Win32Error::from_code(ACCESS_DENIED).to_io_error().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(
            Win32Error::from_code(INVALID_PARAMETER)
                .to_io_error()
                .kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn an_io_error_with_no_os_code_is_none_and_never_success() {
        let without_a_code = [
            io::Error::other("a message"),
            io::Error::new(io::ErrorKind::NotFound, "a kind and a message"),
            io::Error::from(io::ErrorKind::PermissionDenied),
            io::Error::from(io::ErrorKind::UnexpectedEof),
        ];

        for error in &without_a_code {
            assert_eq!(Win32Error::from_io(error), None, "{error}");
        }
    }

    #[test]
    fn an_io_error_made_from_an_os_code_is_that_code_including_zero() {
        for code in SAMPLES {
            let io_error = io::Error::from_raw_os_error(code.cast_signed());

            assert_eq!(
                Win32Error::from_io(&io_error),
                Some(Win32Error::from_code(code)),
                "code {code:#010X}"
            );
        }
    }

    #[test]
    fn the_from_impl_is_the_same_conversion() {
        for code in SAMPLES {
            let by_method = Win32Error::from_code(code).to_io_error();
            let by_from = io::Error::from(Win32Error::from_code(code));

            assert_eq!(by_from.raw_os_error(), by_method.raw_os_error());
            assert_eq!(by_from.kind(), by_method.kind());
        }
    }

    #[test]
    fn the_display_of_a_code_with_a_message_includes_it() {
        let text = Win32Error::from_code(FILE_NOT_FOUND).to_string();

        assert!(text.starts_with("Win32 error 2 (0x00000002): "), "{text}");
        assert!(text.len() > "Win32 error 2 (0x00000002): ".len(), "{text}");
    }

    #[test]
    fn the_last_error_is_the_one_the_thread_holds() {
        for code in SAMPLES {
            // SAFETY: `SetLastError` writes only the calling thread's own error slot.
            unsafe { SetLastError(code) };

            assert_eq!(
                Win32Error::last(),
                Win32Error::from_code(code),
                "code {code:#010X}"
            );
        }
    }

    #[test]
    fn the_last_error_is_read_when_asked_and_not_remembered() {
        // SAFETY: as above.
        unsafe { SetLastError(ACCESS_DENIED) };
        assert_eq!(Win32Error::last().code(), ACCESS_DENIED);

        // SAFETY: as above.
        unsafe { SetLastError(FILE_NOT_FOUND) };
        assert_eq!(Win32Error::last().code(), FILE_NOT_FOUND);
    }
}
