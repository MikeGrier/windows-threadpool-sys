// Copyright (c) Mike Grier.

//! Tests for the aligned buffer primitive.

use super::AlignedBuffer;

/// The two alignments this crate actually needs, plus the boundaries either
/// side of them.
const ALIGNMENTS: [usize; 5] = [1, 2, 4, 8, 16];

fn is_aligned(buffer: &AlignedBuffer) -> bool {
    (buffer.as_ptr() as usize).is_multiple_of(buffer.align())
}

#[test]
fn a_zeroed_buffer_has_the_requested_length_and_alignment() {
    for align in ALIGNMENTS {
        for len in [1_usize, 3, 7, 20, 4096] {
            let buffer = AlignedBuffer::zeroed(len, align);

            assert_eq!(buffer.len(), len, "len {len}, align {align}");
            assert_eq!(buffer.align(), align, "len {len}, align {align}");
            assert!(is_aligned(&buffer), "len {len}, align {align}");
        }
    }
}

#[test]
fn a_zeroed_buffer_is_zeroed() {
    let buffer = AlignedBuffer::zeroed(512, 8);

    assert!(buffer.as_slice().iter().all(|byte| *byte == 0));
}

#[test]
fn an_empty_buffer_is_still_aligned_and_never_dereferenced() {
    for align in ALIGNMENTS {
        let buffer = AlignedBuffer::zeroed(0, align);

        assert!(buffer.is_empty());
        assert_eq!(buffer.len(), 0);
        assert_eq!(buffer.align(), align);
        assert!(is_aligned(&buffer));
        assert!(buffer.as_slice().is_empty());
    }
}

#[test]
fn a_length_that_is_not_a_multiple_of_the_alignment_is_fine() {
    // The alignment constrains the buffer's address, not its size. A
    // 5-byte, 8-aligned buffer is a legitimate request.
    let buffer = AlignedBuffer::zeroed(5, 8);

    assert_eq!(buffer.len(), 5);
    assert!(is_aligned(&buffer));
}

#[test]
fn from_bytes_copies_the_contents() {
    let source: Vec<u8> = (0..=255_u8).collect();

    let buffer = AlignedBuffer::from_bytes(&source, 4);

    assert_eq!(buffer.as_slice(), source.as_slice());
    assert!(is_aligned(&buffer));
}

#[test]
fn from_bytes_accepts_an_empty_source() {
    let buffer = AlignedBuffer::from_bytes(&[], 8);

    assert!(buffer.is_empty());
    assert!(is_aligned(&buffer));
}

#[test]
fn the_contents_can_be_written_through_the_mutable_slice() {
    let mut buffer = AlignedBuffer::zeroed(4, 4);

    buffer.as_mut_slice().copy_from_slice(&[1, 2, 3, 4]);

    assert_eq!(buffer.as_slice(), &[1, 2, 3, 4]);
}

#[test]
fn the_mutable_pointer_addresses_the_same_bytes_as_the_slice() {
    let mut buffer = AlignedBuffer::zeroed(8, 8);
    let expected = buffer.as_ptr();

    assert_eq!(buffer.as_mut_ptr().cast_const(), expected);
}

#[test]
fn a_clone_copies_contents_and_alignment_but_not_the_address() {
    let mut original = AlignedBuffer::zeroed(64, 8);
    original.as_mut_slice()[7] = 0xAB;

    let clone = original.clone();

    assert_eq!(clone.as_slice(), original.as_slice());
    assert_eq!(clone.align(), original.align());
    assert_ne!(clone.as_ptr(), original.as_ptr());
    assert!(is_aligned(&clone));
}

#[test]
fn a_clone_is_independent_of_its_original() {
    let original = AlignedBuffer::zeroed(16, 4);
    let mut clone = original.clone();

    clone.as_mut_slice()[0] = 0xFF;

    assert_eq!(original.as_slice()[0], 0);
}

#[test]
fn equality_compares_contents_and_alignment() {
    let four = AlignedBuffer::from_bytes(&[1, 2, 3], 4);
    let also_four = AlignedBuffer::from_bytes(&[1, 2, 3], 4);
    let eight = AlignedBuffer::from_bytes(&[1, 2, 3], 8);
    let different = AlignedBuffer::from_bytes(&[1, 2, 4], 4);

    assert_eq!(four, also_four);
    assert_ne!(four, eight, "alignment is part of what the buffer promises");
    assert_ne!(four, different);
}

#[test]
fn the_debug_form_reports_shape_rather_than_contents() {
    let buffer = AlignedBuffer::zeroed(32, 8);

    let rendered = format!("{buffer:?}");

    assert!(rendered.contains("len: 32"), "unexpected: {rendered}");
    assert!(rendered.contains("align: 8"), "unexpected: {rendered}");
}

#[test]
#[should_panic(expected = "an alignment that is a power of two")]
fn an_alignment_that_is_not_a_power_of_two_is_a_programming_error() {
    let _ = AlignedBuffer::zeroed(16, 3);
}

#[test]
#[should_panic(expected = "an alignment that is a power of two")]
fn a_zero_alignment_is_a_programming_error() {
    let _ = AlignedBuffer::zeroed(16, 0);
}

#[test]
fn a_buffer_moves_and_shares_across_threads() {
    const fn assert_send<T: Send>() {}
    const fn assert_sync<T: Sync>() {}

    assert_send::<AlignedBuffer>();
    assert_sync::<AlignedBuffer>();

    let buffer = AlignedBuffer::from_bytes(&[9; 32], 8);

    let observed = std::thread::spawn(move || buffer.as_slice().to_vec())
        .join()
        .expect("the worker did not panic");

    assert_eq!(observed, vec![9_u8; 32]);
}

#[test]
fn many_live_buffers_stay_aligned_and_distinct() {
    let buffers: Vec<AlignedBuffer> = (1..=64).map(|len| AlignedBuffer::zeroed(len, 8)).collect();

    for (index, buffer) in buffers.iter().enumerate() {
        assert_eq!(buffer.len(), index + 1);
        assert!(is_aligned(buffer));
    }
}
