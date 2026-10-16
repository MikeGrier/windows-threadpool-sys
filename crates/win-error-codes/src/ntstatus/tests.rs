// Copyright (c) Mike Grier.

//! Tests for `NtStatus`, and for the two `HResult` conversions that go with it. The statuses are this
//! file's own.

use std::collections::HashSet;

use super::{NtSeverity, NtStatus};
use crate::HResult;

/// `STATUS_SUCCESS`.
const SUCCESS: u32 = 0;
/// `STATUS_PENDING`: a success that is not zero.
const PENDING: u32 = 0x0000_0103;
/// The largest value, which is informational: its top two bits are 01.
const LARGEST: u32 = 0x7FFF_FFFF;
/// `STATUS_OBJECT_NAME_EXISTS`: the first informational status.
const OBJECT_NAME_EXISTS: u32 = 0x4000_0000;
/// `STATUS_GUARD_PAGE_VIOLATION`: the first warning.
const GUARD_PAGE_VIOLATION: u32 = 0x8000_0001;
/// `STATUS_BUFFER_OVERFLOW`: a warning that carries data.
const BUFFER_OVERFLOW: u32 = 0x8000_0005;
/// `STATUS_NO_MORE_FILES`: a warning.
const NO_MORE_FILES: u32 = 0x8000_0006;
/// `STATUS_INFO_LENGTH_MISMATCH`: an error.
const INFO_LENGTH_MISMATCH: u32 = 0xC000_0004;
/// `STATUS_ACCESS_VIOLATION`: an error.
const ACCESS_VIOLATION: u32 = 0xC000_0005;
/// `STATUS_ACCESS_DENIED`: an error.
const ACCESS_DENIED: u32 = 0xC000_0022;
/// `STATUS_VIRTDISK_NOT_VIRTUAL_DISK`: an error in facility 0x3A, and the same number as
/// `ERROR_VIRTDISK_NOT_VIRTUAL_DISK`.
const NOT_A_VIRTUAL_DISK: u32 = 0xC03A_0015;
/// An error with the customer bit set.
const CUSTOMER_ERROR: u32 = 0xE000_0001;
/// An error with the largest facility, and a code of zero.
const LARGEST_FACILITY: u32 = 0xCFFF_0000;
/// Every bit set: an error, the customer bit, and the bit `HRESULT_FROM_NT` sets.
const ALL_BITS: u32 = u32::MAX;

/// What `HRESULT_FROM_NT` sets.
const FROM_NT_BIT: u32 = 0x1000_0000;

const SUCCESSES: [u32; 4] = [SUCCESS, PENDING, LARGEST, OBJECT_NAME_EXISTS];
const WARNINGS: [u32; 3] = [GUARD_PAGE_VIOLATION, BUFFER_OVERFLOW, NO_MORE_FILES];
const ERRORS: [u32; 6] = [
    INFO_LENGTH_MISMATCH,
    ACCESS_VIOLATION,
    ACCESS_DENIED,
    NOT_A_VIRTUAL_DISK,
    CUSTOMER_ERROR,
    LARGEST_FACILITY,
];

fn status(bits: u32) -> NtStatus {
    NtStatus::from_raw(bits.cast_signed())
}

fn every() -> Vec<u32> {
    SUCCESSES
        .into_iter()
        .chain(WARNINGS)
        .chain(ERRORS)
        .chain([ALL_BITS])
        .collect()
}

#[test]
fn a_raw_value_is_carried_unaltered() {
    for bits in every() {
        assert_eq!(status(bits).raw(), bits.cast_signed(), "{bits:#010X}");
    }
    for raw in [i32::MIN, -1, 0, 1, i32::MAX] {
        assert_eq!(NtStatus::from_raw(raw).raw(), raw);
    }
}

