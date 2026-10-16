// Copyright (c) 2026 Mike Grier
//! `ErrorCode`: a number it carries and prints, and no more. What a Windows code means in it is
//! `win-durable-ioring`'s (DI-9.7.11).

use super::ErrorCode;

#[test]
fn a_code_reads_back_as_the_hresult_it_was_made_from() {
    for hresult in [
        0,
        1,
        0x8007_045D_u32.cast_signed(),
        0x8007_0005_u32.cast_signed(),
        0x8046_0005_u32.cast_signed(),
        0xC03A_0015_u32.cast_signed(),
        i32::MIN,
        i32::MAX,
        -1,
    ] {
        assert_eq!(ErrorCode::from_hresult(hresult).hresult(), hresult);
    }
}

#[test]
fn codes_are_equal_exactly_when_their_numbers_are() {
    assert_eq!(ErrorCode::from_hresult(1117), ErrorCode::from_hresult(1117));
    assert_ne!(ErrorCode::from_hresult(1117), ErrorCode::from_hresult(1118));
    assert_ne!(ErrorCode::from_hresult(1), ErrorCode::from_hresult(-1));
}

#[test]
fn a_code_prints_as_its_hresult_in_hex() {
    let code = ErrorCode::from_hresult(0x8007_045D_u32.cast_signed());
    assert_eq!(code.to_string(), "0x8007045D");
    assert_eq!(format!("{code:?}"), "ErrorCode(0x8007045D)");
}

#[test]
fn a_small_code_prints_with_its_leading_zeros() {
    assert_eq!(ErrorCode::from_hresult(1117).to_string(), "0x0000045D");
    assert_eq!(ErrorCode::from_hresult(0).to_string(), "0x00000000");
}

#[test]
fn a_negative_code_prints_as_the_unsigned_value_it_is() {
    assert_eq!(ErrorCode::from_hresult(-1).to_string(), "0xFFFFFFFF");
    assert_eq!(ErrorCode::from_hresult(i32::MIN).to_string(), "0x80000000");
}
