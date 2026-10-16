// Copyright (c) Mike Grier.

//! Tests for `HResult`. The Win32 codes are the crate's shared set, in `crate::test_codes`; the
//! `HRESULT`s are this file's own.

use super::HResult;
use crate::Win32Error;
use crate::test_codes::*;

/// `S_OK`.
const S_OK: u32 = 0;
/// `S_FALSE`, a success that is not zero.
const S_FALSE: u32 = 1;
/// `E_NOTIMPL`: a failure in `FACILITY_NULL`.
const E_NOTIMPL: u32 = 0x8000_4001;
/// `E_FAIL`: a failure in `FACILITY_NULL`.
const E_FAIL: u32 = 0x8000_4005;
/// `E_ACCESSDENIED`: `ERROR_ACCESS_DENIED` wrapped.
const E_ACCESSDENIED: u32 = 0x8007_0005;
/// `E_INVALIDARG`: `ERROR_INVALID_PARAMETER` wrapped.
const E_INVALIDARG: u32 = 0x8007_0057;
/// `E_OUTOFMEMORY`: `ERROR_OUTOFMEMORY` wrapped.
const E_OUTOFMEMORY: u32 = 0x8007_000E;
/// `ERROR_IO_DEVICE` wrapped.
const IO_DEVICE_WRAPPED: u32 = 0x8007_045D;
/// `DXGI_ERROR_INVALID_CALL`: `FACILITY_DXGI`, 0x87A, which needs 12 bits.
const DXGI_INVALID_CALL: u32 = 0x887A_0001;
/// `STATUS_ACCESS_VIOLATION` mapped by `HRESULT_FROM_NT`: the `NTSTATUS` bit is set.
const NTSTATUS_MAPPED: u32 = 0xD000_0005;
/// A failure with the customer bit set.
const CUSTOMER: u32 = 0xA000_0001;
/// The largest value, a success.
const LARGEST: u32 = 0x7FFF_FFFF;
/// The smallest value, a failure with everything else clear.
const SMALLEST: u32 = 0x8000_0000;

const FAILURES: [u32; 10] = [
    E_NOTIMPL,
    E_FAIL,
    E_ACCESSDENIED,
    E_INVALIDARG,
    E_OUTOFMEMORY,
    IO_DEVICE_WRAPPED,
    DXGI_INVALID_CALL,
    NTSTATUS_MAPPED,
    CUSTOMER,
    SMALLEST,
];

const SUCCESSES: [u32; 4] = [S_OK, S_FALSE, 0x0007_0005, LARGEST];

fn hresult(bits: u32) -> HResult {
    HResult::from_raw(bits.cast_signed())
}

#[test]
fn a_raw_value_is_carried_unaltered() {
    for bits in FAILURES.into_iter().chain(SUCCESSES) {
        assert_eq!(hresult(bits).raw(), bits.cast_signed(), "{bits:#010X}");
    }
    for raw in [i32::MIN, -1, 0, 1, i32::MAX] {
        assert_eq!(HResult::from_raw(raw).raw(), raw);
    }
}

#[test]
fn the_constructors_and_accessors_work_in_a_constant() {
    const E_INVALID: HResult = HResult::from_raw(0x8007_0057_u32.cast_signed());
    const WRAPPED: HResult = HResult::from_win32(Win32Error::from_code(1117));
    const E_FAIL_VALUE: HResult = HResult::from_raw(E_FAIL.cast_signed());

    const {
        assert!(E_INVALID.raw() == 0x8007_0057_u32.cast_signed());
        assert!(E_INVALID.is_failure());
        assert!(!E_INVALID.is_success());
        assert!(HResult::from_raw(0).is_success());
        assert!(!HResult::from_raw(0).is_failure());
        assert!(HResult::from_raw(0).into_result().is_ok());
        assert!(E_INVALID.into_result().is_err());
        assert!(E_INVALID.code() == 0x57);
        assert!(E_INVALID.facility() == 7);
        assert!(WRAPPED.raw() == 0x8007_045D_u32.cast_signed());
        assert!(E_INVALID.win32().is_some());
        assert!(E_FAIL_VALUE.win32().is_none());
    }
}

#[test]
fn a_failure_is_the_severity_bit_and_a_success_is_everything_else() {
    for bits in FAILURES {
        assert!(hresult(bits).is_failure(), "{bits:#010X}");
        assert!(!hresult(bits).is_success(), "{bits:#010X}");
    }
    for bits in SUCCESSES {
        assert!(hresult(bits).is_success(), "{bits:#010X}");
        assert!(!hresult(bits).is_failure(), "{bits:#010X}");
    }
}

#[test]
fn s_false_is_a_success_and_the_result_cannot_tell_it_from_s_ok() {
    assert_eq!(hresult(S_OK).into_result(), Ok(()));
    assert_eq!(hresult(S_FALSE).into_result(), Ok(()));
    assert_ne!(hresult(S_OK), hresult(S_FALSE));
}