#[test]
fn the_constructor_and_accessors_work_in_a_constant() {
    const ERROR: NtStatus = NtStatus::from_raw(0xC000_0005_u32.cast_signed());
    const PENDING_STATUS: NtStatus = NtStatus::from_raw(0x103);
    const WARNING: NtStatus = NtStatus::from_raw(0x8000_0005_u32.cast_signed());
    const CONVERTED: HResult = HResult::from_ntstatus(ERROR);

    const {
        assert!(ERROR.raw() == 0xC000_0005_u32.cast_signed());
        assert!(matches!(ERROR.severity(), NtSeverity::Error));
        assert!(matches!(PENDING_STATUS.severity(), NtSeverity::Success));
        assert!(matches!(WARNING.severity(), NtSeverity::Warning));
        assert!(!ERROR.is_success());
        assert!(PENDING_STATUS.is_success());
        assert!(!WARNING.is_success());
        assert!(ERROR.is_error());
        assert!(WARNING.is_warning());
        assert!(PENDING_STATUS.into_result().is_ok());
        assert!(ERROR.into_result().is_err());
        assert!(ERROR.code() == 5);
        assert!(CONVERTED.raw() == 0xD000_0005_u32.cast_signed());
        assert!(CONVERTED.ntstatus().is_some());
    }
}

#[test]
fn the_severity_is_the_top_two_bits() {
    for bits in SUCCESSES {
        let expected = if bits >> 30 == 1 {
            NtSeverity::Informational
        } else {
            NtSeverity::Success
        };
        assert_eq!(status(bits).severity(), expected, "{bits:#010X}");
    }
    for bits in WARNINGS {
        assert_eq!(status(bits).severity(), NtSeverity::Warning, "{bits:#010X}");
    }
    for bits in ERRORS.into_iter().chain([ALL_BITS]) {
        assert_eq!(status(bits).severity(), NtSeverity::Error, "{bits:#010X}");
    }
}

#[test]
fn each_severity_is_the_one_value_of_the_top_two_bits() {
    assert_eq!(status(0x0000_0000).severity(), NtSeverity::Success);
    assert_eq!(status(0x3FFF_FFFF).severity(), NtSeverity::Success);
    assert_eq!(status(0x4000_0000).severity(), NtSeverity::Informational);
    assert_eq!(status(0x7FFF_FFFF).severity(), NtSeverity::Informational);
    assert_eq!(status(0x8000_0000).severity(), NtSeverity::Warning);
    assert_eq!(status(0xBFFF_FFFF).severity(), NtSeverity::Warning);
    assert_eq!(status(0xC000_0000).severity(), NtSeverity::Error);
    assert_eq!(status(0xFFFF_FFFF).severity(), NtSeverity::Error);
}

#[test]
fn the_severities_are_the_values_ntstatus_h_gives_them() {
    assert_eq!(NtSeverity::Success as u8, 0);
    assert_eq!(NtSeverity::Informational as u8, 1);
    assert_eq!(NtSeverity::Warning as u8, 2);
    assert_eq!(NtSeverity::Error as u8, 3);
}

#[test]
fn nt_success_is_not_negative_so_an_informational_status_is_a_success_and_a_warning_is_not() {
    for bits in SUCCESSES {
        assert!(status(bits).is_success(), "{bits:#010X}");
    }
    for bits in WARNINGS.into_iter().chain(ERRORS).chain([ALL_BITS]) {
        assert!(!status(bits).is_success(), "{bits:#010X}");
    }
}

#[test]
fn the_three_predicates_each_name_one_severity() {
    for bits in every() {
        let value = status(bits);
        let flags = [value.is_information(), value.is_warning(), value.is_error()];

        assert_eq!(
            flags,
            [
                value.severity() == NtSeverity::Informational,
                value.severity() == NtSeverity::Warning,
                value.severity() == NtSeverity::Error,
            ],
            "{bits:#010X}"
        );
    }
    assert!(status(OBJECT_NAME_EXISTS).is_information());
    assert!(!status(PENDING).is_information());
    assert!(status(NO_MORE_FILES).is_warning());
    assert!(status(ACCESS_DENIED).is_error());
}

