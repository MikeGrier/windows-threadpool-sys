// Copyright (c) 2026 Mike Grier
//! The descriptors built from fields (WE-D-2): each field is the bytes it has in memory, a terminated string is its
//! units and then a zero, and the blocks are in the order of the fields. The tests read through the very descriptors
//! that would be passed to `EventWrite`, so they say what an event's data would be.

use super::{Built, Field, build, size_of_block};
use crate::guid::Guid;
use crate::registration::WriteError;

/// The bytes the descriptors describe, concatenated as the page says `EventWrite` does.
fn data(built: &Built<'_>) -> Vec<u8> {
    let mut out = Vec::new();
    for descriptor in built.descriptors() {
        // SAFETY: each descriptor describes memory held by the fields the test borrows, or a static, valid for
        // reads of its size.
        let block = unsafe {
            std::slice::from_raw_parts(
                descriptor.Ptr as usize as *const u8,
                descriptor.Size as usize,
            )
        };
        out.extend_from_slice(block);
    }
    out
}

fn of(fields: &[Field<'_>]) -> Vec<u8> {
    data(&build(fields).expect("fields that can be built"))
}

fn blocks(fields: &[Field<'_>]) -> usize {
    build(fields)
        .expect("fields that can be built")
        .descriptors()
        .len()
}

#[test]
fn no_fields_are_passed_as_a_count_of_zero_and_a_null_array() {
    // The page: "UserData may be NULL if UserDataCount is zero". A dangling pointer is what an empty vector holds, and
    // the call refused an event given one.
    let built = build(&[]).expect("built");
    let (count, blocks) = built.as_call();
    assert_eq!(count, 0);
    assert!(blocks.is_null());
}

#[test]
fn fields_are_passed_as_their_count_and_the_array() {
    let built = build(&[Field::U8(1), Field::Utf16Terminated(&[1])]).expect("built");
    let (count, blocks) = built.as_call();
    assert_eq!(count, 3);
    assert_eq!(blocks, built.descriptors().as_ptr());
}

#[test]
fn no_fields_are_no_blocks_and_no_data() {
    assert_eq!(blocks(&[]), 0);
    assert!(of(&[]).is_empty());
}

#[test]
fn the_integers_are_their_bytes_in_the_machines_order() {
    assert_eq!(of(&[Field::U8(0xab)]), [0xab]);
    assert_eq!(of(&[Field::I8(-2)]), [0xfe]);
    assert_eq!(of(&[Field::U16(0x1234)]), 0x1234_u16.to_ne_bytes());
    assert_eq!(of(&[Field::I16(-2)]), (-2_i16).to_ne_bytes());
    assert_eq!(
        of(&[Field::U32(0x1234_5678)]),
        0x1234_5678_u32.to_ne_bytes()
    );
    assert_eq!(of(&[Field::I32(-3)]), (-3_i32).to_ne_bytes());
    assert_eq!(
        of(&[Field::U64(u64::MAX - 1)]),
        (u64::MAX - 1).to_ne_bytes()
    );
    assert_eq!(of(&[Field::I64(i64::MIN)]), i64::MIN.to_ne_bytes());
}

#[test]
fn each_integer_is_one_block_of_its_size() {
    for (field, size) in [
        (Field::U8(0), 1),
        (Field::I8(0), 1),
        (Field::U16(0), 2),
        (Field::I16(0), 2),
        (Field::U32(0), 4),
        (Field::I32(0), 4),
        (Field::U64(0), 8),
        (Field::I64(0), 8),
        (Field::F32(0.0), 4),
        (Field::F64(0.0), 8),
        (Field::Pointer(0), size_of::<usize>()),
        (Field::Guid(Guid::from_u128(0)), 16),
    ] {
        let built = build(std::slice::from_ref(&field)).expect("built");
        assert_eq!(built.descriptors().len(), 1, "{field:?}");
        assert_eq!(built.descriptors()[0].Size as usize, size, "{field:?}");
    }
}

#[test]
fn the_floats_are_their_ieee_bits() {
    assert_eq!(of(&[Field::F32(1.5)]), 1.5_f32.to_ne_bytes());
    assert_eq!(of(&[Field::F64(-2.25)]), (-2.25_f64).to_ne_bytes());
}

#[test]
fn a_boolean_is_four_bytes() {
    assert_eq!(of(&[Field::boolean(true)]), 1_u32.to_ne_bytes());
    assert_eq!(of(&[Field::boolean(false)]), 0_u32.to_ne_bytes());
}

#[test]
fn a_pointer_is_as_wide_as_a_pointer() {
    assert_eq!(of(&[Field::Pointer(0x1000)]), 0x1000_usize.to_ne_bytes());
}

#[test]
fn a_guid_is_the_structure_with_the_first_three_groups_in_the_machines_order() {
    let guid = Guid::from_u128(0x1234_5678_9abc_def0_1122_3344_5566_7788);
    let mut expected = Vec::new();
    expected.extend_from_slice(&0x1234_5678_u32.to_ne_bytes());
    expected.extend_from_slice(&0x9abc_u16.to_ne_bytes());
    expected.extend_from_slice(&0xdef0_u16.to_ne_bytes());
    expected.extend_from_slice(&[0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88]);
    assert_eq!(of(&[Field::Guid(guid)]), expected);
}

#[test]
fn bytes_are_written_as_given_with_no_length_and_no_terminator() {
    assert_eq!(of(&[Field::Bytes(&[1, 2, 3])]), [1, 2, 3]);
    assert_eq!(blocks(&[Field::Bytes(&[1, 2, 3])]), 1);
}

#[test]
fn empty_bytes_are_a_block_of_no_size() {
    assert_eq!(blocks(&[Field::Bytes(&[])]), 1);
    assert!(of(&[Field::Bytes(&[])]).is_empty());
}

#[test]
fn utf16_units_are_written_as_given_with_no_terminator() {
    assert_eq!(
        of(&[Field::Utf16(&[0x68, 0x69])]),
        [0x68_u16.to_ne_bytes(), 0x69_u16.to_ne_bytes()].concat()
    );
    assert_eq!(blocks(&[Field::Utf16(&[0x68, 0x69])]), 1);
}

#[test]
fn a_terminated_utf16_string_is_its_units_then_a_zero_unit_in_two_blocks() {
    let fields = [Field::Utf16Terminated(&[0x68, 0x69])];
    assert_eq!(
        of(&fields),
        [0x68_u16.to_ne_bytes(), 0x69_u16.to_ne_bytes(), [0, 0]].concat()
    );
    assert_eq!(blocks(&fields), 2);
}

#[test]
fn an_empty_terminated_utf16_string_is_the_zero_unit_alone() {
    assert_eq!(of(&[Field::Utf16Terminated(&[])]), [0, 0]);
}

#[test]
fn a_terminated_byte_string_is_its_bytes_then_a_zero_byte() {
    assert_eq!(of(&[Field::BytesTerminated(b"hi")]), [b'h', b'i', 0]);
    assert_eq!(blocks(&[Field::BytesTerminated(b"hi")]), 2);
    assert_eq!(of(&[Field::BytesTerminated(b"")]), [0]);
}

#[test]
fn a_zero_already_in_a_terminated_string_is_written_as_it_is() {
    assert_eq!(
        of(&[Field::Utf16Terminated(&[0x61, 0, 0x62])]),
        [
            0x61_u16.to_ne_bytes(),
            [0, 0],
            0x62_u16.to_ne_bytes(),
            [0, 0]
        ]
        .concat()
    );
    assert_eq!(
        of(&[Field::BytesTerminated(&[b'a', 0, b'b'])]),
        [b'a', 0, b'b', 0]
    );
}

#[test]
fn an_unpaired_surrogate_is_a_unit_like_any_other() {
    assert_eq!(of(&[Field::Utf16(&[0xd800])]), 0xd800_u16.to_ne_bytes());
}

#[test]
fn fields_are_concatenated_in_order_with_no_padding() {
    let fields = [
        Field::U8(1),
        Field::U64(2),
        Field::U16(3),
        Field::Utf16Terminated(&[0x41]),
        Field::U32(4),
    ];
    let expected = [
        vec![1],
        2_u64.to_ne_bytes().to_vec(),
        3_u16.to_ne_bytes().to_vec(),
        0x41_u16.to_ne_bytes().to_vec(),
        vec![0, 0],
        4_u32.to_ne_bytes().to_vec(),
    ]
    .concat();
    assert_eq!(of(&fields), expected);
    assert_eq!(blocks(&fields), 6);
}

#[test]
fn the_blocks_are_the_events_in_the_order_the_fields_were_given() {
    let built = build(&[Field::U8(1), Field::U8(2), Field::U8(3)]).expect("built");
    let sizes: Vec<u32> = built.descriptors().iter().map(|d| d.Size).collect();
    assert_eq!(sizes, [1, 1, 1]);
    assert_eq!(data(&built), [1, 2, 3]);
}

#[test]
fn a_descriptors_address_is_that_of_the_field_and_its_type_is_zero() {
    let value = 0x55_u32;
    let fields = [Field::U32(value)];
    let built = build(&fields).expect("built");
    let descriptor = &built.descriptors()[0];
    let Field::U32(inside) = &fields[0] else {
        unreachable!("built as a U32")
    };
    assert_eq!(descriptor.Ptr, std::ptr::from_ref(inside) as usize as u64);
    // The reserved word is the type and the two reserved fields, all zero: "Normal event data".
    // SAFETY: both members are plain integers over the same four bytes.
    assert_eq!(unsafe { descriptor.Anonymous.Reserved }, 0);
}

#[test]
fn a_terminated_strings_second_block_is_the_same_static_each_time() {
    let a = build(&[Field::Utf16Terminated(&[1])]).expect("built");
    let b = build(&[Field::Utf16Terminated(&[2, 3])]).expect("built");
    assert_eq!(a.descriptors()[1].Ptr, b.descriptors()[1].Ptr);
    assert_eq!(a.descriptors()[1].Size, 2);
}

#[test]
fn a_large_list_is_every_field() {
    let fields: Vec<Field<'_>> = (0..1000_u32).map(Field::U32).collect();
    assert_eq!(blocks(&fields), 1000);
    assert_eq!(of(&fields).len(), 4000);
}

#[test]
fn a_size_that_fits_a_ulong_is_kept() {
    assert_eq!(size_of_block(0), Some(0));
    assert_eq!(size_of_block(u32::MAX as usize), Some(u32::MAX));
}

#[test]
fn a_size_that_does_not_fit_a_ulong_has_none() {
    assert_eq!(size_of_block(u32::MAX as usize + 1), None);
    assert_eq!(size_of_block(usize::MAX), None);
}

#[test]
fn a_block_too_long_is_refused_naming_its_field() {
    let refused = super::descriptor(
        super::Block {
            ptr: std::ptr::null(),
            len: u32::MAX as usize + 1,
        },
        3,
    );
    assert!(matches!(
        refused,
        Err(WriteError::BlockTooLong { field: 3, len }) if len == u32::MAX as usize + 1
    ));
}
