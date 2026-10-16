// Copyright (c) Mike Grier.

//! Tests for `LStatus`. The codes are the crate's shared set, in `crate::test_codes`, which are
//! `u32` and so are cast here to the signed carrier.

use std::collections::HashSet;

use super::LStatus;
use crate::Win32Error;
use crate::test_codes::*;

/// What a `u32` sample is as a `LONG`.
fn as_long(code: u32) -> i32 {
    code.cast_signed()
}

#[test]
fn a_raw_value_is_carried_unaltered() {
    for code in SAMPLES {
        let raw = as_long(code);
        assert_eq!(LStatus::from_raw(raw).raw(), raw, "code {code:#010X}");
    }
}

#[test]
fn the_extremes_of_a_long_are_carried() {
    for raw in [i32::MIN, i32::MIN + 1, -1, 0, 1, i32::MAX - 1, i32::MAX] {
        assert_eq!(LStatus::from_raw(raw).raw(), raw);
    }
}

#[test]
fn the_constructor_and_the_conversions_work_in_a_constant() {
    const STATUS: LStatus = LStatus::from_raw(-1_069_940_715);

    const {
        assert!(STATUS.raw() == -1_069_940_715);
        assert!(LStatus::from_raw(0).is_success());
        assert!(!STATUS.is_success());
        assert!(LStatus::from_raw(0).into_result().is_ok());
        assert!(STATUS.into_result().is_err());
        assert!(STATUS.to_win32().code() == NOT_A_VIRTUAL_DISK);
        assert!(STATUS.to_win32().to_lstatus().raw() == STATUS.raw());
    }
}

#[test]
fn only_zero_is_success() {
    for code in SAMPLES {
        assert_eq!(
            LStatus::from_raw(as_long(code)).is_success(),
            code == SUCCESS,
            "code {code:#010X}"
        );
    }
}

#[test]
fn a_return_becomes_a_result_carrying_the_code() {
    assert_eq!(LStatus::from_raw(0).into_result(), Ok(()));

    for code in SAMPLES.into_iter().filter(|code| *code != SUCCESS) {
        assert_eq!(
            LStatus::from_raw(as_long(code)).into_result(),
            Err(Win32Error::from_code(code)),
            "code {code:#010X}"
        );
    }
}

#[test]
fn a_negative_value_is_a_code_above_two_to_the_thirty_first_and_not_its_magnitude() {
    assert_eq!(
        LStatus::from_raw(-1).to_win32(),
        Win32Error::from_code(ALL_BITS)
    );
    assert_eq!(
        LStatus::from_raw(i32::MIN).to_win32(),
        Win32Error::from_code(FIRST_NEGATIVE)
    );
    assert_eq!(
        LStatus::from_raw(-1_069_940_715).to_win32(),
        Win32Error::from_code(NOT_A_VIRTUAL_DISK)
    );
}

#[test]
fn every_code_converts_to_the_signed_carrier_and_back_without_loss() {
    for code in SAMPLES {
        let error = Win32Error::from_code(code);

        assert_eq!(error.to_lstatus().raw(), as_long(code), "code {code:#010X}");
        assert_eq!(error.to_lstatus().to_win32(), error, "code {code:#010X}");
    }
}

#[test]
fn every_value_converts_to_a_code_and_back_without_loss() {
    for code in SAMPLES {
        let status = LStatus::from_raw(as_long(code));

        assert_eq!(status.to_win32().code(), code, "code {code:#010X}");
        assert_eq!(status.to_win32().to_lstatus(), status, "code {code:#010X}");
    }
}

#[test]
fn the_from_impls_are_the_same_conversions() {
    for code in SAMPLES {
        let status = LStatus::from_raw(as_long(code));
        let error = Win32Error::from_code(code);

        assert_eq!(Win32Error::from(status), status.to_win32());
        assert_eq!(LStatus::from(error), error.to_lstatus());
        assert_eq!(Win32Error::from(status), error, "code {code:#010X}");
        assert_eq!(LStatus::from(error), status, "code {code:#010X}");
    }
}

#[test]
fn equal_values_are_equal_and_distinct_values_are_not() {
    let distinct: HashSet<LStatus> = SAMPLES
        .into_iter()
        .map(|code| LStatus::from_raw(as_long(code)))
        .collect();

    assert_eq!(distinct.len(), SAMPLES.len());
    assert_ne!(LStatus::from_raw(i32::MAX), LStatus::from_raw(i32::MIN));
    assert_ne!(LStatus::from_raw(1), LStatus::from_raw(-1));
}

#[test]
fn the_display_is_the_win32_errors_for_the_same_code() {
    for code in SAMPLES {
        assert_eq!(
            LStatus::from_raw(as_long(code)).to_string(),
            Win32Error::from_code(code).to_string(),
            "code {code:#010X}"
        );
    }
}

#[test]
fn the_display_shows_a_negative_value_as_the_code_it_is() {
    let text = LStatus::from_raw(-1_069_940_715).to_string();

    assert!(text.contains("3225026581"), "{text}");
    assert!(text.contains("0xC03A0015"), "{text}");
}