#[test]
fn a_failure_becomes_an_error_carrying_the_value() {
    for bits in FAILURES {
        assert_eq!(
            hresult(bits).into_result(),
            Err(hresult(bits)),
            "{bits:#010X}"
        );
    }
}

#[test]
fn the_code_is_the_low_sixteen_bits() {
    assert_eq!(hresult(E_FAIL).code(), 0x4005);
    assert_eq!(hresult(E_INVALIDARG).code(), 0x0057);
    assert_eq!(hresult(IO_DEVICE_WRAPPED).code(), 0x045D);
    assert_eq!(hresult(DXGI_INVALID_CALL).code(), 1);
    assert_eq!(hresult(SMALLEST).code(), 0);
    assert_eq!(hresult(u32::MAX).code(), 0xFFFF);
}

#[test]
fn the_facility_is_the_thirteen_bits_above_the_code_as_the_macro_takes_them() {
    assert_eq!(hresult(E_FAIL).facility(), 0);
    assert_eq!(hresult(E_INVALIDARG).facility(), 7);
    assert_eq!(hresult(IO_DEVICE_WRAPPED).facility(), 7);
    // FACILITY_DXGI is 0x87A, which does not fit in the eleven bits the documentation draws.
    assert_eq!(hresult(DXGI_INVALID_CALL).facility(), 0x87A);
    // The NTSTATUS bit is in the thirteen, and the customer bit is above them.
    assert_eq!(hresult(NTSTATUS_MAPPED).facility(), 0x1000);
    assert_eq!(hresult(CUSTOMER).facility(), 0);
    assert_eq!(hresult(u32::MAX).facility(), 0x1FFF);
}

#[test]
fn the_customer_and_ntstatus_bits_are_read_from_their_own_places() {
    assert!(hresult(CUSTOMER).is_customer());
    assert!(!hresult(CUSTOMER).is_ntstatus_mapped());

    assert!(hresult(NTSTATUS_MAPPED).is_ntstatus_mapped());
    assert!(!hresult(NTSTATUS_MAPPED).is_customer());

    for bits in [E_FAIL, E_INVALIDARG, IO_DEVICE_WRAPPED, S_OK, SMALLEST] {
        assert!(!hresult(bits).is_customer(), "{bits:#010X}");
        assert!(!hresult(bits).is_ntstatus_mapped(), "{bits:#010X}");
    }
    assert!(hresult(u32::MAX).is_customer());
    assert!(hresult(u32::MAX).is_ntstatus_mapped());
}

#[test]
fn a_win32_code_is_wrapped_as_hresult_from_win32_wraps_it() {
    assert_eq!(
        HResult::from_win32(Win32Error::from_code(IO_DEVICE)),
        hresult(IO_DEVICE_WRAPPED)
    );
    assert_eq!(
        HResult::from_win32(Win32Error::from_code(ACCESS_DENIED)),
        hresult(E_ACCESSDENIED)
    );
    assert_eq!(
        HResult::from_win32(Win32Error::from_code(INVALID_PARAMETER)),
        hresult(E_INVALIDARG)
    );
    assert_eq!(
        HResult::from_win32(Win32Error::from_code(0xE)),
        hresult(E_OUTOFMEMORY)
    );
}

#[test]
fn zero_is_s_ok_and_a_code_of_two_to_the_thirty_first_or_more_is_taken_as_an_hresult_already() {
    assert_eq!(
        HResult::from_win32(Win32Error::from_code(SUCCESS)),
        hresult(S_OK)
    );
    for code in [FIRST_NEGATIVE, NOT_A_VIRTUAL_DISK, ALL_BITS] {
        assert_eq!(
            HResult::from_win32(Win32Error::from_code(code)).raw(),
            code.cast_signed(),
            "code {code:#010X}"
        );
    }
}

#[test]
fn a_positive_code_keeps_only_its_low_sixteen_bits() {
    // The macro's own rule, and it loses the rest: these all give the same value.
    for code in [
        IO_DEVICE,
        0x0001_0000 | IO_DEVICE,
        0x0010_0000 | IO_DEVICE,
        0x7FFF_0000 | IO_DEVICE,
    ] {
        assert_eq!(
            HResult::from_win32(Win32Error::from_code(code)),
            hresult(IO_DEVICE_WRAPPED),
            "code {code:#010X}"
        );
    }
    assert_eq!(
        HResult::from_win32(Win32Error::from_code(FIRST_ABOVE_16_BIT)),
        hresult(0x8007_0000)
    );
    assert_eq!(
        HResult::from_win32(Win32Error::from_code(LARGEST_NON_NEGATIVE)),
        hresult(0x8007_FFFF)
    );
}