#[test]
fn a_success_and_an_informational_status_become_ok_and_a_warning_or_error_the_status() {
    for bits in SUCCESSES {
        assert_eq!(status(bits).into_result(), Ok(()), "{bits:#010X}");
    }
    for bits in WARNINGS.into_iter().chain(ERRORS) {
        assert_eq!(
            status(bits).into_result(),
            Err(status(bits)),
            "{bits:#010X}"
        );
    }
}

#[test]
fn the_code_is_the_low_sixteen_bits() {
    assert_eq!(status(ACCESS_VIOLATION).code(), 5);
    assert_eq!(status(NOT_A_VIRTUAL_DISK).code(), 0x15);
    assert_eq!(status(PENDING).code(), 0x103);
    assert_eq!(status(LARGEST_FACILITY).code(), 0);
    assert_eq!(status(ALL_BITS).code(), 0xFFFF);
}

#[test]
fn the_facility_is_the_twelve_bits_above_the_code() {
    assert_eq!(status(ACCESS_VIOLATION).facility(), 0);
    assert_eq!(status(NOT_A_VIRTUAL_DISK).facility(), 0x3A);
    assert_eq!(status(LARGEST_FACILITY).facility(), 0xFFF);
    // The customer bit and the bit HRESULT_FROM_NT sets are above the twelve.
    assert_eq!(status(CUSTOMER_ERROR).facility(), 0);
    assert_eq!(status(0xD000_0005).facility(), 0);
    assert_eq!(status(ALL_BITS).facility(), 0xFFF);
}

#[test]
fn the_customer_bit_is_read_from_its_own_place() {
    // The largest value has every bit but the sign set, the customer bit among them.
    let customers = [CUSTOMER_ERROR, ALL_BITS, LARGEST];
    for bits in customers {
        assert!(status(bits).is_customer(), "{bits:#010X}");
    }

    for bits in every().into_iter().filter(|bits| !customers.contains(bits)) {
        assert!(!status(bits).is_customer(), "{bits:#010X}");
    }
    assert!(!status(0xD000_0005).is_customer());
}

#[test]
fn the_display_is_the_value_in_hex_with_eight_digits() {
    assert_eq!(status(ACCESS_VIOLATION).to_string(), "0xC0000005");
    assert_eq!(status(SUCCESS).to_string(), "0x00000000");
    assert_eq!(status(PENDING).to_string(), "0x00000103");
    assert_eq!(status(ALL_BITS).to_string(), "0xFFFFFFFF");
}

#[test]
fn the_debug_is_in_hex_and_not_the_negative_number() {
    assert_eq!(
        format!("{:?}", status(ACCESS_VIOLATION)),
        "NtStatus(0xC0000005)"
    );
    assert_eq!(format!("{:?}", status(SUCCESS)), "NtStatus(0x00000000)");
}

#[test]
fn equal_values_are_equal_and_distinct_values_are_not() {
    let all = every();
    let distinct: HashSet<NtStatus> = all.iter().map(|bits| status(*bits)).collect();

    assert_eq!(distinct.len(), all.len());
    assert_ne!(status(LARGEST), status(GUARD_PAGE_VIOLATION));
}

// --- HRESULT_FROM_NT -----------------------------------------------------------------------

#[test]
fn hresult_from_nt_sets_one_bit() {
    for bits in every() {
        assert_eq!(
            HResult::from_ntstatus(status(bits)).raw().cast_unsigned(),
            bits | FROM_NT_BIT,
            "{bits:#010X}"
        );
    }
}

#[test]
fn a_known_conversion_is_what_windows_gives() {
    // STATUS_ACCESS_VIOLATION as HRESULT_FROM_NT makes it.
    assert_eq!(
        HResult::from_ntstatus(status(ACCESS_VIOLATION))
            .raw()
            .cast_unsigned(),
        0xD000_0005
    );
    assert_eq!(
        HResult::from_ntstatus(status(SUCCESS))
            .raw()
            .cast_unsigned(),
        0x1000_0000
    );
}

