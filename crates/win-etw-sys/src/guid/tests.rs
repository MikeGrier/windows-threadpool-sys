// Copyright (c) 2026 Mike Grier
//! A GUID's value, text and layout.

use super::Guid;

const VALUE: u128 = 0x1234_5678_9abc_def0_1122_3344_5566_7788;

#[test]
fn a_value_reads_back_as_it_was_given() {
    assert_eq!(Guid::from_u128(VALUE).as_u128(), VALUE);
}

#[test]
fn the_zero_and_the_all_ones_values_read_back() {
    assert_eq!(Guid::from_u128(0).as_u128(), 0);
    assert_eq!(Guid::from_u128(u128::MAX).as_u128(), u128::MAX);
}

#[test]
fn each_group_of_the_text_is_its_place_in_the_number() {
    let raw = *Guid::from_u128(VALUE).raw();
    assert_eq!(raw.data1, 0x1234_5678);
    assert_eq!(raw.data2, 0x9abc);
    assert_eq!(raw.data3, 0xdef0);
    assert_eq!(raw.data4, [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88]);
}

#[test]
fn it_displays_as_the_registry_writes_one() {
    assert_eq!(
        Guid::from_u128(VALUE).to_string(),
        "{12345678-9abc-def0-1122-334455667788}"
    );
}

#[test]
fn it_displays_the_zero_value_with_every_digit() {
    assert_eq!(
        Guid::from_u128(0).to_string(),
        "{00000000-0000-0000-0000-000000000000}"
    );
}

#[test]
fn a_group_with_leading_zeros_keeps_them() {
    assert_eq!(
        Guid::from_u128(0x0000_0001_0002_0003_0004_0000_0000_0005).to_string(),
        "{00000001-0002-0003-0004-000000000005}"
    );
}

#[test]
fn it_debugs_as_its_text() {
    assert_eq!(
        format!("{:?}", Guid::from_u128(1)),
        "Guid({00000000-0000-0000-0000-000000000001})"
    );
}

#[test]
fn the_structure_is_laid_out_as_the_machine_holds_the_first_three_groups() {
    let guid = Guid::from_u128(VALUE);
    let raw = guid.raw();
    // SAFETY: the structure is plain data, so its bytes are valid to read.
    let bytes: [u8; 16] = unsafe { std::mem::transmute_copy(raw) };
    assert_eq!(&bytes[..4], &0x1234_5678_u32.to_ne_bytes());
    assert_eq!(&bytes[4..6], &0x9abc_u16.to_ne_bytes());
    assert_eq!(&bytes[6..8], &0xdef0_u16.to_ne_bytes());
    assert_eq!(
        &bytes[8..],
        &[0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88]
    );
}

#[test]
fn equal_values_are_equal_and_others_are_not() {
    assert_eq!(Guid::from_u128(VALUE), Guid::from_u128(VALUE));
    assert_ne!(Guid::from_u128(VALUE), Guid::from_u128(VALUE + 1));
}

#[test]
fn it_is_usable_in_a_const() {
    const ID: Guid = Guid::from_u128(VALUE);
    const BACK: u128 = ID.as_u128();
    assert_eq!(BACK, VALUE);
}