#[test]
fn the_codes_from_one_to_sixty_five_thousand_round_trip() {
    for code in SAMPLES
        .into_iter()
        .filter(|code| (1..=LARGEST_16_BIT).contains(code))
    {
        let error = Win32Error::from_code(code);

        assert_eq!(
            HResult::from_win32(error).win32(),
            Some(error),
            "code {code:#010X}"
        );
    }
}

#[test]
fn an_hresult_that_wraps_a_win32_code_gives_it_back() {
    assert_eq!(
        hresult(IO_DEVICE_WRAPPED).win32(),
        Some(Win32Error::from_code(IO_DEVICE))
    );
    assert_eq!(
        hresult(E_ACCESSDENIED).win32(),
        Some(Win32Error::from_code(ACCESS_DENIED))
    );
    assert_eq!(
        hresult(E_INVALIDARG).win32(),
        Some(Win32Error::from_code(INVALID_PARAMETER))
    );
    assert_eq!(
        hresult(0x8007_FFFF).win32(),
        Some(Win32Error::from_code(LARGEST_16_BIT))
    );
}

#[test]
fn s_ok_wraps_no_error_so_zero_does_not_round_trip() {
    assert_eq!(hresult(S_OK).win32(), None);
    assert_eq!(
        HResult::from_win32(Win32Error::from_code(SUCCESS)).win32(),
        None
    );
}

#[test]
fn only_the_exact_wrapping_gives_a_win32_code() {
    // Each differs from 0x80070005 in one bit above the code, or in the severity.
    for bits in [
        S_FALSE,
        E_FAIL,
        E_NOTIMPL,
        DXGI_INVALID_CALL,
        NTSTATUS_MAPPED,
        CUSTOMER,
        SMALLEST,
        0x0007_0005,
        0x8006_0005,
        0x8008_0005,
        0xA007_0005,
        0x9007_0005,
        0xC007_0005,
        0x8807_0005,
        u32::MAX,
    ] {
        assert_eq!(hresult(bits).win32(), None, "{bits:#010X}");
    }
}

#[test]
fn the_display_is_the_value_in_hex_with_eight_digits() {
    assert_eq!(hresult(IO_DEVICE_WRAPPED).to_string(), "0x8007045D");
    assert_eq!(hresult(S_OK).to_string(), "0x00000000");
    assert_eq!(hresult(S_FALSE).to_string(), "0x00000001");
    assert_eq!(hresult(DXGI_INVALID_CALL).to_string(), "0x887A0001");
    assert_eq!(hresult(u32::MAX).to_string(), "0xFFFFFFFF");
}

#[test]
fn the_debug_is_in_hex_and_not_the_negative_number() {
    assert_eq!(
        format!("{:?}", hresult(IO_DEVICE_WRAPPED)),
        "HResult(0x8007045D)"
    );
    assert_eq!(format!("{:?}", hresult(S_OK)), "HResult(0x00000000)");
}

#[test]
fn equal_values_are_equal_and_distinct_values_are_not() {
    let all: Vec<u32> = FAILURES.into_iter().chain(SUCCESSES).collect();
    let distinct: std::collections::HashSet<HResult> =
        all.iter().map(|bits| hresult(*bits)).collect();

    assert_eq!(distinct.len(), all.len());
    assert_ne!(hresult(LARGEST), hresult(SMALLEST));
}

#[cfg(windows)]
mod against_windows {
    use windows_sys::Win32::Foundation as windows;

    use super::*;

    #[test]
    fn the_test_constants_are_the_values_windows_defines() {
        // A typo in a literal above would make every other test here check the wrong number.
        assert_eq!(S_OK.cast_signed(), windows::S_OK);
        assert_eq!(S_FALSE.cast_signed(), windows::S_FALSE);
        assert_eq!(E_NOTIMPL.cast_signed(), windows::E_NOTIMPL);
        assert_eq!(E_FAIL.cast_signed(), windows::E_FAIL);
        assert_eq!(E_ACCESSDENIED.cast_signed(), windows::E_ACCESSDENIED);
        assert_eq!(E_INVALIDARG.cast_signed(), windows::E_INVALIDARG);
        assert_eq!(E_OUTOFMEMORY.cast_signed(), windows::E_OUTOFMEMORY);
    }

    #[test]
    fn the_wrapped_constants_are_what_from_win32_makes_of_the_win32_ones() {
        assert_eq!(
            HResult::from_win32(Win32Error::from_code(windows::ERROR_ACCESS_DENIED)).raw(),
            windows::E_ACCESSDENIED
        );
        assert_eq!(
            HResult::from_win32(Win32Error::from_code(windows::ERROR_INVALID_PARAMETER)).raw(),
            windows::E_INVALIDARG
        );
        assert_eq!(
            HResult::from_win32(Win32Error::from_code(windows::ERROR_OUTOFMEMORY)).raw(),
            windows::E_OUTOFMEMORY
        );
    }
}