#[test]
fn a_converted_status_is_marked_as_one() {
    for bits in every() {
        assert!(
            HResult::from_ntstatus(status(bits)).is_ntstatus_mapped(),
            "{bits:#010X}"
        );
    }
}

#[test]
fn a_status_without_the_bit_set_round_trips_through_an_hresult() {
    for bits in every().into_iter().filter(|bits| bits & FROM_NT_BIT == 0) {
        assert_eq!(
            HResult::from_ntstatus(status(bits)).ntstatus(),
            Some(status(bits)),
            "{bits:#010X}"
        );
    }
}

#[test]
fn a_status_that_already_has_the_bit_set_does_not_come_back_as_it_went_in() {
    // HRESULT_FROM_NT ors the bit in and ntstatus clears it, so the one bit is not recoverable.
    let original = status(ALL_BITS);
    let back = HResult::from_ntstatus(original)
        .ntstatus()
        .expect("the bit is set");

    assert_eq!(back, status(ALL_BITS & !FROM_NT_BIT));
    assert_ne!(back, original);
}

#[test]
fn an_hresult_without_the_bit_is_not_a_status() {
    for bits in [
        0x0000_0000_u32,
        0x0000_0001,
        0x8000_4005,
        0x8007_045D,
        0x887A_0001,
        0xA000_0001,
        0xC000_0005,
    ] {
        assert_eq!(
            HResult::from_raw(bits.cast_signed()).ntstatus(),
            None,
            "{bits:#010X}"
        );
    }
}

#[cfg(windows)]
mod against_windows {
    use windows_sys::Win32::Foundation as windows;

    use super::*;

    #[test]
    fn the_test_constants_are_the_values_windows_defines() {
        // A typo in a literal above would make every other test here check the wrong number.
        assert_eq!(SUCCESS.cast_signed(), windows::STATUS_SUCCESS);
        assert_eq!(PENDING.cast_signed(), windows::STATUS_PENDING);
        assert_eq!(
            OBJECT_NAME_EXISTS.cast_signed(),
            windows::STATUS_OBJECT_NAME_EXISTS
        );
        assert_eq!(
            GUARD_PAGE_VIOLATION.cast_signed(),
            windows::STATUS_GUARD_PAGE_VIOLATION
        );
        assert_eq!(
            BUFFER_OVERFLOW.cast_signed(),
            windows::STATUS_BUFFER_OVERFLOW
        );
        assert_eq!(NO_MORE_FILES.cast_signed(), windows::STATUS_NO_MORE_FILES);
        assert_eq!(
            INFO_LENGTH_MISMATCH.cast_signed(),
            windows::STATUS_INFO_LENGTH_MISMATCH
        );
        assert_eq!(
            ACCESS_VIOLATION.cast_signed(),
            windows::STATUS_ACCESS_VIOLATION
        );
        assert_eq!(ACCESS_DENIED.cast_signed(), windows::STATUS_ACCESS_DENIED);
        assert_eq!(
            NOT_A_VIRTUAL_DISK.cast_signed(),
            windows::STATUS_VIRTDISK_NOT_VIRTUAL_DISK
        );
    }

    #[test]
    fn a_status_and_the_win32_error_of_the_same_number_are_the_same_value_in_different_spaces() {
        // STATUS_VIRTDISK_NOT_VIRTUAL_DISK and ERROR_VIRTDISK_NOT_VIRTUAL_DISK are one number. That
        // is not a conversion, and this crate offers none between the two; the test only shows the
        // case that prompted the type.
        assert_eq!(
            windows::STATUS_VIRTDISK_NOT_VIRTUAL_DISK.cast_unsigned(),
            windows::ERROR_VIRTDISK_NOT_VIRTUAL_DISK
        );
    }
}
